//! Atomic desired configuration, separate from the traffic snapshot.
use felucca_core::ServerConfig;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub version: u32,
    pub revision: u64,
    pub config: ServerConfig,
    #[serde(default)]
    pub operations: Vec<crate::daemon::OperationRecord>,
}

/// A saved configuration, and the reason if it came from the last-good copy.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub snapshot: Snapshot,
    pub recovered: Option<io::Error>,
}

#[derive(Debug)]
pub(crate) struct ConfigStore {
    path: PathBuf,
    _lock: File,
}
impl ConfigStore {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub fn open(path: &Path) -> io::Result<Self> {
        if path.as_os_str().is_empty() {
            return Err(io::ErrorKind::InvalidInput.into());
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
                "runtime configuration is already owned by another daemon",
            )
        })?;
        let store = Self {
            path: path.to_owned(),
            _lock: lock,
        };
        store.remove_stale_temporaries();
        Ok(store)
    }
    pub fn load(&self) -> io::Result<Option<Loaded>> {
        Self::read_recoverable(&self.path)
    }

    /// Reads the saved configuration, or the last one that started when the
    /// saved file no longer parses or validates.
    ///
    /// Writes are atomic, so this covers damaged media and hand edits. Without
    /// it one bad byte keeps the server down until someone repairs the file
    /// by hand, since the saved file overrides the bootstrap configuration.
    /// A missing file still means "start from the bootstrap", and I/O errors
    /// are reported unchanged.
    pub fn read_recoverable(path: &Path) -> io::Result<Option<Loaded>> {
        match Self::read(path) {
            Ok(snapshot) => Ok(snapshot.map(|snapshot| Loaded {
                snapshot,
                recovered: None,
            })),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                match Self::read(&last_good(path)) {
                    Ok(Some(snapshot)) => Ok(Some(Loaded {
                        snapshot,
                        recovered: Some(error),
                    })),
                    _ => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Moves an unreadable saved file aside for inspection. Rename needs no
    /// free space, and `restore` puts it back if the recovery cannot persist.
    pub fn quarantine(&self) -> io::Result<Quarantined> {
        Ok(Quarantined {
            original: self.path.clone(),
            kept: quarantine(&self.path)?,
        })
    }

    /// Records the configuration that just started, unless a copy exists.
    pub fn ensure_last_good(&self) {
        if !last_good(&self.path).exists() {
            refresh_last_good(&self.path);
        }
    }

    /// A crash between writing and renaming leaves a temporary file behind.
    /// This store holds the lock, so no other writer owns one of these names.
    fn remove_stale_temporaries(&self) {
        let name = self.path.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!("{TEMP_PREFIX}{name}-");
        let Ok(entries) = std::fs::read_dir(parent(&self.path)) else {
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
    pub fn read(path: &Path) -> io::Result<Option<Snapshot>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !file.metadata()?.is_file() {
            return Err(invalid("runtime configuration must be a regular file"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(invalid("runtime configuration exceeds 16 MiB"));
        }
        let mut snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
        if snapshot.version != 1 {
            return Err(invalid("unsupported runtime configuration version"));
        }
        snapshot.config.validate().map_err(invalid)?;
        if snapshot.operations.len() > 256
            || snapshot
                .operations
                .iter()
                .any(|op| op.revision > snapshot.revision)
        {
            return Err(invalid("invalid configuration operation history"));
        }
        Ok(Some(snapshot))
    }
    pub fn prepare(&self, snapshot: &Snapshot) -> io::Result<Pending> {
        let bytes = serde_json::to_vec(snapshot).map_err(invalid)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(invalid("runtime configuration exceeds 16 MiB"));
        }
        let mut pending = Pending {
            temp: Some(temporary(&self.path)),
            destination: self.path.clone(),
        };
        let result = (|| {
            let mut file = options()
                .create_new(true)
                .open(pending.temp.as_ref().expect("new pending"))?;
            file.write_all(&bytes)?;
            file.sync_all()
        })();
        if let Err(error) = result {
            if let Some(path) = pending.temp.take() {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
        Ok(pending)
    }
}

pub(crate) struct Pending {
    temp: Option<PathBuf>,
    destination: PathBuf,
}
impl Pending {
    /// Rename is the commit point. A later directory-sync failure cannot be
    /// reported as a rollback: the new configuration is already visible.
    pub fn commit(mut self) -> io::Result<bool> {
        std::fs::rename(self.temp.as_ref().expect("uncommitted"), &self.destination)?;
        self.temp = None;
        if let Err(error) = sync_parent(&self.destination) {
            tracing::error!(%error,"configuration committed, but directory synchronization failed");
            return Ok(false);
        }
        refresh_last_good(&self.destination);
        Ok(true)
    }
}

/// A saved file moved aside by [`ConfigStore::quarantine`].
#[derive(Debug)]
pub(crate) struct Quarantined {
    original: PathBuf,
    pub kept: PathBuf,
}
impl Quarantined {
    pub fn restore(self) {
        if let Err(error) = std::fs::rename(&self.kept, &self.original) {
            tracing::error!(%error, kept = %self.kept.display(), "cannot restore the unreadable configuration file");
        }
    }
}

const TEMP_PREFIX: &str = ".felucca-config-";

/// Renames an unreadable state file to `<name>.corrupt-<unix seconds>`.
pub(crate) fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut kept = path.as_os_str().to_owned();
    kept.push(format!(".corrupt-{stamp}"));
    let kept = PathBuf::from(kept);
    std::fs::rename(path, &kept)?;
    Ok(kept)
}

/// Temporary names include the file name, so stores sharing a directory never
/// remove each other's in-flight writes.
fn temporary(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    parent(path).join(format!("{TEMP_PREFIX}{name}-{}.tmp", uuid::Uuid::new_v4()))
}

fn last_good(path: &Path) -> PathBuf {
    let mut copy = path.as_os_str().to_owned();
    copy.push(".last-good");
    PathBuf::from(copy)
}

/// Copies a configuration that started to the last-good file, best effort.
/// A failure only loses the fallback; the live configuration is already saved.
fn refresh_last_good(path: &Path) {
    let temp = temporary(path);
    let result = (|| {
        let mut source = File::open(path)?;
        let mut copy = options().create_new(true).open(&temp)?;
        io::copy(&mut source, &mut copy)?;
        copy.sync_all()?;
        std::fs::rename(&temp, last_good(path))?;
        sync_parent(path)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        tracing::warn!(%error, "cannot update the last-good configuration copy");
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(parent(path))?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(temp) = self.temp.take() {
            let _ = std::fs::remove_file(temp);
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
