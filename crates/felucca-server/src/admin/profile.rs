//! Process-wide sampling. A disconnected HTTP client must not release the lock
//! while its blocking profiler is still changing the process signal handler.
use super::{Body, Error, Result, bad};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use http::{HeaderValue, header};
use http::{Response, StatusCode};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Options {
    #[serde(default = "duration")]
    seconds: u64,
}
fn duration() -> u64 {
    5
}

pub(super) async fn collect(
    options: Options,
    cancel: CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) -> Result<Response<Body>> {
    if !(1..=30).contains(&options.seconds) {
        return Err(bad("profile seconds must be 1..30"));
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::sync::{Arc, OnceLock};
        static WORKER: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
        let permit = WORKER
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                Error(
                    StatusCode::CONFLICT,
                    "a CPU profile is already running".into(),
                )
            })?;
        // HTTP cancellation drops the JoinHandle, but shutdown must still wait
        // for the worker to restore the process signal handler and release its permit.
        let svg = tasks
            .spawn_blocking(move || {
                let _permit = permit;
                sample(options.seconds, cancel)
            })
            .await
            .map_err(|_| {
                Error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "profile worker failed".into(),
                )
            })?
            .map_err(|error| {
                tracing::warn!(%error, "CPU profiling failed");
                Error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "CPU profiling unavailable".into(),
                )
            })?;
        let mut response = crate::http_support::response(StatusCode::OK, svg);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("image/svg+xml"),
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"felucca-cpu.svg\""),
        );
        Ok(response)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (cancel, tasks);
        Err(Error(
            StatusCode::NOT_IMPLEMENTED,
            "CPU profiling is unavailable on this platform".into(),
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn sample(seconds: u64, cancel: CancellationToken) -> anyhow::Result<Vec<u8>> {
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    anyhow::ensure!(!cancel.is_cancelled(), "server stopped");
    let guard = pprof::ProfilerGuardBuilder::default()
        .frequency(49)
        // Avoid unwinding allocator / system signal frames.
        .blocklist(&["libc", "libgcc", "pthread", "vdso", "libsystem"])
        .build()?;
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        anyhow::ensure!(!cancel.is_cancelled(), "server stopped");
        std::thread::sleep(
            Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())),
        );
    }
    let report = guard.report().build()?;
    drop(guard);
    if report.data.is_empty() {
        return Ok(br#"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="80"><text x="16" y="40">No CPU samples recorded during this interval.</text></svg>"#.to_vec());
    }
    // pprof's renderer unwraps write errors. Consume excess bytes but discard
    // them, then report the bound explicitly instead of panicking in its code.
    #[derive(Default)]
    struct Limited {
        bytes: Vec<u8>,
        overflow: bool,
    }
    impl Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if !self.overflow
                && bytes.len() <= (8 * 1024 * 1024usize).saturating_sub(self.bytes.len())
            {
                self.bytes.extend_from_slice(bytes);
            } else {
                self.overflow = true;
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Limited::default();
    report.flamegraph(&mut output)?;
    anyhow::ensure!(!output.overflow, "CPU profile exceeds 8 MiB");
    Ok(output.bytes)
}
