//! Immutable policy snapshots; publishing one never replaces bridge listeners.
use super::*;
use felucca_core::{Acl, config::ClientEntry};

#[derive(Debug)]
pub(crate) struct LivePolicy {
    pub config: Arc<ServerConfig>,
    pub sources: HashMap<String, Arc<Vec<Acl>>>,
    pub shares: HashMap<String, (Acl, CancellationToken)>,
    pub managed_revisions: HashMap<String, [u8; 32]>,
}

pub(super) fn equal<T: serde::Serialize>(left: &T, right: &T) -> bool {
    serde_json::to_value(left).expect("configuration")
        == serde_json::to_value(right).expect("configuration")
}

fn same_identity(left: &ClientEntry, right: &ClientEntry) -> bool {
    let mut left = left.clone();
    left.remark.clone_from(&right.remark);
    equal(&left, right)
}

impl LivePolicy {
    pub fn prepare(
        config: Arc<ServerConfig>,
        previous: Option<&Self>,
    ) -> Result<Self, felucca_core::ConfigError> {
        let geo = felucca_core::geodata::GeoResolver::new(&config.geo)?;
        let parse = |mode, rules: &str| {
            Acl::parse_with_geo(mode, rules, &geo)
                .map_err(|e| felucca_core::ConfigError::invalid(e.to_string()))
        };
        let global = parse(config.acl_mode, &config.acl_rules)?;
        let mut sources = HashMap::new();
        for (id, client) in &config.clients {
            let mut scopes = vec![global.clone()];
            if let Some(user) = client.user.as_ref().and_then(|id| config.users.get(id)) {
                scopes.push(parse(user.acl_mode, &user.acl_rules)?);
            }
            scopes.push(parse(client.acl_mode, &client.acl_rules)?);
            sources.insert(id.clone(), Arc::new(scopes));
        }
        let mut shares = HashMap::new();
        for (id, share) in &config.shares {
            let cancel = previous
                .filter(|old| old.config.shares.get(id).is_some_and(|s| equal(s, share)))
                .and_then(|old| old.shares.get(id))
                .map(|(_, cancel)| cancel.clone())
                .unwrap_or_default();
            shares.insert(
                id.clone(),
                (parse(share.acl_mode, &share.acl_rules)?, cancel),
            );
        }
        let managed_revisions = config
            .managed
            .iter()
            .map(|(id, desired)| {
                desired
                    .encoded()
                    .map(|(_, revision)| (id.clone(), revision))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            config,
            sources,
            shares,
            managed_revisions,
        })
    }
}

impl Server {
    pub(crate) fn prepare_live(&self, config: ServerConfig) -> Result<LivePolicy, ServerError> {
        Ok(LivePolicy::prepare(
            Arc::new(config),
            Some(&self.live.load()),
        )?)
    }

    /// Called only after the durable commit. All fallible preparation precedes
    /// it. The gate prevents a handshake or old registration racing activation.
    pub(crate) async fn activate_live(&self, next: LivePolicy) {
        let _gate = self.policy_gate.write().await;
        let previous = self.live.load_full();
        let next = Arc::new(next);
        self.live.store(next.clone());
        for (id, (_, cancel)) in &previous.shares {
            if !next.config.shares.get(id).is_some_and(|share| {
                previous
                    .config
                    .shares
                    .get(id)
                    .is_some_and(|old| equal(old, share))
            }) {
                cancel.cancel();
            }
        }
        for client in self.registry.clients() {
            if client.key_id.starts_with("local.") {
                continue;
            }
            let authorized = next
                .config
                .clients
                .get(&client.key_id)
                .is_some_and(|entry| {
                    entry.enabled
                        && previous
                            .config
                            .clients
                            .get(&client.key_id)
                            .is_some_and(|old| same_identity(old, entry))
                });
            let changed = !equal(
                &previous.config.managed.get(&client.key_id),
                &next.config.managed.get(&client.key_id),
            );
            if !authorized
                || (changed
                    && !client
                        .live_config
                        .load(std::sync::atomic::Ordering::Acquire))
            {
                client.cancel.cancel();
                client.tunnel.close(felucca_mux::CloseReason::Shutdown);
                self.registrations.retire(&client).await;
            } else if changed {
                {
                    let mut state = client.config_state.write();
                    state.rejected.clear();
                    // A change can be reverted before the next agent poll.
                    // Even an identical digest must then restore withdrawn routes.
                    state.dirty = true;
                }
                // Withdraw disabled/changed public and private routes immediately.
                // Unchanged registrations keep their listeners and active streams.
                let specs = next
                    .config
                    .managed
                    .get(&client.key_id)
                    .map(|config| felucca_service::specs(&config.tunnels))
                    .unwrap_or_default();
                self.registrations.reconcile(&client, &specs).await;
            }
        }
    }
}
