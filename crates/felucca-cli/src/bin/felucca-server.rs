//! The felucca server.

use anyhow::Context;
use clap::Parser;
use felucca_cli::messages::say;
use felucca_core::ServerConfig;
use felucca_i18n::{Message, Value};
use felucca_server::Daemon;

#[derive(Debug, Parser)]
#[command(name = "felucca-server", version, about = "Expose services behind NAT")]
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
    /// Read one password line from stdin and print its management password hash.
    #[arg(long, conflicts_with_all = ["generate_token", "check", "fingerprint"])]
    hash_password: bool,
    /// Print a new management bearer token and its storage hash as JSON.
    #[arg(long, conflicts_with_all = ["check", "fingerprint"])]
    generate_token: bool,
    /// Persist runtime configuration here; an existing snapshot overrides --config.
    #[arg(long, env = "FELUCCA_RUNTIME_CONFIG")]
    runtime_config: Option<std::path::PathBuf>,
    /// TOML/JSON file, HTTP(S) URL or inline JSON/base64 configuration.
    #[arg(short, long, env = "FELUCCA_CONFIG")]
    config: Option<String>,

    /// Create a private configuration and initial login, then start the server.
    #[arg(long, conflicts_with_all = ["check", "fingerprint", "hash_password", "generate_token", "build_info"])]
    init: bool,

    /// Browser address for first-run setup. HTTPS uses a local TLS reverse proxy.
    #[arg(long, requires = "init")]
    admin_url: Option<String>,

    /// Expand ${VAR} and ${VAR:-default} in parsed string values.
    #[arg(long, env = "FELUCCA_EXPAND_ENV")]
    expand_env: bool,

    /// Environment variable containing the bearer token for remote configuration downloads.
    #[arg(long, env = "FELUCCA_CONFIG_TOKEN_ENV")]
    config_token_env: Option<String>,

    /// Language for messages: zh, en, ja, ko, fr, es or ru. Defaults to English.
    #[arg(long, value_name = "LANGUAGE")]
    lang: Option<String>,

    /// Check the configuration and exit without binding anything.
    #[arg(long)]
    check: bool,

    /// Print the configured bridge certificate fingerprint and exit.
    /// Requires explicit bridge.tls; automatic identities are reported at startup.
    #[arg(long)]
    fingerprint: bool,
}

fn main() -> anyhow::Result<()> {
    felucca_cli::service::entrypoint(run)
}

#[tokio::main]
async fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    felucca_cli::messages::adopt(args.lang.as_deref());
    if args.build_info {
        felucca_cli::print_build_info("felucca-server");
        return Ok(());
    }
    if let Some(command) = args.command {
        return command.run("felucca-server").await;
    }
    if args.hash_password {
        use std::io::{BufRead, Read};
        let input = std::io::stdin();
        let mut reader = std::io::BufReader::new(input.lock().take(4099));
        let mut password = Vec::new();
        reader.read_until(b'\n', &mut password)?;
        if password.last() == Some(&b'\n') {
            password.pop();
        }
        if password.last() == Some(&b'\r') {
            password.pop();
        }
        println!(
            "{}",
            felucca_server::admin::credentials::hash_password(&password)?
        );
        return Ok(());
    }
    if args.generate_token {
        let token = felucca_server::admin::credentials::token();
        println!(
            "{}",
            serde_json::json!({"token":token,"token_hash":felucca_server::admin::credentials::token_hash(&token)})
        );
        return Ok(());
    }
    let source = args.config.as_deref().unwrap_or("felucca-server.toml");
    if args.init {
        anyhow::ensure!(
            !source.contains("://") && !source.starts_with(['{', '[']),
            "{}",
            say("Choose a local configuration file path for first-run setup.")
        );
        felucca_cli::initialize::server(std::path::Path::new(source), args.admin_url.as_deref())?;
    }
    let mut config: ServerConfig =
        felucca_cli::config_source::load(source, args.expand_env, args.config_token_env.as_deref())
            .await
            .with_context(|| {
                say(Message::with(
                    "cannot load {source}",
                    [Value::new(
                        "server",
                        felucca_cli::config_source::label(source),
                    )],
                ))
            })?;
    config = Daemon::load_configuration(config, args.runtime_config.as_deref())?;
    config.validate()?;

    if args.check {
        println!(
            "{}",
            say(Message::with(
                "{server} is valid",
                [Value::new(
                    "server",
                    felucca_cli::config_source::label(source)
                )],
            ))
        );
        return Ok(());
    }

    if args.fingerprint {
        let tls = config.bridge.tls.as_ref().context(say(
            "--fingerprint requires [bridge.tls]. For an automatic identity, use the running server's fingerprint in the console or startup log."
        ))?;
        println!(
            "{}",
            felucca_transport::Identity::from_pem_files(&tls.cert, &tls.key)?.fingerprint()
        );
        return Ok(());
    }

    felucca_cli::init_logging(&config.log)?;
    felucca_cli::process::prepare();
    let shutdown = felucca_cli::ShutdownSignal::prepare()
        .await
        .context(say("cannot install shutdown signal handlers"))?;
    let daemon = Daemon::start(config, args.runtime_config.as_deref())
        .await
        .context(say("cannot start the daemon"))?;
    tracing::info!("felucca-server ready");
    felucca_cli::service::ready()?;

    shutdown.wait().await;

    // Cancel listeners and active relays.
    daemon
        .shutdown()
        .await
        .context(say("cannot finish server shutdown"))?;
    tracing::info!("shutting down");
    Ok(())
}
