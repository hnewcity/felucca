//! Who is connected, and what they may do.
//!
//! One entry per authenticated agent, holding the tunnel handle its traffic
//! rides and the limits its traffic is measured against. The data path reaches
//! this on every inbound connection, so lookups are by key rather than a scan.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::resources::{Lease, MeterGroup, MeterStore, ResourceKey};
use dashmap::DashMap;
use felucca_core::config::ClientEntry;
use felucca_core::traffic::ResourceLimits;
use felucca_core::{Acl, ClientId, Quota, QuotaState, RateLimiter, TrafficCounter};
use felucca_transport::TunnelHandle;

/// One connected agent.
#[derive(Debug)]
pub struct Client {
    route_cancels:
        parking_lot::Mutex<std::collections::HashMap<u32, tokio_util::sync::CancellationToken>>,
    pub(crate) live_config: std::sync::atomic::AtomicBool,
    pub(crate) config_state: parking_lot::RwLock<ConfigState>,
    pub secure: std::sync::atomic::AtomicBool,
    pub user: Option<String>,
    pub max_tunnels: Option<usize>,
    pub(crate) vkey: felucca_core::Secret,
    pub id: ClientId,
    /// The `key_id` from the agent's config; how an operator refers to it.
    pub key_id: String,
    /// Where to open streams toward this agent.
    pub tunnel: TunnelHandle,
    pub traffic: Arc<TrafficCounter>,
    pub rate: Arc<RateLimiter>,
    pub quota: Quota,
    /// Who may reach this client's tunnels.
    pub source_acls: Arc<Vec<Acl>>,
    pub max_connections: Option<usize>,
    /// Live proxied connections, for the connection cap and the dashboard.
    pub resources: MeterGroup,
    pub store: Arc<MeterStore>,
    pub allow_config_push: bool,
    pub cancel: tokio_util::sync::CancellationToken,
}

#[derive(Debug, Default)]
pub(crate) struct ConfigState {
    pub revision: Option<[u8; 32]>,
    pub rejected: Vec<u32>,
    pub dirty: bool,
}

impl Client {
    pub fn route_cancel(&self, id: u32) -> tokio_util::sync::CancellationToken {
        self.route_cancels
            .lock()
            .entry(id)
            .or_insert_with(|| self.cancel.child_token())
            .clone()
    }
    pub(crate) fn cancel_route(&self, id: u32) {
        if let Some(cancel) = self.route_cancels.lock().remove(&id) {
            cancel.cancel();
        }
    }
    pub fn permits_source(&self, address: std::net::IpAddr) -> bool {
        self.source_acls.iter().all(|acl| acl.permits(address))
    }
    /// Takes a slot against the connection cap.
    ///
    /// Returns a guard so the count cannot leak: a forwarding task that returns
    /// early, panics, or is cancelled still gives its slot back.
    pub fn try_acquire(self: &Arc<Self>) -> Option<ConnectionGuard> {
        self.resources.acquire().map(ConnectionGuard::new)
    }

    /// Live proxied connections.
    pub fn live_connections(&self) -> usize {
        self.resources.0[0].live()
    }

    /// Whether this client's quota still permits traffic.
    pub fn quota_state(&self, now: i64) -> QuotaState {
        self.quota.check(self.traffic.total(), now)
    }
    pub fn route_resources(&self, id: u32, limits: ResourceLimits) -> MeterGroup {
        let mut resources = self.resources.clone();
        resources.0.push(self.store.meter(
            ResourceKey::Tunnel {
                client: self.key_id.clone(),
                id,
            },
            limits,
        ));
        resources
    }
}

/// Releases a connection slot when dropped.
#[derive(Debug)]
pub struct ConnectionGuard {
    _lease: Lease,
}

impl ConnectionGuard {
    pub(crate) fn new(lease: Lease) -> Self {
        Self { _lease: lease }
    }
}

/// Every connected agent.
#[derive(Debug, Default)]
pub struct Registry {
    by_key: DashMap<String, Arc<Client>>,
    next_id: AtomicUsize,
    pub store: Arc<MeterStore>,
    users: std::collections::HashMap<String, ResourceLimits>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_store(
        store: Arc<MeterStore>,
        users: std::collections::HashMap<String, ResourceLimits>,
    ) -> Self {
        Self {
            store,
            users,
            ..Self::default()
        }
    }

    /// Registers a newly authenticated agent, returning its entry.
    ///
    /// A second connection under the same `key_id` replaces the first: an agent
    /// that reconnected after a network drop would otherwise be locked out by
    /// its own stale entry until the old one timed out.
    pub fn insert(&self, key_id: &str, entry: &ClientEntry, tunnel: TunnelHandle) -> Arc<Client> {
        let acl = Acl::parse(entry.acl_mode, &entry.acl_rules).unwrap_or_else(|_| {
            // Validated at config load; a failure here means the config changed
            // underneath us, and refusing all traffic is safer than allowing all.
            tracing::error!(
                client = key_id,
                "unparsable ACL at registration; denying traffic"
            );
            Acl::deny_all()
        });
        self.insert_with_acls(key_id, entry, tunnel, Arc::new(vec![acl]))
    }
    pub fn insert_with_acls(
        &self,
        key_id: &str,
        entry: &ClientEntry,
        tunnel: TunnelHandle,
        source_acls: Arc<Vec<Acl>>,
    ) -> Arc<Client> {
        let id = ClientId::new(self.next_id.fetch_add(1, Ordering::Relaxed) as u32 + 1);
        let meter = self.store.meter(
            ResourceKey::Client {
                name: key_id.into(),
            },
            ResourceLimits {
                quota: entry.quota,
                rate_limit: entry.rate_limit,
                max_connections: entry.max_connections,
            },
        );
        let mut resources = MeterGroup(vec![meter.clone()]);
        if let Some(name) = &entry.user {
            let limits = self.users.get(name).copied().unwrap_or(ResourceLimits {
                max_connections: Some(0),
                ..Default::default()
            });
            resources.0.push(
                self.store
                    .meter(ResourceKey::User { name: name.clone() }, limits),
            );
        }
        let client = Arc::new(Client {
            route_cancels: Default::default(),
            live_config: std::sync::atomic::AtomicBool::new(false),
            config_state: Default::default(),
            secure: std::sync::atomic::AtomicBool::new(false),
            user: entry.user.clone(),
            max_tunnels: entry.max_tunnels,
            vkey: entry.vkey.clone(),
            id,
            key_id: key_id.to_owned(),
            tunnel,
            traffic: meter.traffic.clone(),
            rate: meter.rate.clone(),
            quota: entry.quota,
            source_acls,
            max_connections: entry.max_connections,
            resources,
            store: self.store.clone(),
            allow_config_push: entry.allow_config_push,
            cancel: tokio_util::sync::CancellationToken::new(),
        });

        if let Some(old) = self.by_key.insert(key_id.to_owned(), Arc::clone(&client)) {
            tracing::info!(client = key_id, "replacing an earlier connection");
            old.cancel.cancel();
            old.tunnel.close(felucca_mux::CloseReason::Shutdown);
        }
        client
    }

    /// Looks up a connected agent.
    pub fn get(&self, key_id: &str) -> Option<Arc<Client>> {
        self.by_key.get(key_id).map(|e| Arc::clone(e.value()))
    }

    /// Removes an agent, but only if `client` is still the current entry.
    ///
    /// Comparing identity matters: a reconnect installs a new entry, and a late
    /// cleanup from the old connection must not evict it.
    pub fn remove_if_current(&self, client: &Arc<Client>) -> bool {
        self.by_key
            .remove_if(&client.key_id, |_, current| Arc::ptr_eq(current, client))
            .is_some()
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Every connected agent, for the dashboard and for shutdown.
    pub fn clients(&self) -> Vec<Arc<Client>> {
        self.by_key.iter().map(|e| Arc::clone(e.value())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use felucca_core::Secret;

    fn entry() -> ClientEntry {
        ClientEntry {
            enabled: true,
            user: None,
            vkey: Secret::new("k"),
            remark: String::new(),
            quota: Quota::UNLIMITED,
            rate_limit: None,
            max_connections: None,
            max_tunnels: None,
            acl_mode: felucca_core::AclMode::Off,
            acl_rules: String::new(),
            allow_config_push: false,
        }
    }

    /// A handle to a real but unused mux session, so tests need no network.
    fn handle() -> TunnelHandle {
        let (a, _b) = tokio::io::duplex(1024);
        let tunnel = felucca_transport::Tunnel::over_stream(
            a,
            felucca_mux::Role::Server,
            felucca_mux::Config::default(),
        );
        let h = tunnel.handle();
        std::mem::forget(tunnel);
        h
    }

    #[tokio::test]
    async fn a_registered_client_can_be_found() {
        let reg = Registry::new();
        let c = reg.insert("agent-1", &entry(), handle());
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get("agent-1").unwrap().id, c.id);
        assert!(reg.get("agent-2").is_none());
    }

    #[tokio::test]
    async fn client_ids_are_distinct() {
        let reg = Registry::new();
        let a = reg.insert("agent-1", &entry(), handle());
        let b = reg.insert("agent-2", &entry(), handle());
        assert_ne!(a.id, b.id);
    }

    #[tokio::test]
    async fn a_reconnect_replaces_the_earlier_entry() {
        // Otherwise an agent whose connection dropped is locked out by its own
        // stale entry until that times out.
        let reg = Registry::new();
        let first = reg.insert("agent-1", &entry(), handle());
        first.traffic.add_inlet(500);
        first.traffic.add_export(300);
        let second = reg.insert("agent-1", &entry(), handle());

        assert_eq!(reg.len(), 1, "the same agent must not occupy two slots");
        assert_eq!(reg.get("agent-1").unwrap().id, second.id);
        assert_ne!(first.id, second.id);
        assert_eq!(
            second.traffic.total(),
            800,
            "reconnecting must not reset usage"
        );
        assert!(Arc::ptr_eq(&first.rate, &second.rate));
        assert!(first.cancel.is_cancelled());
    }

    #[tokio::test]
    async fn a_late_cleanup_does_not_evict_a_reconnected_client() {
        let reg = Registry::new();
        let first = reg.insert("agent-1", &entry(), handle());
        let second = reg.insert("agent-1", &entry(), handle());

        // The old connection's cleanup runs after the reconnect.
        assert!(
            !reg.remove_if_current(&first),
            "should not have removed anything"
        );
        assert_eq!(
            reg.get("agent-1").unwrap().id,
            second.id,
            "the live entry survives"
        );

        assert!(reg.remove_if_current(&second));
        assert!(reg.is_empty());
    }

    #[tokio::test]
    async fn the_connection_cap_is_enforced_exactly() {
        let reg = Registry::new();
        let c = reg.insert(
            "agent-1",
            &ClientEntry {
                max_connections: Some(2),
                ..entry()
            },
            handle(),
        );

        let a = c.try_acquire().expect("first");
        let b = c.try_acquire().expect("second");
        assert_eq!(c.live_connections(), 2);
        assert!(c.try_acquire().is_none(), "the cap must not be exceeded");

        drop(a);
        assert_eq!(c.live_connections(), 1);
        assert!(c.try_acquire().is_some(), "a freed slot is reusable");
        drop(b);
    }

    #[tokio::test]
    async fn a_cap_of_one_admits_exactly_one() {
        // fetch_add-then-check would briefly allow two here.
        let reg = Registry::new();
        let c = reg.insert(
            "agent-1",
            &ClientEntry {
                max_connections: Some(1),
                ..entry()
            },
            handle(),
        );
        let _held = c.try_acquire().expect("first");
        assert!(c.try_acquire().is_none());
    }

    #[tokio::test]
    async fn a_slot_is_returned_even_when_a_task_unwinds() {
        let reg = Registry::new();
        let c = reg.insert(
            "agent-1",
            &ClientEntry {
                max_connections: Some(1),
                ..entry()
            },
            handle(),
        );

        let client = Arc::clone(&c);
        let task = tokio::spawn(async move {
            let _guard = client.try_acquire().expect("should acquire");
            panic!("a forwarding task failing mid-copy");
        });
        assert!(task.await.is_err(), "the task should have panicked");

        assert_eq!(c.live_connections(), 0, "the slot must not leak");
        assert!(c.try_acquire().is_some());
    }

    #[tokio::test]
    async fn no_cap_means_no_limit() {
        let reg = Registry::new();
        let c = reg.insert("agent-1", &entry(), handle());
        let guards: Vec<_> = (0..1000)
            .map(|_| c.try_acquire().expect("uncapped"))
            .collect();
        assert_eq!(c.live_connections(), 1000);
        drop(guards);
        assert_eq!(c.live_connections(), 0);
    }

    #[tokio::test]
    async fn an_unlimited_quota_always_permits() {
        let reg = Registry::new();
        let c = reg.insert("agent-1", &entry(), handle());
        c.traffic.add_inlet(u64::MAX / 2);
        assert!(c.quota_state(i64::MAX).is_ok());
    }

    #[tokio::test]
    async fn an_exhausted_quota_stops_permitting() {
        let reg = Registry::new();
        let quota = Quota {
            flow_limit: Some(1000),
            expires_at: None,
        };
        let c = reg.insert("agent-1", &ClientEntry { quota, ..entry() }, handle());

        c.traffic.add_inlet(999);
        assert!(c.quota_state(0).is_ok());
        c.traffic.add_export(1);
        assert_eq!(c.quota_state(0), QuotaState::FlowExhausted);
    }

    #[tokio::test]
    async fn listing_clients_sees_every_entry() {
        let reg = Registry::new();
        reg.insert("a", &entry(), handle());
        reg.insert("b", &entry(), handle());
        let mut ids: Vec<_> = reg.clients().iter().map(|c| c.key_id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["a", "b"]);
    }
}
