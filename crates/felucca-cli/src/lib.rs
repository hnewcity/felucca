//! Shared plumbing for the felucca binaries.

pub mod config_source;
pub mod initialize;
pub mod logging;
pub mod messages;
pub mod process;
pub mod service;
pub mod update;
use std::path::PathBuf;

use felucca_core::config::LogConfig;

#[derive(Debug, clap::Subcommand)]
pub enum Maintenance {
    /// Install, render, start, stop or uninstall a system service.
    Service(service::ServiceArgs),
    /// Verify and install a standalone binary using a trusted SHA-256 digest.
    Update(update::UpdateArgs),
}
impl Maintenance {
    pub async fn run(self, role: &str) -> anyhow::Result<()> {
        match self {
            Self::Service(args) => args.run(role).await,
            Self::Update(args) => args.run(role).await,
        }
    }
}
pub fn print_build_info(program: &str) {
    println!(
        "{}",
        serde_json::json!({"program":program, "version":env!("CARGO_PKG_VERSION"),
        "protocol":felucca_proto::PROTOCOL_VERSION, "target":env!("FELUCCA_BUILD_TARGET")})
    );
}

/// Installs the logging subscriber.
///
/// `RUST_LOG` wins when set, so an operator can raise the level for one run
/// without editing a config file and restarting into it.
pub fn init_logging(config: &LogConfig) -> anyhow::Result<()> {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriterExt};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("felucca={}", config.level)));

    config.validate()?;
    let writer = if let (Some(path), Some(rotation)) = (&config.file, &config.rotation) {
        BoxMakeWriter::new(std::io::stderr.and(logging::RotatingFile::new(path, rotation.clone())?))
    } else if let Some(path) = &config.file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        BoxMakeWriter::new(std::io::stderr.and(std::sync::Arc::new(file)))
    } else {
        BoxMakeWriter::new(std::io::stderr)
    };
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(config.file.is_none());
    if config.json {
        builder.json().init();
    } else {
        builder.init();
    }

    Ok(())
}

/// Installs signal handlers before opening listeners or starting agents.
///
/// SIGTERM as well as ctrl-c, since that is what a service manager or a
/// container runtime sends; handling only ctrl-c means every restart ends in a
/// kill after the grace period.
pub struct ShutdownSignal {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(not(unix))]
    interrupt:
        Option<std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>>,
}
impl ShutdownSignal {
    pub async fn prepare() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
            })
        }
        #[cfg(not(unix))]
        {
            use std::{future::Future, task::Poll};
            let mut interrupt = Box::pin(tokio::signal::ctrl_c());
            // ctrl_c registers lazily on its first poll. Poll now and retain the
            // future, including a signal received before initialization ends.
            let ready = std::future::poll_fn(|cx| Poll::Ready(interrupt.as_mut().poll(cx))).await;
            let interrupt = match ready {
                Poll::Ready(result) => {
                    result?;
                    None
                }
                Poll::Pending => Some(
                    interrupt
                        as std::pin::Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>>,
                ),
            };
            Ok(Self { interrupt })
        }
    }
    pub async fn wait(mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.interrupt.recv() => tracing::info!("interrupted"),
            _ = self.terminate.recv() => tracing::info!("terminated"),
        }
        #[cfg(not(unix))]
        {
            let interrupt = async {
                if let Some(interrupt) = self.interrupt.take() {
                    let _ = interrupt.await;
                }
            };
            #[cfg(windows)]
            tokio::select! {
                () = interrupt => {},
                () = service::cancelled() => {},
            }
            #[cfg(not(windows))]
            interrupt.await;
            tracing::info!("interrupted");
        }
    }
}

/// Resolves a config path, preferring an explicit one.
pub fn config_path(explicit: Option<PathBuf>, default: &str) -> PathBuf {
    explicit.unwrap_or_else(|| PathBuf::from(default))
}
