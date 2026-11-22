//! External platform authority and durable event/control connectors.
mod api;
mod callback;
mod changes;
pub(crate) mod direct;
mod operations;
mod reports;
mod store;
mod subscriptions;
mod ws;
use crate::admin::{
    Admin,
    auth::{Authenticated, Principal},
    credentials,
};
use felucca_core::{
    ServerConfig,
    config::{
        AdminRole,
        node::{CallbackConfig, NodeConfig, PlatformConfig, PlatformScope},
    },
};
use parking_lot::RwLock;
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Weak},
    time::Duration,
};
pub(crate) use store::Store;
use subtle::ConstantTimeEq;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, Clone)]
pub(crate) struct ReverseIdentity {
    pub id: String,
    pub generation: [u8; 32],
}
pub(super) fn generation(platform: &PlatformConfig) -> [u8; 32] {
    credentials::digest(&serde_json::to_vec(platform).expect("platform config serializes"))
}
pub(super) fn principal(id: &str, config: &PlatformConfig) -> Principal {
    Principal {
        account: format!("@platform/{id}"),
        role: if config.scope == PlatformScope::Full {
            AdminRole::Admin
        } else {
            AdminRole::Operator
        },
        user: config.user.clone(),
    }
}
pub(crate) fn authenticate(
    config: Option<&NodeConfig>,
    headers: &http::HeaderMap,
    reverse: Option<&ReverseIdentity>,
) -> Option<Authenticated> {
    let config = config?;
    if headers.get_all(http::header::AUTHORIZATION).iter().count() != 1 {
        return None;
    }
    let token = headers
        .get(http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    if !(16..=4096).contains(&token.len()) {
        return None;
    }
    let hash = credentials::digest(token.as_bytes());
    let mut found = None;
    for (id, platform) in &config.platforms {
        let valid: bool = credentials::digest(platform.token.as_bytes())
            .ct_eq(&hash)
            .into();
        let channel = match reverse {
            None => platform.mode.direct(),
            Some(reverse) => {
                platform.mode.reverse()
                    && reverse.id == *id
                    && reverse.generation == generation(platform)
            }
        };
        if valid && channel && platform.enabled {
            found = Some(Authenticated {
                principal: principal(id, platform),
                csrf: None,
            });
        }
    }
    found
}
pub(crate) fn prepare(config: &ServerConfig) -> io::Result<Option<Arc<Store>>> {
    config
        .node
        .as_ref()
        .map(|node| Store::open(&node.state_path))
        .transpose()
}
pub(crate) fn compatible(old: Option<&NodeConfig>, new: Option<&NodeConfig>) -> bool {
    old.map(|n| (&n.id, &n.state_path)) == new.map(|n| (&n.id, &n.state_path))
}
#[derive(Debug, Clone)]
struct Target {
    id: String,
    user: Option<String>,
    platform: Option<String>,
    enabled: bool,
    delivery: CallbackConfig,
}
impl Target {
    fn generation(&self) -> [u8; 32] {
        let mut bytes = serde_json::to_vec(&(
            &self.delivery.url,
            &self.delivery.signing_key,
            &self.user,
            &self.platform,
        ))
        .expect("target identity");
        if self.delivery.selector != Default::default()
            || self.delivery.content != Default::default()
        {
            bytes.extend(
                serde_json::to_vec(&(&self.delivery.selector, &self.delivery.content))
                    .expect("event sink"),
            );
        }
        credentials::digest(&bytes)
    }
    fn visible(&self, principal: &Principal) -> bool {
        principal.role == AdminRole::Admin
            || (self.user.is_some()
                && self.user.as_ref() == principal.user.as_ref()
                && principal
                    .account
                    .strip_prefix("@platform/")
                    .is_none_or(|id| self.platform.as_deref().is_none_or(|owner| owner == id)))
    }
}
fn targets(config: &NodeConfig) -> BTreeMap<String, Target> {
    let mut result = BTreeMap::new();
    for (id, platform) in &config.platforms {
        if let Some(delivery) = &platform.callback {
            let id_key = format!("platform/{id}");
            result.insert(
                id_key.clone(),
                Target {
                    id: id_key,
                    user: platform.user.clone(),
                    platform: Some(id.clone()),
                    enabled: platform.enabled && delivery.enabled,
                    delivery: delivery.clone(),
                },
            );
        }
    }
    for (id, webhook) in &config.webhooks {
        let id = format!("webhook/{id}");
        result.insert(
            id.clone(),
            Target {
                id,
                user: webhook.user.clone(),
                platform: None,
                enabled: webhook.delivery.enabled,
                delivery: webhook.delivery.clone(),
            },
        );
    }
    result
}
#[derive(Debug, Clone, Default, serde::Serialize)]
struct ReverseStatus {
    connected: bool,
    attempts: u64,
    requests: u64,
    responses: u64,
    last_connected_ms: Option<u64>,
    last_pong_ms: Option<u64>,
    last_error: Option<String>,
}
#[derive(Debug)]
struct ReverseRuntime {
    generation: [u8; 32],
    status: ReverseStatus,
}
pub(crate) struct Node {
    admin: Weak<Admin>,
    daemon: Weak<crate::Daemon>,
    management: Arc<crate::control::Management>,
    id: String,
    boot: uuid::Uuid,
    started: u64,
    store: Arc<Store>,
    cancel: CancellationToken,
    tasks: TaskTracker,
    wake: tokio::sync::Notify,
    events: tokio::sync::broadcast::Sender<store::Event>,
    reverse: RwLock<BTreeMap<String, ReverseRuntime>>,
    journal_error: RwLock<Option<String>>,
    rpc_slots: Arc<tokio::sync::Semaphore>,
    ws_slots: Arc<tokio::sync::Semaphore>,
    reports: tokio::sync::broadcast::Sender<reports::Report>,
    report_progress: parking_lot::Mutex<BTreeMap<String, (tokio::time::Instant, u64)>>,
}
impl Node {
    fn notify_live(&self, kind: &str, user: Option<&str>, target: Option<&str>) {
        let Ok((revision, _)) = self.config() else {
            return;
        };
        let (resource, action) = kind.split_once('.').unwrap_or(("operation", "updated"));
        let _ = self.events.send(store::Event {
            sequence: 0,
            revision,
            timestamp_ms: store::now(),
            kind: kind.into(),
            ephemeral: true,
            resource: Some(resource.into()),
            action: Some(action.into()),
            user: user.map(str::to_owned),
            target: target.map(str::to_owned),
            client: None,
            tunnel: None,
            host: None,
        });
    }
    pub(crate) fn start(admin: &Arc<Admin>, store: Arc<Store>) -> Arc<Self> {
        let daemon = admin.daemon.upgrade().expect("starting daemon");
        let config = daemon.config().1;
        let node = Arc::new(Self {
            admin: Arc::downgrade(admin),
            daemon: admin.daemon.clone(),
            management: admin.management.clone(),
            id: config.node.as_ref().expect("prepared node").id.clone(),
            boot: uuid::Uuid::new_v4(),
            started: store::now(),
            store,
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
            wake: tokio::sync::Notify::new(),
            events: tokio::sync::broadcast::channel(256).0,
            reverse: Default::default(),
            journal_error: Default::default(),
            rpc_slots: Arc::new(tokio::sync::Semaphore::new(16)),
            ws_slots: Arc::new(tokio::sync::Semaphore::new(16)),
            reports: tokio::sync::broadcast::channel(128).0,
            report_progress: Default::default(),
        });
        node.tasks.spawn(node.clone().manage());
        node
    }
    fn config(&self) -> io::Result<(u64, NodeConfig)> {
        let (revision, config) = self.view()?;
        Ok((revision, config.node.ok_or(io::ErrorKind::NotConnected)?))
    }
    fn view(&self) -> io::Result<(u64, ServerConfig)> {
        let daemon = self.daemon.upgrade().ok_or(io::ErrorKind::NotConnected)?;
        Ok(daemon.config())
    }
    pub(crate) fn stop(&self) {
        self.cancel.cancel();
        self.tasks.close();
    }
    pub(crate) async fn stopped(&self) {
        self.tasks.wait().await;
        self.store.shutdown().await;
    }
    fn note(&self, id: &str, generation: [u8; 32], edit: impl FnOnce(&mut ReverseStatus)) {
        if let Some(runtime) = self
            .reverse
            .write()
            .get_mut(id)
            .filter(|r| r.generation == generation)
        {
            edit(&mut runtime.status);
        }
    }
    async fn manage(self: Arc<Self>) {
        let Some(daemon) = self.daemon.upgrade() else {
            return;
        };
        let mut changes = daemon.subscribe();
        drop(daemon);
        let mut workers = tokio::task::JoinSet::new();
        let mut previous = None;
        let mut seen_revision = None;
        let mut seen_config: Option<ServerConfig> = None;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let Ok((revision, snapshot)) = self.view() else {
                return;
            };
            let Some(config) = snapshot.node.clone() else {
                return;
            };
            let digest = credentials::digest(&serde_json::to_vec(&config).expect("node config"));
            let changed = previous != Some(digest);
            if changed || seen_revision != Some(revision) {
                let initial = seen_revision.is_none();
                let changes = seen_config
                    .as_ref()
                    .map_or_else(Vec::new, |old| changes::between(old, &snapshot));
                match self.reconcile(revision, &config, initial, changes).await {
                    Ok(events) => {
                        *self.journal_error.write() = None;
                        for event in events {
                            let _ = self.events.send(event);
                        }
                        self.notify_live("operations.updated", None, None);
                        if changed {
                            self.notify_live("management_platforms.updated", None, None);
                        }
                        seen_config = Some(snapshot);
                        self.wake.notify_waiters();
                        seen_revision = Some(revision);
                        if changed {
                            workers.abort_all();
                            while workers.join_next().await.is_some() {}
                            self.reverse
                                .write()
                                .retain(|id, _| config.platforms.contains_key(id));
                            for (id, platform) in &config.platforms {
                                let generation = generation(platform);
                                self.reverse.write().insert(
                                    id.clone(),
                                    ReverseRuntime {
                                        generation,
                                        status: Default::default(),
                                    },
                                );
                                if platform.enabled && platform.mode.reverse() {
                                    workers.spawn(ws::run(
                                        self.clone(),
                                        id.clone(),
                                        platform.clone(),
                                        generation,
                                    ));
                                }
                            }
                            for target in targets(&config).into_values().filter(|t| t.enabled) {
                                workers.spawn(callback::reports(self.clone(), target.clone()));
                                workers.spawn(callback::run(self.clone(), target));
                            }
                            previous = Some(digest);
                        }
                    }
                    Err(error) => {
                        *self.journal_error.write() = Some("journal update failed".into());
                        tracing::error!(%error,"node journal update failed");
                    }
                }
            }
            tokio::select! {
                () = self.cancel.cancelled() => return,
                _ = tick.tick() => {},
                _ = changes.recv() => {},
                _ = workers.join_next(), if !workers.is_empty() => {},
            }
        }
    }
    async fn reconcile(
        &self,
        revision: u64,
        config: &NodeConfig,
        initial: bool,
        changes: Vec<changes::Change>,
    ) -> io::Result<Vec<store::Event>> {
        let targets = targets(config);
        let window = config.changes_window;
        self.store
            .update(move |state| {
                state.queues.retain(|id, _| targets.contains_key(id));
                for target in targets.values() {
                    let generation = target.generation();
                    let queue =
                        state
                            .queues
                            .entry(target.id.clone())
                            .or_insert_with(|| store::Queue {
                                generation,
                                user: target.user.clone(),
                                platform: target.platform.clone(),
                                pending: Default::default(),
                                counters: Default::default(),
                            });
                    if queue.generation != generation {
                        queue.counters.dropped = queue
                            .counters
                            .dropped
                            .saturating_add(queue.pending.len() as u64);
                        queue.pending.clear();
                        queue.generation = generation;
                        queue.user = target.user.clone();
                        queue.platform = target.platform.clone();
                    }
                }
                if !initial && state.last_revision == Some(revision) {
                    return Ok(Vec::new());
                }
                let kind = if initial {
                    "node.started"
                } else if state
                    .last_revision
                    .is_some_and(|old| revision > old.saturating_add(1))
                {
                    "config.resync"
                } else {
                    "config.changed"
                };
                let event = state.event(revision, kind, window)?;
                let mut events = vec![event];
                for change in changes {
                    let mut event = state.event(
                        revision,
                        &format!("{}.{}", change.resource, change.action),
                        window,
                    )?;
                    event.resource = Some(change.resource.into());
                    event.action = Some(change.action.into());
                    event.user = change.user;
                    event.client = change.client;
                    event.tunnel = change.tunnel;
                    event.host = change.host;
                    *state.history.back_mut().expect("appended event") = event.clone();
                    events.push(event);
                }
                for target in targets.values().filter(|target| target.enabled) {
                    let queue = state.queues.get_mut(&target.id).expect("created queue");
                    for event in &events {
                        if !event.visible(target.user.as_deref(), target.user.is_none())
                            || !target
                                .delivery
                                .selector
                                .matches(&serde_json::to_value(event).expect("event"))
                        {
                            continue;
                        }
                        queue.pending.push_back(store::Delivery {
                            id: uuid::Uuid::new_v4(),
                            event: event.clone(),
                            attempts: 0,
                            run: 0,
                            next_at_ms: store::now(),
                            failed: false,
                            last_status: None,
                            last_error: None,
                        });
                        // queue_max=0 disables dead-letter retention; active delivery still
                        // has a small bounded, durable backlog before the first attempt.
                        let capacity = if target.delivery.queue_max == 0 {
                            32
                        } else {
                            target.delivery.queue_max
                        };
                        while queue.pending.len() > capacity {
                            queue.pending.pop_front();
                            queue.counters.dropped = queue.counters.dropped.saturating_add(1);
                        }
                    }
                }
                state.bound_deliveries();
                Ok(events)
            })
            .await
    }
}
