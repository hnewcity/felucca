//! The felucca agent.

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use felucca_cli::messages::say;
use felucca_client::Agent;
use felucca_core::AgentConfig;
use felucca_core::config::LogConfig;
use felucca_i18n::{Message, Value};

#[derive(Debug, Parser)]
#[command(
    name = "felucca-agent",
    version,
    about = "Connect a local service to a felucca server"
)]
struct Args {
    /// Print the binary version, wire protocol and build target as JSON.
    #[arg(long)]
    build_info: bool,
    #[command(subcommand)]
    command: Option<felucca_cli::Maintenance>,
    #[arg(long, hide = true, requires = "service_workdir")]
    service_name: Option<String>,
    #[arg(long, hide = true, requires = "service_name")]
    service_workdir: Option<std::path::PathBuf>,
    /// TOML/JSON file, HTTP(S) URL, inline JSON/base64 or felucca:// URI. Repeat for multiple agents.
    #[arg(short, long, visible_alias = "launch", env = "FELUCCA_CONFIG")]
    config: Vec<String>,

    /// Expand ${VAR} and ${VAR:-default} in parsed string values.
    #[arg(long, env = "FELUCCA_EXPAND_ENV")]
    expand_env: bool,

    /// Environment variable containing the bearer token for remote configuration downloads.
    #[arg(long, env = "FELUCCA_CONFIG_TOKEN_ENV")]
    config_token_env: Option<String>,

    /// Language for messages: zh, en, ja, ko, fr, es or ru. Defaults to English.
    #[arg(long, value_name = "LANGUAGE")]
    lang: Option<String>,

    /// Check the configuration and exit without connecting.
    #[arg(long)]
    check: bool,

    /// Keep one running connection for each local connection file.
    #[arg(long, hide = true)]
    exclusive: bool,

    /// Log level: trace, debug, info, warn or error.
    #[arg(long, default_value = "info")]
    log_level: String,
    /// Append logs here as well as stderr.
    #[arg(long)]
    log_file: Option<std::path::PathBuf>,
    /// Write structured JSON events.
    #[arg(long)]
    log_json: bool,
    /// Rotate between events once this many bytes have been written.
    #[arg(long, requires = "log_file")]
    log_max_bytes: Option<u64>,
    /// Number of archived logs to retain when rotation is enabled.
    #[arg(long, default_value_t = 5, requires = "log_file")]
    log_backups: usize,
    /// Rotate after this interval (for example 24h), measured from opening the file.
    #[arg(long, requires = "log_file")]
    log_rotate_interval: Option<String>,
}

fn main() -> anyhow::Result<()> {
    felucca_cli::service::entrypoint(run)
}

#[tokio::main]
async fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    felucca_cli::messages::adopt(args.lang.as_deref());
    if args.build_info {
        felucca_cli::print_build_info("felucca-agent");
        return Ok(());
    }
    if let Some(command) = args.command {
        return command.run("felucca-agent").await;
    }
    let rotate = args.log_file.is_some();
    let log = LogConfig {
        level: args.log_level,
        file: args.log_file,
        json: args.log_json,
        rotation: if rotate {
            Some(felucca_core::config::server::LogRotation {
                max_bytes: args.log_max_bytes.unwrap_or(10 * 1024 * 1024),
                max_files: args.log_backups,
                interval: args
                    .log_rotate_interval
                    .as_deref()
                    .map(humantime::parse_duration)
                    .transpose()?,
            })
        } else {
            None
        },
    };
    log.validate()?;
    let sources = if args.config.is_empty() {
        vec!["felucca-agent.toml".into()]
    } else {
        args.config
    };
    anyhow::ensure!(
        sources.len() <= 32,
        "at most 32 agent instances may run in one process"
    );
    let mut configs = Vec::new();
    let mut identities = std::collections::HashSet::new();
    for source in &sources {
        let mut config: AgentConfig = felucca_cli::config_source::load(
            source,
            args.expand_env,
            args.config_token_env.as_deref(),
        )
        .await
        .with_context(|| {
            say(Message::with(
                "cannot load {server}",
                [Value::new(
                    "server",
                    felucca_cli::config_source::label(source),
                )],
            ))
        })?;
        config.validate()?;
        anyhow::ensure!(
            identities.insert((
                config.server.to_ascii_lowercase(),
                config
                    .key_id
                    .clone()
                    .unwrap_or_else(|| "felucca-agent".into())
            )),
            "{}",
            say("duplicate agent server and key_id would repeatedly replace the same session")
        );
        configs.push(config);
    }

    if args.check {
        for (source, config) in sources.iter().zip(&configs) {
            println!(
                "{}",
                say(Message::with(
                    "{server} is valid: {count} tunnels",
                    [
                        Value::new("server", felucca_cli::config_source::label(source)),
                        Value::new("count", config.tunnels.len()),
                    ],
                ))
            );
        }
        return Ok(());
    }

    // A copied command can be run twice accidentally. Keep an independent lock
    // file across atomic configuration replacement so the two agents cannot
    // continuously replace each other's server session.
    let _connection_locks = if args.exclusive {
        sources.iter().map(|source| {
            anyhow::ensure!(std::path::Path::new(source).is_file(), "{}", say("A saved local connection file is required."));
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)] {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(format!("{source}.lock"))?;
            fs2::FileExt::try_lock_exclusive(&file)
                .context(say("This device is already connected in another window. Close it before reconnecting."))?;
            Ok::<_, anyhow::Error>(file)
        }).collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };

    felucca_cli::init_logging(&log)?;
    felucca_cli::process::prepare();
    let shutdown = felucca_cli::ShutdownSignal::prepare()
        .await
        .context(say("cannot install shutdown signal handlers"))?;

    // Prepare every instance before any connects, so one bad root/ACL cannot
    // leave a partially running launch plan.
    let agents = configs
        .into_iter()
        .map(|config| {
            tracing::info!(server = %config.server, transport = config.transport.as_str(),
            tunnels = config.tunnels.len(), "felucca-agent starting");
            Agent::new(config)
                .map(Arc::new)
                .context(say("cannot initialize the agent"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut tasks = tokio::task::JoinSet::new();
    for agent in &agents {
        tasks.spawn(agent.clone().run());
    }
    felucca_cli::service::ready()?;
    let result = tokio::select! {
        result = tasks.join_next() => match result {
            Some(Err(error)) => Err(anyhow::anyhow!("agent instance failed: {error}")),
            _ => Err(anyhow::anyhow!("agent instance stopped unexpectedly")),
        },
        () = shutdown.wait() => {
            tracing::info!("shutting down");
            Ok(())
        },
    };
    for agent in agents {
        agent.shutdown();
    }
    tasks.shutdown().await;
    result
}
