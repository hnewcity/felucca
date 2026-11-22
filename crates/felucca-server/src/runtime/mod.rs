//! Bridge transport dispatch, authenticated sessions and serialized registrations.
use crate::{
    error::ServerError,
    registry::{Client, Registry},
    relay::Route,
    reverse::ReverseProxy,
};
use bytes::Bytes;
use felucca_core::ServerConfig;
use felucca_proto::{AcceptPolicy, Control, StreamRequest, TunnelSpec, read_frame, write_frame};
use felucca_transport::{Identity, PrefixedConn, TransportError, Tunnel, TunnelStream, tcp};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio_util::sync::CancellationToken;

mod listeners;
pub(crate) mod live;
mod local;
mod registration;
mod session;
#[derive(Debug)]
pub struct Server {
    pub(crate) live: arc_swap::ArcSwap<live::LivePolicy>,
    policy_gate: tokio::sync::RwLock<()>,
    pub(crate) peer_history: crate::control::diagnostics::PeerHistory,
    pub(crate) peer_broker: parking_lot::RwLock<Option<Arc<felucca_transport::rendezvous::Broker>>>,
    admin: parking_lot::RwLock<std::sync::Weak<crate::admin::Admin>>,
    tasks: tokio_util::task::TaskTracker,
    local_services: HashMap<String, Arc<felucca_service::Services>>,
    clock: Arc<felucca_transport::clock::Clock>,
    source_acls: HashMap<String, Arc<Vec<felucca_core::Acl>>>,
    config: Arc<ServerConfig>,
    registry: Arc<Registry>,
    registrations: registration::Registrations,
    identity: arc_swap::ArcSwap<Identity>,
    bridge_tls: arc_swap::ArcSwap<rustls::ServerConfig>,
    proxy_accept: felucca_transport::proxy_protocol::Acceptor,
    reverse: Option<Arc<ReverseProxy>>,
    cancel: CancellationToken,
}
impl Server {
    pub(crate) fn certificate_diagnostics(&self) -> serde_json::Value {
        let mut certificates = self
            .reverse
            .as_ref()
            .map(|proxy| proxy.certificates.snapshot())
            .unwrap_or_default();
        if let Some(cert) = self.identity.load().chain.first() {
            certificates.push(crate::certificates::status::certificate("bridge", cert));
        }
        serde_json::json!({"certificates":certificates,
            "events":self.reverse.as_ref().map(|proxy|proxy.certificate_status.snapshot()).unwrap_or_default(),
            "acme":self.reverse.as_ref().is_some_and(|proxy|proxy.config.acme.is_some())})
    }
    pub(crate) fn retry_certificates(&self) -> bool {
        if let Some(proxy) = self
            .reverse
            .as_ref()
            .filter(|proxy| proxy.config.acme.is_some())
        {
            proxy.certificate_status.retry.notify_one();
            true
        } else {
            false
        }
    }
    pub(crate) fn p2p_config(&self) -> Option<&felucca_core::config::peer::P2pServerConfig> {
        self.config.p2p.as_ref()
    }
    pub(crate) fn attach_admin(&self, admin: std::sync::Weak<crate::admin::Admin>) {
        *self.admin.write() = admin;
    }
    pub async fn tunnel_specs(&self) -> Vec<(String, TunnelSpec)> {
        self.registrations.specs().await
    }
    pub fn new(config: ServerConfig) -> Result<Self, ServerError> {
        Self::with_runtime(config, None, None)
    }
    pub(crate) fn with_identity(
        config: ServerConfig,
        identity: Option<Arc<Identity>>,
    ) -> Result<Self, ServerError> {
        Self::with_runtime(config, None, identity)
    }
    pub(crate) fn replacement(&self, config: ServerConfig) -> Result<Self, ServerError> {
        if self.config.log != config.log {
            return Err(felucca_core::ConfigError::invalid(
                "changing log settings requires a daemon restart",
            )
            .into());
        }
        if config.state.as_ref().map(|s| &s.path) != self.config.state.as_ref().map(|s| &s.path) {
            return Err(felucca_core::ConfigError::invalid(
                "changing the traffic state path requires a daemon restart",
            )
            .into());
        }
        Self::with_runtime(
            config,
            Some((self.registry.store.clone(), self.identity.load_full())),
            None,
        )
    }
    fn with_runtime(
        mut config: ServerConfig,
        existing: Option<(Arc<crate::resources::MeterStore>, Arc<Identity>)>,
        initial_identity: Option<Arc<Identity>>,
    ) -> Result<Self, ServerError> {
        config.validate()?;
        let geo = felucca_core::geodata::GeoResolver::new(&config.geo)?;
        let parse_acl = |mode, rules: &str| {
            felucca_core::Acl::parse_with_geo(mode, rules, &geo)
                .map_err(|e| felucca_core::ConfigError::invalid(e.to_string()))
        };
        let global = parse_acl(config.acl_mode, &config.acl_rules)?;
        let mut source_acls = HashMap::new();
        for (id, client) in &config.clients {
            let mut scopes = vec![global.clone()];
            if let Some(user) = client.user.as_ref().and_then(|user| config.users.get(user)) {
                scopes.push(parse_acl(user.acl_mode, &user.acl_rules)?);
            }
            scopes.push(parse_acl(client.acl_mode, &client.acl_rules)?);
            source_acls.insert(id.clone(), Arc::new(scopes));
        }
        let mut local_services = HashMap::new();
        for (name, local) in &config.locals {
            let key = format!("local.{name}");
            let mut scopes = vec![global.clone()];
            if let Some(user) = local.user.as_ref().and_then(|user| config.users.get(user)) {
                scopes.push(parse_acl(user.acl_mode, &user.acl_rules)?);
            }
            scopes.push(parse_acl(local.acl_mode, &local.acl_rules)?);
            source_acls.insert(key, Arc::new(scopes));
            let network = Arc::new(felucca_transport::network::Network::new(
                &local.network,
                local.dial_timeout,
            )?);
            local_services.insert(
                name.clone(),
                Arc::new(felucca_service::Services::new(
                    &local.tunnels,
                    &config.geo,
                    network,
                )?),
            );
        }
        let store = match &existing {
            Some((store, _)) => store.clone(),
            None => Arc::new(crate::resources::MeterStore::open(config.state.as_ref())?),
        };
        let registry = Registry::with_store(
            store,
            config
                .users
                .iter()
                .map(|(name, user)| (name.clone(), user.limits))
                .collect(),
        );
        let _ = rustls::crypto::ring::default_provider().install_default();
        let identity = match &config.bridge.tls {
            Some(tls) => Arc::new(Identity::from_pem_files(&tls.cert, &tls.key)?),
            None if existing.is_some() => existing.as_ref().expect("present runtime").1.clone(),
            None if initial_identity.is_some() => initial_identity.expect("present identity"),
            None => {
                let identity = Identity::self_signed(vec!["felucca".into()])?;
                tracing::warn!(fingerprint = %identity.fingerprint(), "generated bridge certificate; pin this fingerprint in the agent");
                Arc::new(identity)
            }
        };
        let reverse = config
            .http
            .clone()
            .map(ReverseProxy::new)
            .transpose()?
            .map(Arc::new);
        let mut bridge_tls = (*identity.server_config()?).clone();
        bridge_tls.alpn_protocols = vec![
            felucca_transport::BRIDGE_ALPN.to_vec(),
            b"http/1.1".to_vec(),
        ];
        let config = Arc::new(config);
        let live = live::LivePolicy::prepare(config.clone(), None)?;
        let registry = Arc::new(registry);
        let registrations = registration::Registrations::new(
            config.clone(),
            registry.clone(),
            reverse.clone(),
            geo,
        );
        Ok(Self {
            peer_history: Default::default(),
            peer_broker: Default::default(),
            live: arc_swap::ArcSwap::from_pointee(live),
            policy_gate: Default::default(),
            admin: Default::default(),
            tasks: tokio_util::task::TaskTracker::new(),
            local_services,
            clock: Arc::new(felucca_transport::clock::Clock::new(config.ntp.clone())),
            source_acls,
            proxy_accept: felucca_transport::proxy_protocol::Acceptor::new(
                &config.bridge.proxy_accept,
            )?,
            bridge_tls: arc_swap::ArcSwap::from_pointee(bridge_tls),
            config,
            registry,
            registrations,
            identity: arc_swap::ArcSwap::from(identity),
            reverse,
            cancel: CancellationToken::new(),
        })
    }
    pub fn fingerprint(&self) -> String {
        self.identity.load().fingerprint()
    }
    pub(crate) async fn backend_diagnostics(
        &self,
        client: &str,
        id: u32,
    ) -> std::io::Result<felucca_proto::diagnostics::BackendReport> {
        if let Some(local) = client.strip_prefix("local.") {
            return self
                .local_services
                .get(local)
                .and_then(|services| services.diagnostics(id))
                .ok_or_else(|| std::io::ErrorKind::NotFound.into());
        }
        let node = self
            .registry
            .get(client)
            .ok_or(std::io::ErrorKind::NotConnected)?;
        // New read-only extension, requested on demand. An older agent can close
        // this stream without disturbing its forwarding/control streams.
        tokio::select! {
            () = self.cancel.cancelled() => Err(std::io::ErrorKind::Interrupted.into()),
            result = tokio::time::timeout(Duration::from_secs(5), async {
                let mut stream = node.tunnel.open().await.map_err(std::io::Error::other)?;
                write_frame(&mut stream, &StreamRequest::Diagnostics { tunnel_id: id }).await.map_err(std::io::Error::other)?;
                let report: felucca_proto::diagnostics::BackendReport = read_frame(&mut stream).await.map_err(std::io::Error::other)?;
                if report.backends.len() > felucca_proto::diagnostics::MAX_BACKENDS {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                Ok(report)
            }) => result.map_err(|_| std::io::ErrorKind::TimedOut)?,
        }
    }
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }
    pub(crate) fn reverse_proxy(&self) -> Option<Arc<ReverseProxy>> {
        self.reverse.clone()
    }
    pub fn shutdown(&self) {
        self.stop();
        if let Err(error) = self.registry.store.save() {
            tracing::error!(%error, "could not persist traffic during shutdown");
        }
    }
    fn stop(&self) {
        self.cancel.cancel();
        self.peer_broker.write().take();
        for client in self.registry.clients() {
            client.cancel.cancel();
            client.tunnel.close(felucca_mux::CloseReason::Shutdown);
        }
    }
    /// Stops this generation and waits for its relays to finish.
    ///
    /// Only a failed drain is an error. The traffic snapshot is saved once,
    /// off the runtime threads, after the relays stop; a failed save is logged
    /// and keeps the previous snapshot. Reporting it as an error would make a
    /// configuration reload on a full disk roll back and reconnect every agent
    /// although the listeners had already stopped cleanly.
    pub async fn shutdown_and_wait(&self) -> std::io::Result<()> {
        self.stop();
        self.registrations.shutdown().await;
        self.tasks.close();
        let drained = tokio::time::timeout(Duration::from_secs(15), async {
            self.tasks.wait().await;
            if let Some(reverse) = &self.reverse {
                reverse.stopped().await;
            }
            while self.registry.store.has_live_connections() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        let store = self.registry.store.clone();
        match tokio::task::spawn_blocking(move || store.save()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(%error, "could not persist traffic during shutdown; retaining previous snapshot");
            }
            Err(error) => tracing::error!(%error, "traffic snapshot task failed during shutdown"),
        }
        if !drained {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "relays did not drain before the final traffic snapshot",
            ));
        }
        Ok(())
    }
}
