//! Rotation occurs between complete tracing events, under one shared lock.
use felucca_core::config::server::LogRotation;
use parking_lot::{Mutex, MutexGuard};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone)]
pub struct RotatingFile(Arc<Mutex<State>>);
struct State {
    path: PathBuf,
    config: LogRotation,
    file: Option<File>,
    _lock: File,
    bytes: u64,
    opened: Instant,
}
impl RotatingFile {
    pub fn new(path: &Path, config: LogRotation) -> io::Result<Self> {
        if config.max_bytes == 0 || config.max_files == 0 || config.max_files > 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid log rotation bounds",
            ));
        }
        let lock = append(&suffix(path, "rotate.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|error| {
            io::Error::new(error.kind(), "another process owns this rotating log")
        })?;
        let file = append(path)?;
        let bytes = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(State {
            path: path.to_owned(),
            config,
            file: Some(file),
            _lock: lock,
            bytes,
            opened: Instant::now(),
        }))))
    }
}
fn append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
fn suffix(path: &Path, name: impl std::fmt::Display) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(format!(".{name}"));
    PathBuf::from(value)
}
impl State {
    fn rotate(&mut self) -> io::Result<()> {
        if self.bytes == 0
            || (self.bytes < self.config.max_bytes
                && self
                    .config
                    .interval
                    .is_none_or(|interval| self.opened.elapsed() < interval))
        {
            return Ok(());
        }
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        let result = (|| {
            for index in (1..self.config.max_files).rev() {
                let source = suffix(&self.path, index);
                match std::fs::rename(source, suffix(&self.path, index + 1)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            std::fs::rename(&self.path, suffix(&self.path, 1))
        })();
        // Reopen even after a failed rename so one filesystem error does not
        // leave the logger permanently holding None. Errors still reach Write.
        let file = append(&self.path)?;
        self.bytes = file.metadata()?.len();
        self.file = Some(file);
        if result.is_ok() {
            self.opened = Instant::now();
        }
        result
    }
}
pub struct LogGuard<'a> {
    state: MutexGuard<'a, State>,
    error: Option<io::Error>,
}
impl<'a> MakeWriter<'a> for RotatingFile {
    type Writer = LogGuard<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        let mut state = self.0.lock();
        let error = state.rotate().err();
        LogGuard { state, error }
    }
}
impl Write for LogGuard<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        let file = self
            .state
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is unavailable"))?;
        let written = file.write(bytes)?;
        self.state.bytes = self.state.bytes.saturating_add(written as u64);
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.state
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is unavailable"))?
            .flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "felucca-log-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn concurrent_fragmented_events_remain_whole_and_retention_is_bounded() {
        let temp = Temp::new();
        let path = temp.0.join("events.log");
        let writer = RotatingFile::new(
            &path,
            LogRotation {
                max_bytes: 100,
                max_files: 3,
                interval: None,
            },
        )
        .unwrap();
        std::thread::scope(|scope| {
            for thread in 0..4 {
                let writer = writer.clone();
                scope.spawn(move || {
                    for event in 0..200 {
                        let mut record = writer.make_writer();
                        write!(record, "{{\"thread\":{thread},").unwrap();
                        writeln!(record, "\"event\":{event}}}").unwrap();
                    }
                });
            }
        });
        drop(writer);
        for path in std::iter::once(path.clone()).chain((1..=3).map(|n| suffix(&path, n))) {
            let text = std::fs::read_to_string(path).unwrap();
            assert!(text.len() <= 130, "one record of bounded overshoot");
            for line in text.lines() {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                assert!(event["thread"].is_number() && event["event"].is_number());
            }
        }
        assert!(!suffix(&path, 4).exists());
    }
    #[test]
    fn lock_prevents_two_writers_and_time_rotation_retains_existing_contents() {
        let temp = Temp::new();
        let path = temp.0.join("events.log");
        std::fs::write(&path, "before restart\n").unwrap();
        let rotation = LogRotation {
            max_bytes: 65536,
            max_files: 2,
            interval: Some(Duration::from_secs(1)),
        };
        let writer = RotatingFile::new(&path, rotation.clone()).unwrap();
        assert!(RotatingFile::new(&path, rotation.clone()).is_err());
        writer.0.lock().opened = Instant::now() - Duration::from_secs(2);
        writeln!(writer.make_writer(), "after rotation").unwrap();
        assert_eq!(
            std::fs::read_to_string(suffix(&path, 1)).unwrap(),
            "before restart\n"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after rotation\n");
        drop(writer);
        assert!(RotatingFile::new(&path, rotation).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(suffix(&path, "rotate.lock"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
