//! Bounded, atomic event history and callback outbox. No credentials are stored here.
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;
const MAX_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_DELIVERIES: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Event {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ephemeral: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub sequence: u64,
    pub revision: u64,
    pub timestamp_ms: u64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}
impl Event {
    pub fn visible(&self, user: Option<&str>, full: bool) -> bool {
        self.resource.is_none() || full || user.is_some() && self.user.as_deref() == user
    }
    fn valid(&self) -> bool {
        !self.ephemeral
            && self.kind.len() <= 64
            && self.target.as_ref().is_none_or(|s| s.len() <= 128)
            && self.resource.as_ref().is_none_or(|s| s.len() <= 32)
            && self.action.as_ref().is_none_or(|s| s.len() <= 32)
            && self.user.as_ref().is_none_or(|s| s.len() <= 128)
            && self.client.as_ref().is_none_or(|s| s.len() <= 128)
            && self.host.as_ref().is_none_or(|s| s.len() <= 253)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Delivery {
    pub id: Uuid,
    pub event: Event,
    pub attempts: u32,
    #[serde(default)]
    pub run: u64,
    pub next_at_ms: u64,
    pub failed: bool,
    pub last_status: Option<u16>,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Counters {
    pub delivered: u64,
    pub failed: u64,
    pub dropped: u64,
    pub replayed: u64,
    pub last_delivered_ms: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Queue {
    pub generation: [u8; 32],
    pub user: Option<String>,
    pub platform: Option<String>,
    pub pending: VecDeque<Delivery>,
    pub counters: Counters,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Action {
    pub actor: String,
    pub key: String,
    pub digest: [u8; 32],
    pub timestamp_ms: u64,
    pub affected: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    pub version: u16,
    pub epoch: Uuid,
    pub next: u64,
    pub last_revision: Option<u64>,
    pub history: VecDeque<Event>,
    pub queues: BTreeMap<String, Queue>,
    pub actions: VecDeque<Action>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            epoch: Uuid::new_v4(),
            next: 1,
            last_revision: None,
            history: VecDeque::new(),
            queues: BTreeMap::new(),
            actions: VecDeque::new(),
        }
    }
}
impl State {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || self.next == 0
            || self.history.len() > 4096
            || self.queues.len() > 80
            || self.actions.len() > 256
            || self.queues.values().map(|q| q.pending.len()).sum::<usize>() > MAX_DELIVERIES
        {
            return Err(invalid("node journal bounds/version are invalid"));
        }
        let mut previous = 0;
        for event in &self.history {
            if event.sequence <= previous || event.sequence >= self.next || !event.valid() {
                return Err(invalid("node event history is invalid"));
            }
            previous = event.sequence;
        }
        let mut ids = std::collections::HashSet::new();
        for (name, queue) in &self.queues {
            if name.len() > 128
                || queue.user.as_ref().is_some_and(|s| s.len() > 128)
                || queue.platform.as_ref().is_some_and(|s| s.len() > 64)
            {
                return Err(invalid("node queue identity is invalid"));
            }
            for delivery in &queue.pending {
                if !ids.insert(delivery.id)
                    || delivery.event.sequence == 0
                    || delivery.event.sequence >= self.next
                    || !delivery.event.valid()
                    || delivery.last_error.as_ref().is_some_and(|s| s.len() > 200)
                {
                    return Err(invalid("node delivery is invalid"));
                }
            }
        }
        if self.actions.iter().any(|a| {
            a.actor.len() > 128
                || a.key.is_empty()
                || a.key.len() > 128
                || a.affected > MAX_DELIVERIES
        }) {
            return Err(invalid("node action cache is invalid"));
        }
        Ok(())
    }
    pub fn event(&mut self, revision: u64, kind: &str, window: usize) -> io::Result<Event> {
        let event = Event {
            ephemeral: false,
            target: None,
            sequence: self.next,
            revision,
            timestamp_ms: now(),
            kind: kind.into(),
            resource: None,
            action: None,
            user: None,
            client: None,
            tunnel: None,
            host: None,
        };
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| invalid("event sequence exhausted"))?;
        self.last_revision = Some(revision);
        self.history.push_back(event.clone());
        while self.history.len() > window.clamp(100, 4096) {
            self.history.pop_front();
        }
        Ok(event)
    }
    pub fn bound_deliveries(&mut self) {
        while self.queues.values().map(|q| q.pending.len()).sum::<usize>() > MAX_DELIVERIES {
            let oldest = self
                .queues
                .iter()
                .filter_map(|(id, q)| q.pending.front().map(|d| (id.clone(), d.event.sequence)))
                .min_by_key(|(_, seq)| *seq)
                .map(|(id, _)| id);
            let Some(oldest) = oldest else {
                break;
            };
            let queue = self.queues.get_mut(&oldest).expect("selected queue");
            queue.pending.pop_front();
            queue.counters.dropped = queue.counters.dropped.saturating_add(1);
        }
    }
}
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
#[derive(Debug)]
pub(crate) struct Store {
    path: PathBuf,
    _lock: File,
    writer: Mutex<()>,
    jobs: tokio_util::task::TaskTracker,
    stopping: std::sync::atomic::AtomicBool,
    state: RwLock<Arc<State>>,
}
impl Store {
    pub fn open(path: &Path) -> io::Result<Arc<Self>> {
        if path.as_os_str().is_empty() {
            return Err(invalid("empty node state path"));
        }
        let mut dirs = std::fs::DirBuilder::new();
        dirs.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            dirs.mode(0o700);
        }
        dirs.create(parent(path))?;
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock = options().create(true).truncate(false).open(lock_path)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "node journal is already owned by another daemon",
            )
        })?;
        remove_stale_temporaries(path);
        let state = match read(path) {
            Ok(state) => state,
            // The journal holds event history and delivery queues, which the
            // platform resynchronizes from a new epoch. Starting with a fresh
            // epoch beats a server that cannot start because of one bad byte.
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let kept = crate::control::store::quarantine(path)?;
                tracing::error!(%error, kept = %kept.display(), "node journal is unreadable; starting a new journal epoch");
                State::default()
            }
            Err(error) => return Err(error),
        };
        Ok(Arc::new(Self {
            path: path.to_owned(),
            _lock: lock,
            writer: Mutex::new(()),
            jobs: tokio_util::task::TaskTracker::new(),
            stopping: std::sync::atomic::AtomicBool::new(false),
            state: RwLock::new(Arc::new(state)),
        }))
    }
    pub fn snapshot(&self) -> Arc<State> {
        self.state.read().clone()
    }
    /// Only blocking workers hold the writer lock; async readers clone an Arc.
    pub async fn update<T: Send + 'static>(
        self: &Arc<Self>,
        edit: impl FnOnce(&mut State) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let guard = self.jobs.token();
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            // Declared after the guard so this Arc, and with it the journal
            // lock, is gone before shutdown() observes the job finishing.
            let store = store;
            let _writer = store.writer.lock();
            let mut next = (*store.snapshot()).clone();
            let result = edit(&mut next)?;
            next.validate()?;
            let bytes = serde_json::to_vec(&next).map_err(invalid)?;
            if bytes.len() > MAX_BYTES {
                return Err(invalid("node journal exceeds 16 MiB"));
            }
            store.persist(&bytes)?;
            *store.state.write() = Arc::new(next);
            Ok(result)
        })
        .await
        .map_err(io::Error::other)?
    }
    pub async fn shutdown(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::Release);
        self.jobs.close();
        self.jobs.wait().await;
    }
    fn persist(&self, bytes: &[u8]) -> io::Result<()> {
        struct Temporary(Option<PathBuf>);
        impl Drop for Temporary {
            fn drop(&mut self) {
                if let Some(path) = &self.0 {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        let mut temp = Temporary(Some(temporary(&self.path)));
        let mut file = options()
            .create_new(true)
            .open(temp.0.as_ref().expect("new temporary"))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(temp.0.as_ref().expect("temporary"), &self.path)?;
        temp.0 = None;
        #[cfg(unix)]
        {
            if let Err(error) =
                File::open(parent(&self.path)).and_then(|directory| directory.sync_all())
            {
                // Rename already committed. Never restore old in-memory state here.
                tracing::error!(%error,"node journal committed but directory sync failed");
            }
        }
        Ok(())
    }
}
fn read(path: &Path) -> io::Result<State> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(State::default()),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(invalid("node journal must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(invalid("node journal exceeds 16 MiB"));
    }
    // An unknown version is most likely a downgrade: fail closed and keep it.
    #[derive(Deserialize)]
    struct Version {
        version: u16,
    }
    if serde_json::from_slice::<Version>(&bytes).is_ok_and(|header| header.version != 1) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported node journal version",
        ));
    }
    let state: State = serde_json::from_slice(&bytes).map_err(invalid)?;
    state.validate()?;
    Ok(state)
}
/// Temporary names include the journal name, so journals sharing a directory
/// never remove each other's in-flight writes.
fn temporary(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    parent(path).join(format!(".felucca-node-{name}-{}.tmp", Uuid::new_v4()))
}
/// Removes temporaries left by a crash; the caller holds the journal lock.
fn remove_stale_temporaries(path: &Path) {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let prefix = format!(".felucca-node-{name}-");
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
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Journal(PathBuf);
    impl Journal {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("felucca-node-store-{}", Uuid::new_v4())))
        }
        fn path(&self) -> PathBuf {
            self.0.join("node.json")
        }
    }
    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[tokio::test]
    async fn journal_retains_epoch_history_and_rejects_concurrent_or_corrupt_writers() {
        let file = Journal::new();
        let store = Store::open(&file.path()).unwrap();
        let epoch = store.snapshot().epoch;
        assert_eq!(
            Store::open(&file.path()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        store
            .update(|state| {
                for revision in 0..110 {
                    state.event(revision, "config.changed", 100)?;
                }
                Ok(())
            })
            .await
            .unwrap();
        store.shutdown().await;
        drop(store);
        let store = Store::open(&file.path()).unwrap();
        assert_eq!(store.snapshot().epoch, epoch);
        assert_eq!(store.snapshot().history.len(), 100);
        assert_eq!(store.snapshot().history[0].sequence, 11);
        store.shutdown().await;
        drop(store);
        // A future version fails closed and stays untouched.
        std::fs::write(file.path(), r#"{"version":99}"#).unwrap();
        assert!(Store::open(&file.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(file.path()).unwrap(),
            r#"{"version":99}"#
        );
        // A damaged journal is moved aside and a new epoch starts.
        std::fs::write(file.path(), "broken").unwrap();
        let store = Store::open(&file.path()).unwrap();
        assert_ne!(store.snapshot().epoch, epoch);
        assert!(store.snapshot().history.is_empty());
        let kept: Vec<_> = std::fs::read_dir(&file.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("node.json.corrupt-")
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read(kept[0].path()).unwrap(), b"broken");
        store.shutdown().await;
    }
    #[tokio::test]
    async fn failed_edit_or_rename_preserves_previous_state_and_removes_only_its_temporary() {
        let file = Journal::new();
        let store = Store::open(&file.path()).unwrap();
        store
            .update(|state| state.event(1, "config.changed", 100))
            .await
            .unwrap();
        let disk = std::fs::read(file.path()).unwrap();
        assert!(
            store
                .update(|state| {
                    state.event(2, "config.changed", 100)?;
                    Err::<(), _>(io::ErrorKind::InvalidInput.into())
                })
                .await
                .is_err()
        );
        assert_eq!(store.snapshot().next, 2);
        assert_eq!(std::fs::read(file.path()).unwrap(), disk);
        std::fs::remove_file(file.path()).unwrap();
        std::fs::create_dir(file.path()).unwrap();
        let unrelated = file.0.join(".felucca-node-unrelated.tmp");
        std::fs::write(&unrelated, "keep").unwrap();
        assert!(
            store
                .update(|state| state.event(2, "config.changed", 100))
                .await
                .is_err()
        );
        assert_eq!(store.snapshot().next, 2);
        assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "keep");
        assert_eq!(
            std::fs::read_dir(&file.0)
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
                .count(),
            1
        );
        store.shutdown().await;
    }
    #[tokio::test]
    async fn cancelled_request_still_finishes_disk_commit_before_shutdown_releases_lock() {
        let file = Journal::new();
        let store = Store::open(&file.path()).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let worker = store.clone();
        let request = tokio::spawn(async move {
            worker
                .update(move |state| {
                    entered.send(()).unwrap();
                    wait.recv().unwrap();
                    state.event(1, "config.changed", 100)
                })
                .await
        });
        ready.await.unwrap();
        request.abort();
        let _ = request.await;
        let stopping = store.clone();
        let mut stopped = tokio::spawn(async move {
            stopping.shutdown().await;
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut stopped)
                .await
                .is_err()
        );
        resume.send(()).unwrap();
        stopped.await.unwrap();
        assert!(store.update(|_| Ok(())).await.is_err());
        drop(store);
        let restored = Store::open(&file.path()).unwrap();
        assert_eq!(restored.snapshot().next, 2);
        restored.shutdown().await;
    }
}
