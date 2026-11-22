//! Owns registered routes, listener replacement and generation-safe cleanup.
use crate::{
    forward,
    registry::{Client, Registry},
    relay::Route,
    reverse::ReverseProxy,
};
use felucca_core::{ServerConfig, config::ClientEntry, geodata::GeoResolver};
use felucca_proto::{ConfigRejection, Control, TunnelMode, TunnelSpec};
use felucca_transport::TunnelHandle;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
#[derive(Debug)]
struct Registered {
    spec: TunnelSpec,
    listener: Option<forward::Tunnel>,
}
#[derive(Debug)]
struct Registration {
    client: felucca_core::ClientId,
    entries: HashMap<u32, Registered>,
}
fn same_route(left: &TunnelSpec, right: &TunnelSpec) -> bool {
    let mut left = left.clone();
    left.remark.clone_from(&right.remark);
    left == *right
}

#[derive(Debug)]
pub(super) struct Registrations {
    entries: Mutex<HashMap<String, Registration>>,
    config: Arc<ServerConfig>,
    registry: Arc<Registry>,
    reverse: Option<Arc<ReverseProxy>>,
    geo: GeoResolver,
}
impl Registrations {
    pub(super) fn new(
        config: Arc<ServerConfig>,
        registry: Arc<Registry>,
        reverse: Option<Arc<ReverseProxy>>,
        geo: GeoResolver,
    ) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            config,
            registry,
            reverse,
            geo,
        }
    }
    pub(super) async fn attach(
        &self,
        key: &str,
        entry: &ClientEntry,
        tunnel: TunnelHandle,
        source_acls: Arc<Vec<felucca_core::Acl>>,
        secure: bool,
    ) -> Arc<Client> {
        let mut entries = self.entries.lock().await;
        if let Some(old) = self.registry.get(key) {
            old.cancel.cancel();
            old.tunnel.close(felucca_mux::CloseReason::Shutdown);
        }
        if let Some(old) = entries.remove(key) {
            self.stop_registration(old).await;
        }
        let client = self
            .registry
            .insert_with_acls(key, entry, tunnel, source_acls);
        client
            .secure
            .store(secure, std::sync::atomic::Ordering::Release);
        entries.insert(
            key.to_owned(),
            Registration {
                client: client.id,
                entries: HashMap::new(),
            },
        );
        client
    }
    async fn stop_registration(&self, old: Registration) {
        if let Some(reverse) = &self.reverse {
            reverse.remove(old.client);
        }
        for item in old.entries.into_values() {
            if let Some(listener) = item.listener {
                listener.stop_and_wait().await;
            }
        }
    }
    pub(super) async fn shutdown(&self) {
        let registrations = std::mem::take(&mut *self.entries.lock().await);
        for old in registrations.into_values() {
            self.stop_registration(old).await;
        }
        for client in self.registry.clients() {
            self.registry.remove_if_current(&client);
        }
    }
    pub(super) async fn specs(&self) -> Vec<(String, TunnelSpec)> {
        self.entries
            .lock()
            .await
            .iter()
            .flat_map(|(key, registration)| {
                registration
                    .entries
                    .values()
                    .map(|entry| (key.clone(), entry.spec.clone()))
            })
            .collect()
    }
    pub(super) async fn route(&self, key: &str, id: u32) -> std::io::Result<(TunnelSpec, Route)> {
        let registrations = self.entries.lock().await;
        let client = self
            .registry
            .get(key)
            .ok_or(std::io::ErrorKind::NotConnected)?;
        let item = registrations
            .get(key)
            .filter(|r| r.client == client.id)
            .and_then(|r| r.entries.get(&id))
            .ok_or(std::io::ErrorKind::NotFound)?;
        let spec = item.spec.clone();
        let route = Route {
            cancel: client.route_cancel(id),
            trace: Default::default(),
            resources: client.route_resources(id, spec.options.limits),
            host_scope: None,
            id,
            client,
            acl: Arc::new(
                felucca_core::Acl::parse_with_geo(spec.acl_mode, &spec.acl_rules, &self.geo)
                    .map_err(std::io::Error::other)?,
            ),
            options: Arc::new(spec.options.clone()),
            destination: felucca_core::config::parse_bind_host(spec.bind.as_deref(), spec.port)
                .map_err(std::io::Error::other)?,
            datagram: spec.mode == TunnelMode::Udp,
        };
        Ok((spec, route))
    }
    pub(super) async fn retire(&self, client: &Arc<Client>) {
        let mut registrations = self.entries.lock().await;
        if self.registry.remove_if_current(client) {
            if let Some(reverse) = &self.reverse {
                reverse.remove(client.id);
            }
            if let Some(old) = registrations.remove(&client.key_id) {
                for item in old.entries.into_values() {
                    if let Some(listener) = item.listener {
                        listener.stop_and_wait().await;
                    }
                }
            }
        }
    }
    pub(super) async fn reconcile(&self, client: &Arc<Client>, specs: &[TunnelSpec]) {
        let mut registrations = self.entries.lock().await;
        let Some(registration) = registrations
            .get_mut(&client.key_id)
            .filter(|entry| entry.client == client.id)
        else {
            return;
        };
        let obsolete: Vec<_> = registration
            .entries
            .iter()
            .filter(|(_, entry)| !specs.iter().any(|spec| same_route(spec, &entry.spec)))
            .map(|(id, _)| *id)
            .collect();
        for id in obsolete {
            client.cancel_route(id);
            if let Some(reverse) = &self.reverse {
                reverse.remove_tunnel(client.id, id);
            }
            if let Some(entry) = registration.entries.remove(&id)
                && let Some(listener) = entry.listener
            {
                listener.stop_and_wait().await;
            }
        }
    }
    pub(super) async fn register(&self, specs: &[TunnelSpec], client: &Arc<Client>) -> Control {
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        let mut registrations = self.entries.lock().await;
        for spec in specs {
            let outcome = async {
                spec.options.validate().map_err(|e| e.to_string())?;
                if spec.options.transparent_tproxy && spec.mode != TunnelMode::Transparent {
                    return Err("TPROXY requires transparent TCP".into());
                }
                if client.cancel.is_cancelled() {
                    return Err("client may not register tunnels".to_owned());
                }
                let registration = registrations
                    .get_mut(&client.key_id)
                    .filter(|r| r.client == client.id)
                    .ok_or("stale registration")?;
                if let Some(existing) = registration.entries.get_mut(&spec.tunnel_id) {
                    return if same_route(&existing.spec, spec) {
                        existing.spec.remark.clone_from(&spec.remark);
                        Ok(())
                    } else {
                        Err("tunnel id already registered with different configuration".into())
                    };
                }
                if registration.entries.len() >= 4096 || spec.hosts.len() > 256 {
                    return Err("registration exceeds 4096 tunnels or 256 hosts per tunnel".into());
                }
                if client
                    .max_tunnels
                    .is_some_and(|max| registration.entries.len() >= max)
                {
                    return Err("tunnel limit reached".into());
                }
                let acl =
                    felucca_core::Acl::parse_with_geo(spec.acl_mode, &spec.acl_rules, &self.geo)
                        .map_err(|e| e.to_string())?;
                let listener = if !spec.public {
                    if spec.mode == TunnelMode::Transparent
                        || spec.port != 0
                        || spec.bind.is_some()
                        || spec.mode == TunnelMode::Http
                        || !spec.hosts.is_empty()
                        || spec.options.proxy_accept.mode != felucca_core::config::ProxyMode::Off
                    {
                        return Err("invalid private tunnel registration".into());
                    }
                    None
                } else if spec.mode == TunnelMode::Http {
                    let reverse = self
                        .reverse
                        .as_ref()
                        .ok_or("HTTP proxy is disabled on the server")?;
                    let matches_port = [
                        &reverse.config.bind,
                        &reverse.config.tls_bind,
                        &reverse.config.http3_bind,
                    ]
                    .into_iter()
                    .flatten()
                    .any(|b| {
                        felucca_core::config::parse_bind(b).is_ok_and(|a| a.port() == spec.port)
                    });
                    if !matches_port {
                        return Err(
                            "HTTP tunnel port must match a server HTTP/HTTPS listener".into()
                        );
                    }
                    reverse.register(
                        &spec.hosts,
                        Route {
                            cancel: client.route_cancel(spec.tunnel_id),
                            trace: Default::default(),
                            resources: client.route_resources(spec.tunnel_id, spec.options.limits),
                            host_scope: None,
                            id: spec.tunnel_id,
                            client: client.clone(),
                            acl: Arc::new(acl),
                            options: Arc::new(spec.options.clone()),
                            destination: ([0, 0, 0, 0], spec.port).into(),
                            datagram: false,
                        },
                    )?;
                    None
                } else {
                    if !self.config.port_allowed(spec.port) {
                        return Err(format!("port {} is not allowed", spec.port));
                    }
                    let bind =
                        felucca_core::config::parse_bind_host(spec.bind.as_deref(), spec.port)
                            .map_err(|e| e.to_string())?;
                    Some(
                        forward::start_with_acl(spec, bind, client.clone(), acl)
                            .await
                            .map_err(|e| e.to_string())?,
                    )
                };
                registration.entries.insert(
                    spec.tunnel_id,
                    Registered {
                        spec: spec.clone(),
                        listener,
                    },
                );
                Ok(())
            }
            .await;
            match outcome {
                Ok(()) => accepted.push(spec.tunnel_id),
                Err(reason) => rejected.push(ConfigRejection {
                    tunnel_id: spec.tunnel_id,
                    reason,
                }),
            }
        }
        Control::ConfigResult { accepted, rejected }
    }
}
