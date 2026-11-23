use super::*;
use std::process::Stdio;
use tokio::process::Command;

#[tokio::test]
async fn first_run_setup_creates_private_durable_configuration_without_overwriting() {
    let files = identity("initialize.test").await;
    let path = files.root.join("server.toml");
    let port = free_port().await;
    let origin = format!("http://127.0.0.1:{port}");
    felucca_cli::initialize::server(&path, Some(&origin)).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    let mut config: ServerConfig = toml::from_str(&saved).unwrap();
    let admin = config.admin.as_ref().unwrap();
    assert_eq!(admin.bind, format!("127.0.0.1:{port}"));
    assert_eq!(admin.origin.as_deref(), Some(origin.as_str()));
    assert!(admin.state_path.is_absolute());
    assert!(!admin.accounts["root"].password_hash.is_empty());
    assert!(config.state.as_ref().unwrap().path.is_absolute());
    assert!(felucca_cli::initialize::server(&path, Some(&origin)).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    config.bridge.bind = "127.0.0.1:0".into();
    let daemon = felucca_server::Daemon::start(config.clone(), None)
        .await
        .unwrap();
    let fingerprint = daemon.server().fingerprint();
    assert!(
        http_request(
            port,
            &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
        )
        .await
        .contains("200 OK")
    );
    daemon.shutdown().await.unwrap();
    drop(daemon);
    let restarted = felucca_server::Daemon::start(config, None).await.unwrap();
    assert_eq!(restarted.server().fingerprint(), fingerprint);
    restarted.shutdown().await.unwrap();
    drop(restarted);
    std::fs::remove_file(&path).unwrap();
    assert!(felucca_cli::initialize::server(&path, Some(&origin)).is_err());
    assert!(
        !path.exists(),
        "initialization must not print a new login over existing server data"
    );
}

#[tokio::test]
async fn first_run_setup_rejects_ambiguous_addresses_and_supports_reverse_proxy() {
    let files = identity("initialize-url.test").await;
    let path = files.root.join("server.toml");
    for address in [
        "example.com",
        "ftp://example.com",
        "http://root:secret@example.com",
        "https://example.com/admin",
        "https://example.com/?key=secret",
    ] {
        assert!(felucca_cli::initialize::server(&path, Some(address)).is_err());
        assert!(!path.exists());
    }
    felucca_cli::initialize::server(&path, Some("https://console.example.com")).unwrap();
    let config: ServerConfig = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let admin = config.admin.unwrap();
    assert_eq!(admin.bind, "127.0.0.1:8990");
    assert_eq!(admin.origin.as_deref(), Some("https://console.example.com"));
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_felucca-agent"));
    command
        .kill_on_drop(true)
        .env_remove("FELUCCA_CONFIG")
        .env_remove("FELUCCA_EXPAND_ENV")
        .env_remove("FELUCCA_CONFIG_TOKEN_ENV")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}
#[cfg(unix)]
struct Servers(Vec<Arc<Server>>);
#[cfg(unix)]
impl Drop for Servers {
    fn drop(&mut self) {
        for server in &self.0 {
            server.shutdown();
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn one_agent_process_runs_independent_servers_with_env_keys_and_sigterm() {
    let mut servers = Servers(Vec::new());
    let mut configurations = Vec::new();
    let mut ports = Vec::new();
    // Quotes, newlines and placeholder text remain literal key bytes.
    let first_key = "\"\nserver='malicious:1'\n${DO_NOT_EXPAND}";
    let second_key = "other-instance-secret";
    for (key, variable) in [
        (first_key, "SOURCE_KEY_ONE"),
        (second_key, "SOURCE_KEY_TWO"),
    ] {
        let backend = echo().await;
        let public = free_port().await;
        let mut config = server_config();
        config.clients.get_mut("features").unwrap().vkey = felucca_core::Secret::new(key);
        let server = Arc::new(Server::new(config).unwrap());
        let bridge = server.clone().run().await.unwrap();
        let mut config = agent(&format!(
            "[[tunnel]]\nmode='tcp'\nbind='127.0.0.1'\nport={public}\nbackend=[{{addr='127.0.0.1:{backend}'}}]"
        ));
        config.server = format!("127.0.0.1:{bridge}");
        config.vkey = felucca_core::Secret::new(format!("${{{variable}}}"));
        config.trust = TrustPolicy::Pinned {
            fingerprint: server.fingerprint(),
        };
        configurations.push(serde_json::to_string(&config).unwrap());
        ports.push(public);
        servers.0.push(server);
    }
    let mut command = command();
    command
        .arg("--expand-env")
        .env("SOURCE_KEY_ONE", first_key)
        .env("SOURCE_KEY_TWO", second_key);
    for config in &configurations {
        command.args(["--config", config]);
    }
    let child = command.spawn().unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        for port in &ports {
            loop {
                if let Ok(io) = TcpStream::connect(("127.0.0.1", *port)).await {
                    roundtrip(io).await;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    })
    .await
    .expect("both configured agent instances must forward traffic");
    servers.0[0].shutdown_and_wait().await.unwrap();
    roundtrip(TcpStream::connect(("127.0.0.1", ports[1])).await.unwrap()).await;
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().unwrap().to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!logs.contains(first_key));
    assert!(!logs.contains(second_key));
}

#[tokio::test]
async fn cli_remote_config_uses_env_bearer_auth_and_redacts_sources() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        for status in [200, 403] {
            let (mut io, _) = listener.accept().await.unwrap();
            let request = head(&mut io).await;
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer token-secret")
            );
            let body = r#"{"server":"127.0.0.1:9001","vkey":"body-secret"}"#;
            io.write_all(format!("HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    for valid in [true, false] {
        let output = command()
            .args([
                "--check",
                "--config-token-env",
                "SOURCE_TOKEN",
                "--config",
                &format!("http://{address}/private-path?token=query-secret"),
            ])
            .env("SOURCE_TOKEN", "token-secret")
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.success(), valid);
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for secret in [
            "token-secret",
            "query-secret",
            "body-secret",
            "private-path",
        ] {
            assert!(!logs.contains(secret), "{logs}");
        }
    }
    task.await.unwrap();
}

#[tokio::test]
async fn cli_checks_every_instance_before_connecting_and_rejects_duplicate_sessions() {
    let bridge = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let valid = serde_json::json!({"server":bridge.local_addr().unwrap().to_string(), "vkey":"test", "key_id":"same"}).to_string();
    let invalid = r#"{"server":"localhost:9001","vkey":"test","typo":true}"#;
    for second in [&valid, invalid] {
        let output = command()
            .args(["--config", &valid, "--config", second])
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), bridge.accept())
                .await
                .is_err()
        );
    }
    let output = command()
        .args([
            "--check",
            "--launch",
            "felucca://localhost:9001?key_id=one&vkey=first",
            "--launch",
            "felucca://localhost:9002?key_id=two&vkey=second",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .matches("is valid")
            .count(),
        2
    );
}
