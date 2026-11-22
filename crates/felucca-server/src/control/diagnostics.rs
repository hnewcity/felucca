//! Tenant-filtered runtime inspection. HTTP never initiates backend probes.
use super::{Principal, Result, resources};
use crate::Server;
use felucca_core::ServerConfig;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Default)]
pub(crate) struct PeerHistory {
    history: parking_lot::Mutex<VecDeque<Value>>,
    active: parking_lot::Mutex<BTreeMap<uuid::Uuid, Value>>,
    truncated: std::sync::atomic::AtomicBool,
}
impl PeerHistory {
    pub(crate) fn session<'a>(
        &'a self,
        client: &'a str,
        tunnel: u32,
        visitor: &'a str,
    ) -> PeerSession<'a> {
        self.record(client, tunnel, visitor, "direct");
        let id = uuid::Uuid::new_v4();
        let mut active = self.active.lock();
        if active.len() < 512 {
            active.insert(
                id,
                json!({"id":id,"client":client,"tunnel":tunnel,"visitor":visitor}),
            );
        } else {
            self.truncated
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        PeerSession {
            history: self,
            client,
            tunnel,
            visitor,
            id,
        }
    }
    pub(crate) fn record(&self, client: &str, tunnel: u32, visitor: &str, state: &str) {
        let mut history = self.history.lock();
        if history.len() == 512 {
            history.pop_front();
        }
        history.push_back(json!({"client":client,"tunnel":tunnel,"visitor":visitor,
            "state":state,"timestamp_ms":std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64}));
    }
    pub(crate) fn visible(&self, config: &ServerConfig, principal: &Principal) -> Value {
        let visible = |event: &&Value| {
            config
                .clients
                .get(event["client"].as_str().unwrap_or(""))
                .is_some_and(|client| resources::visible(principal, client.user.as_deref()))
        };
        let events: Vec<_> = self
            .history
            .lock()
            .iter()
            .filter(visible)
            .cloned()
            .collect();
        let active: Vec<_> = self
            .active
            .lock()
            .values()
            .filter(visible)
            .cloned()
            .collect();
        json!({"events":events,"active":active,"truncated":self.truncated.load(std::sync::atomic::Ordering::Relaxed)})
    }
}

pub(crate) struct PeerSession<'a> {
    history: &'a PeerHistory,
    client: &'a str,
    tunnel: u32,
    visitor: &'a str,
    id: uuid::Uuid,
}
impl Drop for PeerSession<'_> {
    fn drop(&mut self) {
        self.history.active.lock().remove(&self.id);
        self.history
            .record(self.client, self.tunnel, self.visitor, "closed");
    }
}

pub(crate) async fn traffic(
    server: &Server,
    config: &ServerConfig,
    principal: &Principal,
) -> Value {
    let specs = server.tunnel_specs().await;
    let mut routes = Vec::new();
    for (client, spec) in specs {
        let owner = config
            .clients
            .get(&client)
            .map(|c| c.user.as_deref())
            .or_else(|| {
                client
                    .strip_prefix("local.")
                    .and_then(|name| config.locals.get(name))
                    .map(|c| c.user.as_deref())
            });
        if !owner.is_some_and(|owner| resources::visible(principal, owner)) {
            continue;
        }
        let stats = server
            .registry()
            .store
            .stats(&crate::resources::ResourceKey::Tunnel {
                client: client.clone(),
                id: spec.tunnel_id,
            })
            .unwrap_or_default();
        routes.push(json!({"client":client,"id":spec.tunnel_id,"mode":spec.mode,"traffic":stats}));
    }
    json!({"tunnels":routes,"peers":server.peer_history.visible(config,principal)})
}

pub(crate) async fn backends(
    server: &Server,
    config: &ServerConfig,
    principal: &Principal,
    client: &str,
    id: u32,
) -> Result<Value> {
    let resource = if let Some(local) = client.strip_prefix("local.") {
        super::Resource::Local(local.to_owned())
    } else {
        super::Resource::Client(client.to_owned())
    };
    resources::read(config, principal, &resource)?;
    let result = server.backend_diagnostics(client, id).await;
    Ok(match result {
        Ok(report) => json!({"available":true,"report":report}),
        Err(_) => json!({"available":false}),
    })
}
