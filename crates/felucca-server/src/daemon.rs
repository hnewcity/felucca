//! Serialized configuration replacement with rollback and durable revisions.
use crate::{
    Server,
    control::store::{ConfigStore, Loaded, Snapshot},
};
use felucca_core::ServerConfig;
use parking_lot::RwLock;
use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone)]
struct Active {
    snapshot: Snapshot,
    server: Arc<Server>,
    healthy: bool,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Applied {
    pub revision: u64,
    /// False without persistence, or if rename committed but the directory could not sync.
    pub durable: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRecord {
    pub key: String,
    pub account: String,
    pub user: Option<String>,
    pub digest: [u8; 32],
    pub revision: u64,
}

#[derive(Debug)]
pub struct Daemon {
    admin: RwLock<Option<Arc<crate::admin::Admin>>>,
    events: tokio::sync::broadcast::Sender<u64>,
    active: RwLock<Active>,
    store: Option<ConfigStore>,
    operations: tokio::sync::Mutex<()>,
    stopping: AtomicBool,
}

impl Daemon {
    pub(crate) fn configuration_path(&self) -> Option<&Path> {
        self.store.as_ref().map(ConfigStore::path)
    }
    pub fn load_configuration(
        bootstrap: ServerConfig,
        path: Option<&Path>,
    ) -> io::Result<ServerConfig> {
        let path = path.map(Path::to_path_buf).or_else(|| {
            bootstrap
                .admin
                .as_ref()
                .map(|admin| admin.state_path.clone())
        });
        Ok(path
            .as_deref()
            .map(ConfigStore::read_recoverable)
            .transpose()?
            .flatten()
            .map_or(bootstrap, |saved| saved.snapshot.config))
    }
    /// Existing saved configuration wins over the bootstrap file. An absent
    /// state file is initialized only after listeners successfully start.
    pub async fn start(mut config: ServerConfig, path: Option<&Path>) -> io::Result<Arc<Self>> {
        let path = path
            .map(Path::to_path_buf)
            .or_else(|| config.admin.as_ref().map(|admin| admin.state_path.clone()));
        let store = path.as_deref().map(ConfigStore::open).transpose()?;
        let mut quarantined = None;
        // Rewrite the file only when it does not already hold this snapshot.
        // An unconditional boot-time write turned a full disk into a server
        // that could not start.
        let (snapshot, persist) = match store.as_ref().map(ConfigStore::load).transpose()? {
            Some(Some(Loaded {
                snapshot,
                recovered: None,
            })) => (snapshot, false),
            Some(Some(Loaded {
                snapshot,
                recovered: Some(error),
            })) => {
                let store = store.as_ref().expect("loaded from a store");
                match store.quarantine() {
                    Ok(kept) => {
                        tracing::error!(%error, revision = snapshot.revision, kept = %kept.kept.display(),
                            "saved configuration is unreadable; starting from the last configuration that started");
                        quarantined = Some(kept);
                    }
                    Err(rename) => {
                        tracing::error!(%error, %rename, revision = snapshot.revision,
                            "saved configuration is unreadable and cannot be moved aside; starting from the last configuration that started");
                    }
                }
                (snapshot, true)
            }
            _ => {
                config.validate().map_err(io::Error::other)?;
                let snapshot = Snapshot {
                    version: 1,
                    revision: 0,
                    config,
                    operations: Vec::new(),
                };
                (snapshot, true)
            }
        };
        let started = Self::start_snapshot(snapshot, store, persist).await;
        if let Some(kept) = quarantined
            && started.is_err()
        {
            // Leave the state as found so the next start can recover again.
            kept.restore();
        }
        started
    }

    async fn start_snapshot(
        snapshot: Snapshot,
        store: Option<ConfigStore>,
        persist: bool,
    ) -> io::Result<Arc<Self>> {
        let node_journal = crate::platform::prepare(&snapshot.config)?;
        let prepared_admin = match &snapshot.config.admin {
            Some(config) => Some(crate::admin::Prepared::bind(config).await?),
            None => None,
        };
        let identity = if snapshot.config.bridge.tls.is_none() {
            store
                .as_ref()
                .map(|store| {
                    crate::certificates::identity::load_or_create(store.path()).map(Arc::new)
                })
                .transpose()?
        } else {
            None
        };
        if let Some(identity) = &identity {
            tracing::info!(fingerprint = %identity.fingerprint(), "using persistent bridge identity");
        }
        let server = Arc::new(
            Server::with_identity(snapshot.config.clone(), identity).map_err(io::Error::other)?,
        );
        let pending = store
            .as_ref()
            .filter(|_| persist)
            .map(|store| store.prepare(&snapshot))
            .transpose()?;
        if let Err(error) = server.clone().run().await {
            let _ = server.shutdown_and_wait().await;
            return Err(io::Error::other(error));
        }
        if let Some(pending) = pending
            && let Err(error) = pending.commit()
        {
            let _ = server.shutdown_and_wait().await;
            return Err(error);
        }
        if let Some(store) = &store {
            store.ensure_last_good();
        }
        let daemon = Arc::new(Self {
            admin: Default::default(),
            events: tokio::sync::broadcast::channel(256).0,
            active: RwLock::new(Active {
                snapshot,
                server,
                healthy: true,
            }),
            store,
            operations: Default::default(),
            stopping: AtomicBool::new(false),
        });
        if let Some(prepared) = prepared_admin {
            let admin = crate::admin::Admin::start(prepared, Arc::downgrade(&daemon), node_journal);
            daemon.server().attach_admin(Arc::downgrade(&admin));
            *daemon.admin.write() = Some(admin);
        }
        Ok(daemon)
    }
    pub(crate) fn view(&self) -> (u64, ServerConfig, Arc<Server>) {
        let active = self.active.read();
        (
            active.snapshot.revision,
            active.snapshot.config.clone(),
            active.server.clone(),
        )
    }
    pub fn server(&self) -> Arc<Server> {
        self.active.read().server.clone()
    }
    pub fn config(&self) -> (u64, ServerConfig) {
        let active = self.active.read();
        (active.snapshot.revision, active.snapshot.config.clone())
    }
    pub fn healthy(&self) -> bool {
        self.active.read().healthy && !self.stopping.load(Ordering::Acquire)
    }

    /// The transaction outlives an HTTP request dropping its response future.
    /// It must reach either activation or rollback after stopping listeners.
    pub async fn apply(
        self: &Arc<Self>,
        config: ServerConfig,
        expected_revision: u64,
    ) -> io::Result<Applied> {
        let daemon = self.clone();
        tokio::spawn(async move { daemon.apply_inner(config, expected_revision, None).await })
            .await
            .map_err(io::Error::other)?
    }
    pub async fn apply_operation(
        self: &Arc<Self>,
        config: ServerConfig,
        expected_revision: u64,
        operation: OperationRecord,
    ) -> io::Result<Applied> {
        let daemon = self.clone();
        tokio::spawn(async move {
            daemon
                .apply_inner(config, expected_revision, Some(operation))
                .await
        })
        .await
        .map_err(io::Error::other)?
    }
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<u64> {
        self.events.subscribe()
    }
    pub fn operation(&self, account: &str, key: &str) -> Option<OperationRecord> {
        self.active
            .read()
            .snapshot
            .operations
            .iter()
            .find(|op| op.account == account && op.key == key)
            .cloned()
    }
    pub(crate) async fn revision_guard(
        &self,
        expected: u64,
    ) -> io::Result<tokio::sync::MutexGuard<'_, ()>> {
        let guard = self.operations.lock().await;
        if self.stopping.load(Ordering::Acquire) || self.active.read().snapshot.revision != expected
        {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        Ok(guard)
    }
    async fn apply_inner(
        &self,
        mut config: ServerConfig,
        expected_revision: u64,
        operation: Option<OperationRecord>,
    ) -> io::Result<Applied> {
        let _operation = self.operations.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let old = self.active.read().clone();
        if !crate::platform::compatible(old.snapshot.config.node.as_ref(), config.node.as_ref()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "node id and journal path cannot change while running",
            ));
        }

        if let Some(operation) = &operation {
            if operation.key.is_empty()
                || operation.key.len() > 128
                || operation.account.len() > 128
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid operation identity",
                ));
            }
            if let Some(previous) = old
                .snapshot
                .operations
                .iter()
                .find(|op| op.account == operation.account && op.key == operation.key)
            {
                if previous.digest != operation.digest {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "idempotency key already names a different request",
                    ));
                }
                return Ok(Applied {
                    revision: previous.revision,
                    durable: self.store.is_some(),
                    replayed: true,
                });
            }
        }
        if expected_revision != old.snapshot.revision {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "configuration revision changed",
            ));
        }
        config.validate().map_err(io::Error::other)?;
        if !crate::admin::compatible(old.snapshot.config.admin.as_ref(), config.admin.as_ref()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "changing the admin listener, origin or state path requires a daemon restart",
            ));
        }
        let revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("configuration revision exhausted"))?;
        let mut operations = old.snapshot.operations.clone();
        if let Some(mut operation) = operation {
            operation.revision = revision;
            operations.push(operation);
            if operations.len() > 256 {
                operations.remove(0);
            }
        }
        let next = Snapshot {
            version: 1,
            revision,
            config,
            operations,
        };
        if crate::control::configuration::live_compatible(&old.snapshot.config, &next.config) {
            let prepared = old
                .server
                .prepare_live(next.config.clone())
                .map_err(io::Error::other)?;
            let pending = self
                .store
                .as_ref()
                .map(|store| store.prepare(&next))
                .transpose()?;
            let durable = match pending {
                Some(pending) => pending.commit()?,
                None => false,
            };
            old.server.activate_live(prepared).await;
            *self.active.write() = Active {
                snapshot: next,
                server: old.server,
                healthy: true,
            };
            let _ = self.events.send(revision);
            return Ok(Applied {
                revision,
                durable,
                replayed: false,
            });
        }
        // Prepare both generations and the durable file before disrupting any traffic.
        let replacement = Arc::new(
            old.server
                .replacement(next.config.clone())
                .map_err(io::Error::other)?,
        );
        let rollback = Arc::new(
            old.server
                .replacement(old.snapshot.config.clone())
                .map_err(io::Error::other)?,
        );
        if let Some(admin) = self.admin.read().as_ref() {
            replacement.attach_admin(Arc::downgrade(admin));
            rollback.attach_admin(Arc::downgrade(admin));
        }
        let pending = self
            .store
            .as_ref()
            .map(|store| store.prepare(&next))
            .transpose()?;
        self.active.write().healthy = false;
        if let Err(error) = old.server.shutdown_and_wait().await {
            let restored = rollback.clone().run().await;
            *self.active.write() = Active {
                snapshot: old.snapshot,
                server: rollback,
                healthy: restored.is_ok(),
            };
            return Err(io::Error::other(format!(
                "old generation did not drain: {error}; rollback: {restored:?}"
            )));
        }
        let result = match replacement.clone().run().await {
            Ok(_) => match pending {
                Some(pending) => pending.commit(),
                None => Ok(false),
            },
            Err(error) => Err(io::Error::other(error)),
        };
        match result {
            Ok(durable) => {
                *self.active.write() = Active {
                    snapshot: next,
                    server: replacement,
                    healthy: true,
                };
                let _ = self.events.send(revision);
                Ok(Applied {
                    revision,
                    durable,
                    replayed: false,
                })
            }
            Err(error) => {
                let _ = replacement.shutdown_and_wait().await;
                let restored = rollback.clone().run().await;
                *self.active.write() = Active {
                    snapshot: old.snapshot,
                    server: rollback,
                    healthy: restored.is_ok(),
                };
                if let Err(rollback_error) = restored {
                    return Err(io::Error::other(format!(
                        "activation failed: {error}; rollback also failed: {rollback_error}"
                    )));
                }
                Err(io::Error::other(format!(
                    "activation failed; previous configuration restored: {error}"
                )))
            }
        }
    }
    pub async fn shutdown(&self) -> io::Result<()> {
        self.stopping.store(true, Ordering::Release);
        let admin = self.admin.read().clone();
        if let Some(admin) = &admin {
            admin.stop();
        }
        let _operation = self.operations.lock().await;
        let server = self.server();
        self.active.write().healthy = false;
        let result = server.shutdown_and_wait().await;
        if let Some(admin) = admin {
            admin.stopped().await;
        }
        result
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        self.active.get_mut().server.shutdown();
        if let Some(admin) = self.admin.get_mut() {
            admin.stop();
        }
    }
}
