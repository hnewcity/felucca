//! Shared resource limits and atomic traffic snapshots across daemon restarts.
use dashmap::DashMap;
use felucca_core::{
    RateLimiter, TrafficCounter,
    traffic::{ResourceLimits, TrafficStats},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceKey {
    User { name: String },
    Client { name: String },
    Tunnel { client: String, id: u32 },
    Host { pattern: String },
}

#[derive(Debug)]
pub struct Meter {
    pub limits: ResourceLimits,
    pub traffic: Arc<TrafficCounter>,
    pub rate: Arc<RateLimiter>,
    live: Arc<AtomicUsize>,
}
impl Meter {
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }
    fn permits(&self) -> bool {
        self.limits
            .quota
            .check(self.traffic.total(), crate::relay::now())
            .is_ok()
    }
    fn acquire(&self) -> bool {
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |live| {
                if self.limits.max_connections.is_some_and(|max| live >= max) {
                    None
                } else {
                    live.checked_add(1)
                }
            })
            .is_ok()
    }
}

#[derive(Debug, Clone, Default)]
pub struct MeterGroup(pub Vec<Arc<Meter>>);
impl MeterGroup {
    pub fn permits(&self) -> bool {
        self.0.iter().all(|meter| meter.permits())
    }
    pub fn acquire(&self) -> Option<Lease> {
        if !self.permits() {
            return None;
        }
        let mut lease = Lease(Vec::new());
        for meter in &self.0 {
            if !meter.acquire() {
                return None;
            }
            lease.0.push(meter.clone());
        }
        Some(lease)
    }
    pub fn delay(&self, bytes: u64) -> Option<Duration> {
        // Each bucket is charged once, and the strictest schedule wins.
        // Adding delays would incorrectly multiply the imposed rate limit.
        self.0
            .iter()
            .filter_map(|meter| meter.rate.take(bytes))
            .max()
    }
    pub async fn pace(&self, bytes: u64) -> io::Result<()> {
        if !self.permits() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "quota exhausted",
            ));
        }
        if let Some(delay) = self.delay(bytes) {
            tokio::time::sleep(delay).await;
        }
        if !self.permits() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "quota exhausted",
            ));
        }
        Ok(())
    }
    pub fn record(&self, direction: crate::copy::Direction, bytes: u64) {
        for meter in &self.0 {
            match direction {
                crate::copy::Direction::Inlet => meter.traffic.add_inlet(bytes),
                crate::copy::Direction::Export => meter.traffic.add_export(bytes),
            }
        }
    }
}
#[derive(Debug)]
pub struct Lease(Vec<Arc<Meter>>);
impl Drop for Lease {
    fn drop(&mut self) {
        for meter in &self.0 {
            meter.live.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    counters: Vec<(ResourceKey, TrafficStats)>,
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    imports: VecDeque<TrafficImport>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrafficImport {
    actor: String,
    key: String,
    digest: [u8; 32],
}
#[derive(Debug)]
struct Persistence {
    path: PathBuf,
    _lock: File,
    writer: Mutex<()>,
}
const MAX_SNAPSHOT: u64 = 64 * 1024 * 1024;
#[derive(Debug, Default)]
pub struct MeterStore {
    meters: DashMap<ResourceKey, Arc<Meter>>,
    persistence: Option<Persistence>,
    imports: Mutex<VecDeque<TrafficImport>>,
}
impl MeterStore {
    pub fn open(config: Option<&felucca_core::config::server::StateConfig>) -> io::Result<Self> {
        let Some(config) = config else {
            return Ok(Self::default());
        };
        let parent = parent(&config.path);
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
        let mut lock_path = config.path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock = secure_options()
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "runtime state is already in use by another server",
            )
        })?;
        remove_stale_temporaries(&config.path);
        let mut store = Self::default();
        let snapshot = match read_snapshot(&config.path) {
            Ok(snapshot) => snapshot,
            // Counters and import history are not worth a server that stays
            // down: use the copy from the previous save, or start empty.
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let kept = crate::control::store::quarantine(&config.path)?;
                match read_snapshot(&previous(&config.path)) {
                    Ok(Some(snapshot)) => {
                        tracing::error!(%error, kept = %kept.display(), "traffic state is unreadable; using the snapshot saved before it");
                        Some(snapshot)
                    }
                    _ => {
                        tracing::error!(%error, kept = %kept.display(), "traffic state is unreadable; starting with empty traffic counters");
                        None
                    }
                }
            }
            Err(error) => return Err(error),
        };
        if let Some(snapshot) = snapshot {
            *store.imports.get_mut() = snapshot.imports;
            for (key, traffic) in snapshot.counters {
                let meter = Arc::new(Meter {
                    limits: ResourceLimits::default(),
                    traffic: Arc::new(TrafficCounter::from_stats(traffic)),
                    rate: Arc::new(RateLimiter::new(0)),
                    live: Arc::new(AtomicUsize::new(0)),
                });
                store.meters.insert(key, meter);
            }
        }
        store.persistence = Some(Persistence {
            path: config.path.clone(),
            _lock: lock,
            writer: Mutex::new(()),
        });
        Ok(store)
    }
    pub fn meter(&self, key: ResourceKey, limits: ResourceLimits) -> Arc<Meter> {
        let mut entry = self.meters.entry(key).or_insert_with(|| {
            Arc::new(Meter {
                limits,
                traffic: Arc::new(TrafficCounter::new()),
                rate: Arc::new(RateLimiter::new(limits.rate_limit.unwrap_or(0))),
                live: Arc::new(AtomicUsize::new(0)),
            })
        });
        // Preserve usage and live reservations when a reconnect changes limits.
        if entry.limits != limits {
            let replacement = Arc::new(Meter {
                limits,
                traffic: entry.traffic.clone(),
                live: entry.live.clone(),
                rate: if entry.limits.rate_limit == limits.rate_limit {
                    entry.rate.clone()
                } else {
                    Arc::new(RateLimiter::new(limits.rate_limit.unwrap_or(0)))
                },
            });
            *entry = replacement;
        }
        entry.clone()
    }
    pub fn stats(&self, key: &ResourceKey) -> Option<TrafficStats> {
        self.meters.get(key).map(|meter| meter.traffic.stats())
    }
    /// Drops tunnel and host meters that never carried a byte and that no
    /// registered route holds. An agent allowed to push configuration names
    /// its own tunnel ids and hosts, and a meter is created before the route
    /// is validated, so without this every rejected or short-lived id would be
    /// kept and persisted for the life of the state file. Absent and zero mean
    /// the same usage, so quotas are unaffected.
    fn prune_unused(&self) {
        self.meters.retain(|key, meter| {
            matches!(key, ResourceKey::User { .. } | ResourceKey::Client { .. })
                || meter.traffic.total() != 0
                || meter.live() != 0
                || Arc::strong_count(meter) > 1
        });
    }
    pub fn has_live_connections(&self) -> bool {
        self.meters.iter().any(|meter| meter.live() != 0)
    }
    pub fn save(&self) -> io::Result<()> {
        let Some(persistence) = &self.persistence else {
            return Ok(());
        };
        let _guard = persistence.writer.lock();
        self.prune_unused();
        let mut counters: Vec<_> = self
            .meters
            .iter()
            .map(|entry| (entry.key().clone(), entry.traffic.stats()))
            .collect();
        counters.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let imports = self.imports.lock().clone();
        persistence.persist(&Snapshot {
            version: if imports.is_empty() { 1 } else { 2 },
            counters,
            imports,
        })
    }
    /// Counters and the retry marker commit in one file before live atomics change.
    /// A crash before publication restores the committed totals on the next boot;
    /// ordinary data-path increments remain lock-free and are never overwritten.
    pub(crate) fn import(
        &self,
        actor: String,
        key: String,
        digest: [u8; 32],
        deltas: BTreeMap<ResourceKey, (ResourceLimits, TrafficStats)>,
    ) -> io::Result<bool> {
        let persistence = self.persistence.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "traffic import requires state.path",
            )
        })?;
        let _writer = persistence.writer.lock();
        let mut history = self.imports.lock();
        if let Some(previous) = history.iter().find(|i| i.actor == actor && i.key == key) {
            if previous.digest != digest {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "idempotency key names another traffic import",
                ));
            }
            return Ok(true);
        }
        if actor.is_empty()
            || actor.len() > 128
            || key.is_empty()
            || key.len() > 128
            || deltas.is_empty()
            || deltas.len() > 256
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut counters: BTreeMap<_, _> = self
            .meters
            .iter()
            .map(|entry| (entry.key().clone(), entry.traffic.stats()))
            .collect();
        for (resource, (_, delta)) in &deltas {
            let total = counters.entry(resource.clone()).or_default();
            total.inlet = total.inlet.saturating_add(delta.inlet);
            total.export = total.export.saturating_add(delta.export);
        }
        let mut imports = history.clone();
        imports.push_back(TrafficImport { actor, key, digest });
        while imports.len() > 256 {
            imports.pop_front();
        }
        persistence.persist(&Snapshot {
            version: 2,
            counters: counters.into_iter().collect(),
            imports: imports.clone(),
        })?;
        for (resource, (limits, delta)) in deltas {
            let meter = self.meter(resource, limits);
            meter.traffic.add_inlet(delta.inlet);
            meter.traffic.add_export(delta.export);
        }
        *history = imports;
        Ok(false)
    }
}
impl Persistence {
    /// Callers hold writer. After rename, publication is committed even if the
    /// directory fsync fails; returning failure then would permit double import.
    fn persist(&self, snapshot: &Snapshot) -> io::Result<()> {
        let bytes = serde_json::to_vec(snapshot).map_err(invalid)?;
        if bytes.len() as u64 > MAX_SNAPSHOT {
            return Err(invalid("state exceeds 64 MiB"));
        }
        let temp = temporary(&self.path);
        let result = (|| {
            let mut file = secure_options().create_new(true).open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            // Keep the snapshot being replaced: a hard link to its inode costs
            // no space or copy, and the rename below leaves that inode intact.
            let kept = temporary(&self.path);
            if std::fs::hard_link(&self.path, &kept).is_ok()
                && std::fs::rename(&kept, previous(&self.path)).is_err()
            {
                let _ = std::fs::remove_file(&kept);
            }
            std::fs::rename(&temp, &self.path)?;
            #[cfg(unix)]
            if let Err(error) =
                File::open(parent(&self.path)).and_then(|directory| directory.sync_all())
            {
                tracing::error!(%error, "traffic state committed but directory sync failed");
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
}
#[derive(Deserialize)]
struct Version {
    version: u32,
}
fn read_snapshot(path: &Path) -> io::Result<Option<Snapshot>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(invalid("state must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SNAPSHOT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SNAPSHOT {
        return Err(invalid("state exceeds 64 MiB"));
    }
    // A version this binary does not know is most likely a downgrade, with an
    // operator present: fail closed and keep the data. Anything else that does
    // not parse is damage, which the caller recovers from.
    if serde_json::from_slice::<Version>(&bytes)
        .is_ok_and(|header| !matches!(header.version, 1 | 2))
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported state version",
        ));
    }
    let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
    let mut identities = std::collections::HashSet::new();
    if snapshot.imports.len() > 256
        || snapshot.imports.iter().any(|item| {
            item.actor.is_empty()
                || item.actor.len() > 128
                || item.key.is_empty()
                || item.key.len() > 128
                || !identities.insert((&item.actor, &item.key))
        })
    {
        return Err(invalid("invalid traffic import history"));
    }
    let mut keys = std::collections::HashSet::new();
    if !snapshot.counters.iter().all(|(key, _)| keys.insert(key)) {
        return Err(invalid("duplicate state resource"));
    }
    Ok(Some(snapshot))
}
/// Temporary names include the state file name, so stores sharing a directory
/// never remove each other's in-flight writes.
fn temporary(path: &Path) -> std::path::PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    parent(path).join(format!(
        ".felucca-state-{name}-{}.tmp",
        uuid::Uuid::new_v4()
    ))
}
/// Removes temporary files left by a crash between write and rename; the
/// caller holds the state lock, so none of them belongs to a live writer.
fn remove_stale_temporaries(path: &Path) {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let prefix = format!(".felucca-state-{name}-");
    let Ok(entries) = std::fs::read_dir(parent(path)) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        if file.starts_with(&prefix) && file.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
/// The snapshot replaced by the latest save, kept as a fallback.
fn previous(path: &Path) -> std::path::PathBuf {
    let mut copy = path.as_os_str().to_owned();
    copy.push(".previous");
    copy.into()
}
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
fn secure_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn invalid(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
