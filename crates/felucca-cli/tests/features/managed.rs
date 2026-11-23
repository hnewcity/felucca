use super::*;
use felucca_core::access::TargetPolicy;
use felucca_core::config::{ManagedConfig, managed::RemoteFileRoot};
use felucca_server::Daemon;

fn desired(port: u16, backend: u16) -> ManagedConfig {
    toml::from_str(&format!(
        "[[tunnel]]\nid=1\nmode='tcp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='localhost:{backend}'}}]",
    )).unwrap()
}
async fn ready(agent: &Agent) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !agent.is_ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("managed agent did not register");
}
async fn body(port: u16) -> String {
    http_request(
        port,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await
}
async fn wait_body(port: u16, expected: &str) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(mut io) = TcpStream::connect(("127.0.0.1", port)).await {
                let _ = io
                    .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .await;
                let mut response = String::new();
                // Windows retries a refused loopback connect for about two
                // seconds before `localhost` falls back from ::1 to 127.0.0.1.
                if let Ok(Ok(_)) =
                    tokio::time::timeout(Duration::from_secs(5), io.read_to_string(&mut response))
                        .await
                    && response.contains(expected)
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("updated backend never became available");
}

#[tokio::test]
async fn live_edits_preserve_sessions_and_unrelated_streams_and_commit_failures_do_not_disrupt() {
    for transport in [TransportKind::Tls, TransportKind::Quic] {
        let files = identity("live.test").await;
        let path = files.root.join("live.json");
        let bridge = free_port().await;
        let first_port = free_port().await;
        let second_port = free_port().await;
        let backend = echo().await;
        let mut server = server_config();
        server.bridge.bind = format!("127.0.0.1:{bridge}");
        server.bridge.tls = Some(files.config.clone());
        if transport == TransportKind::Quic {
            server.bridge.quic_bind = Some(server.bridge.bind.clone());
        }
        let mut managed = desired(first_port, backend);
        let mut second = desired(second_port, backend).tunnels.remove(0);
        second.id = Some(2);
        managed.tunnels.push(second);
        server.managed.insert("features".into(), managed);
        let daemon = Daemon::start(server, Some(&path)).await.unwrap();
        let instance = daemon.server();
        let mut config = agent("");
        config.server = format!("127.0.0.1:{bridge}");
        config.transport = transport;
        config.trust = TrustPolicy::Pinned {
            fingerprint: instance.fingerprint(),
        };
        config.remote.enabled = true;
        let agent = Arc::new(Agent::new(config).unwrap());
        let task = tokio::spawn(agent.clone().run());
        ready(&agent).await;
        // Ensure the WatchConfig frame has reached the server.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let session = instance.registry().get("features").unwrap().id;
        let mut held = TcpStream::connect(("127.0.0.1", first_port)).await.unwrap();
        let mut changed = TcpStream::connect(("127.0.0.1", second_port))
            .await
            .unwrap();
        async fn exchange(stream: &mut TcpStream) {
            stream.write_all(b"still-open").await.unwrap();
            let mut reply = [0; 10];
            tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut reply))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&reply, b"still-open");
        }
        exchange(&mut held).await;
        exchange(&mut changed).await;

        let (revision, mut next) = daemon.config();
        let mut other = next.clients["features"].clone();
        other.vkey = "new-device-secret".into();
        next.clients.insert("new-device".into(), other);
        daemon.apply(next, revision).await.unwrap();
        exchange(&mut held).await;
        assert!(Arc::ptr_eq(&instance, &daemon.server()));
        assert_eq!(session, instance.registry().get("features").unwrap().id);

        let (revision, mut next) = daemon.config();
        next.managed.get_mut("features").unwrap().tunnels[1].enabled = false;
        daemon.apply(next, revision).await.unwrap();
        let mut byte = [0];
        let ended = tokio::time::timeout(Duration::from_secs(3), changed.read(&mut byte))
            .await
            .unwrap();
        assert!(
            matches!(ended, Ok(0) | Err(_)),
            "disabled route retained its connection"
        );
        assert!(
            TcpStream::connect(("127.0.0.1", second_port))
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(1200)).await;
        exchange(&mut held).await;
        assert_eq!(session, instance.registry().get("features").unwrap().id);

        let (revision, mut renamed) = daemon.config();
        renamed.managed.get_mut("features").unwrap().tunnels[0].remark =
            "Friendly service name".into();
        renamed.clients.get_mut("features").unwrap().remark = "Friendly device name".into();
        daemon.apply(renamed, revision).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
        exchange(&mut held).await;

        let (revision, mut invalid) = daemon.config();
        let bad_file: ManagedConfig = toml::from_str(
            "[[tunnel]]\nid=3\nmode='file'\npublic=false\n[tunnel.file]\nroot='not-granted'",
        )
        .unwrap();
        invalid
            .managed
            .get_mut("features")
            .unwrap()
            .tunnels
            .extend(bad_file.tunnels);
        daemon.apply(invalid, revision).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while agent.is_ready() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        exchange(&mut held).await;
        assert_eq!(session, instance.registry().get("features").unwrap().id);
        let (revision, mut repaired) = daemon.config();
        repaired
            .managed
            .get_mut("features")
            .unwrap()
            .tunnels
            .retain(|t| t.id != Some(3));
        daemon.apply(repaired, revision).await.unwrap();
        ready(&agent).await;
        exchange(&mut held).await;

        let saved = files.root.join("saved.json");
        std::fs::rename(&path, &saved).unwrap();
        std::fs::create_dir(&path).unwrap();
        let (revision, mut next) = daemon.config();
        next.clients.get_mut("features").unwrap().enabled = false;
        assert!(daemon.apply(next, revision).await.is_err());
        assert_eq!(daemon.config().0, revision);
        exchange(&mut held).await;
        assert_eq!(session, instance.registry().get("features").unwrap().id);
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&saved, &path).unwrap();

        let (revision, mut next) = daemon.config();
        next.clients.get_mut("features").unwrap().vkey = "rotated-secret".into();
        daemon.apply(next, revision).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(3), held.read(&mut byte))
            .await
            .unwrap();
        assert!(
            matches!(ended, Ok(0) | Err(_)),
            "rotated identity retained its connection"
        );
        assert!(instance.registry().get("features").is_none());
        agent.shutdown();
        task.await.unwrap();
        daemon.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn server_owned_routes_register_from_empty_bootstrap_on_verified_transports() {
    for transport in [TransportKind::Tls, TransportKind::Wss, TransportKind::Quic] {
        let port = free_port().await;
        let backend = web_backend("server-owned-backend").await;
        let mut server = server_config();
        server
            .clients
            .get_mut("features")
            .unwrap()
            .allow_config_push = false;
        server
            .managed
            .insert("features".into(), desired(port, backend));
        let mut config = agent("");
        config.remote.enabled = true;
        config.remote.target_policy.ports = vec![backend];
        let running = start(server, config, transport).await;
        assert!(body(port).await.contains("server-owned-backend"));
        running.agent.shutdown();
        running.server.shutdown_and_wait().await.unwrap();
    }
}

#[tokio::test]
async fn managed_edits_stop_start_delete_restore_and_survive_daemon_restart() {
    let files = identity("managed.test").await;
    let path = files.root.join("runtime.json");
    let bridge = free_port().await;
    let port = free_port().await;
    let original = web_backend("bootstrap-target").await;
    let first = web_backend("first-target").await;
    let second = web_backend("replacement-target").await;
    let mut server = server_config();
    server.bridge.bind = format!("127.0.0.1:{bridge}");
    server.bridge.tls = Some(files.config.clone());
    server
        .managed
        .insert("features".into(), desired(port, first));
    let bootstrap_server = server.clone();
    let daemon = Daemon::start(server, Some(&path)).await.unwrap();
    let mut config = agent("");
    config.tunnels = desired(port, original).tunnels;
    config.remote.enabled = true;
    config.remote.target_policy.ports = vec![original, first, second];
    config.server = format!("127.0.0.1:{bridge}");
    config.trust = TrustPolicy::Pinned {
        fingerprint: daemon.server().fingerprint(),
    };
    let agent = Arc::new(Agent::new(config).unwrap());
    let task = tokio::spawn(agent.clone().run());
    ready(&agent).await;
    assert!(body(port).await.contains("first-target"));
    let (rev, mut next) = daemon.config();
    next.managed
        .insert("features".into(), desired(port, second));
    daemon.apply(next, rev).await.unwrap();
    wait_body(port, "replacement-target").await;
    let (rev, mut next) = daemon.config();
    next.managed.get_mut("features").unwrap().tunnels[0].enabled = false;
    daemon.apply(next, rev).await.unwrap();
    // The managed disable must not fall back to the bootstrap.
    tokio::time::sleep(Duration::from_millis(250)).await;
    ready(&agent).await;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    let (rev, mut next) = daemon.config();
    next.managed.get_mut("features").unwrap().tunnels[0].enabled = true;
    daemon.apply(next, rev).await.unwrap();
    wait_body(port, "replacement-target").await;
    daemon.shutdown().await.unwrap();
    drop(daemon);
    let daemon = Daemon::start(bootstrap_server, Some(&path)).await.unwrap();
    wait_body(port, "replacement-target").await;
    let (rev, mut next) = daemon.config();
    next.managed
        .insert("features".into(), ManagedConfig::default());
    daemon.apply(next, rev).await.unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    ready(&agent).await;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    let (rev, mut next) = daemon.config();
    next.managed.remove("features");
    daemon.apply(next, rev).await.unwrap();
    wait_body(port, "bootstrap-target").await;
    agent.shutdown();
    task.await.unwrap();
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn plaintext_peer_cannot_request_config_secrets_or_forge_a_managed_digest() {
    use felucca_proto::{ConnectionRole, Control, StreamRequest, read_frame, write_frame};
    let mut config = server_config();
    config
        .managed
        .insert("features".into(), desired(free_port().await, echo().await));
    let server = Arc::new(Server::new(config).unwrap());
    let port = server.clone().run().await.unwrap();
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    felucca_proto::session::connect(
        &mut io,
        "features",
        &"feature-secret".into(),
        ConnectionRole::Tunnel,
    )
    .await
    .unwrap();
    let tunnel =
        felucca_transport::Tunnel::over_stream(io, felucca_mux::Role::Client, Default::default());
    let mut control = tunnel.open().await.unwrap();
    write_frame(&mut control, &StreamRequest::Control)
        .await
        .unwrap();
    write_frame(
        &mut control,
        &Control::PushConfig {
            tunnels: vec![],
            remote_config: true,
            revision: None,
        },
    )
    .await
    .unwrap();
    let answer: Control = read_frame(&mut control).await.unwrap();
    assert!(
        matches!(answer, Control::ConfigResult{ref rejected,..} if rejected[0].reason.contains("encrypted"))
    );
    drop(control);
    tunnel.close(felucca_mux::CloseReason::Normal);
    // An encrypted, authenticated peer still cannot publish different listener
    // options just by echoing the managed revision.
    let io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut io = felucca_transport::tls::handshake(
        io,
        "localhost",
        &felucca_transport::tls::Verification::Pinned {
            fingerprint: server.fingerprint(),
        },
        &[felucca_transport::BRIDGE_ALPN],
    )
    .await
    .unwrap();
    felucca_proto::session::connect(
        &mut io,
        "features",
        &"feature-secret".into(),
        ConnectionRole::Tunnel,
    )
    .await
    .unwrap();
    let tunnel =
        felucca_transport::Tunnel::over_stream(io, felucca_mux::Role::Client, Default::default());
    let mut control = tunnel.open().await.unwrap();
    write_frame(&mut control, &StreamRequest::Control)
        .await
        .unwrap();
    write_frame(
        &mut control,
        &Control::PushConfig {
            tunnels: vec![],
            remote_config: false,
            revision: None,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(read_frame::<_,Control>(&mut control).await.unwrap(), Control::ConfigResult{rejected,..} if !rejected.is_empty())
    );
    write_frame(
        &mut control,
        &Control::PushConfig {
            tunnels: vec![],
            remote_config: true,
            revision: None,
        },
    )
    .await
    .unwrap();
    let Control::Configure { revision, .. } = read_frame(&mut control).await.unwrap() else {
        panic!("expected configuration")
    };
    write_frame(
        &mut control,
        &Control::PushConfig {
            tunnels: vec![],
            remote_config: true,
            revision: Some(revision),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(read_frame::<_,Control>(&mut control).await.unwrap(), Control::ConfigResult{rejected,..} if rejected[0].reason.contains("do not match"))
    );
    tunnel.close(felucca_mux::CloseReason::Normal);
    server.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn managed_destination_rules_cannot_expand_operator_acl_or_health_probe_access() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let public = free_port().await;
    let mut managed = desired(public, backend_port);
    managed.tunnels[0].target_policy.allow = vec!["domain:localhost".into()];
    managed.tunnels[0].health = Some(felucca_core::config::HealthCheck {
        kind: felucca_core::config::HealthCheckKind::Tcp,
        interval: Duration::from_millis(50),
        timeout: Duration::from_millis(20),
        ..Default::default()
    });
    let mut server = server_config();
    server.managed.insert("features".into(), managed);
    let mut config = agent("");
    config.remote.enabled = true;
    config.remote.target_policy.deny = vec!["127.0.0.0/8".into(), "::1".into()];
    let running = start(server, config, TransportKind::Tls).await;
    let mut io = TcpStream::connect(("127.0.0.1", public)).await.unwrap();
    let _ = io.write_all(b"forbidden").await;
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), io.read_to_end(&mut response))
        .await
        .unwrap();
    assert!(response.is_empty());
    assert!(
        tokio::time::timeout(Duration::from_millis(200), backend.accept())
            .await
            .is_err()
    );
    running.agent.shutdown();
    running.server.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn managed_files_use_preopened_capabilities_and_refuse_write_escalation() {
    let files = identity("remote-files.test").await;
    let root = files.root.join("approved");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("allowed.txt"), "approved content").unwrap();
    std::fs::write(files.root.join("outside.txt"), "outside secret").unwrap();
    let public = free_port().await;
    let mut server = server_config();
    let managed: ManagedConfig = toml::from_str(&format!(
        "[[tunnel]]\nid=1\nmode='file'\nport={public}\nbind='127.0.0.1'\n[tunnel.file]\nroot='documents'\n[tunnel.file.auth.users]\nreader='password'"
    )).unwrap();
    server.managed.insert("features".into(), managed.clone());
    let mut config = agent("");
    config.remote.enabled = true;
    config.remote.file_roots.insert(
        "documents".into(),
        RemoteFileRoot {
            path: root.clone(),
            read_only: true,
        },
    );
    let running = start(server, config.clone(), TransportKind::Tls).await;
    let request = |path: &str, auth: bool| {
        format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\n{}Connection: close\r\n\r\n",
            if auth {
                "Authorization: Basic cmVhZGVyOnBhc3N3b3Jk\r\n"
            } else {
                ""
            }
        )
    };
    assert!(
        http_request(public, &request("/allowed.txt", false))
            .await
            .starts_with("HTTP/1.1 401")
    );
    assert!(
        http_request(public, &request("/allowed.txt", true))
            .await
            .contains("approved content")
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(files.root.join("outside.txt"), root.join("escape.txt"))
            .unwrap();
        assert!(
            !http_request(public, &request("/escape.txt", true))
                .await
                .contains("outside secret")
        );
    }
    running.agent.shutdown();
    running.server.shutdown_and_wait().await.unwrap();
    let geo = felucca_core::geodata::GeoResolver::default();
    let authority = felucca_service::remote::RemoteAuthority::new(&config.remote, &geo).unwrap();
    // Replace the ambient path after opening the capability. It must retain the
    // old directory, and never accidentally serve the replacement directory.
    // Windows refuses to rename a directory while the capability holds it open,
    // which rules out the swap by itself.
    #[cfg(unix)]
    {
        std::fs::rename(&root, files.root.join("retained")).unwrap();
        std::fs::create_dir(&root).unwrap();
    }
    #[cfg(windows)]
    assert!(std::fs::rename(&root, files.root.join("retained")).is_err());
    let mut managed = managed;
    managed.validate(&geo).unwrap();
    let network = Arc::new(
        felucca_transport::network::Network::new(&Default::default(), Duration::from_secs(1))
            .unwrap(),
    );
    assert!(
        felucca_service::Services::with_authority(
            &managed.tunnels,
            &Default::default(),
            network.clone(),
            Some(&authority)
        )
        .is_ok()
    );
    managed.tunnels[0].file.as_mut().unwrap().read_only = false;
    assert!(
        felucca_service::Services::with_authority(
            &managed.tunnels,
            &Default::default(),
            network.clone(),
            Some(&authority)
        )
        .is_err()
    );
    managed.tunnels[0].file.as_mut().unwrap().read_only = true;
    managed.tunnels[0].file.as_mut().unwrap().root = "unknown".into();
    assert!(
        felucca_service::Services::with_authority(
            &managed.tunnels,
            &Default::default(),
            network,
            Some(&authority)
        )
        .is_err()
    );
}

#[tokio::test]
async fn console_scope_replaces_the_local_boundary_only_after_agent_opt_in() {
    let backend = web_backend("console-scope-backend").await;
    let blocked = TargetPolicy {
        ports: vec![1],
        ..Default::default()
    };
    let opened = TargetPolicy {
        allow: vec!["ip:127.0.0.0/8".into(), "domain:localhost".into()],
        ..Default::default()
    };
    // Without opt-in the agent refuses the console scope and serves nothing.
    let port = free_port().await;
    let mut managed = desired(port, backend);
    managed.target_policy = Some(opened.clone());
    let mut server = server_config();
    server.managed.insert("features".into(), managed);
    let server = Arc::new(felucca_server::Server::new(server).unwrap());
    let bridge = server.clone().run().await.unwrap();
    let mut config = agent("");
    config.server = format!("127.0.0.1:{bridge}");
    config.transport = TransportKind::Tls;
    config.trust = felucca_core::TrustPolicy::Pinned {
        fingerprint: server.fingerprint(),
    };
    config.remote.enabled = true;
    config.remote.target_policy = blocked.clone();
    let refused = Arc::new(Agent::new(config).unwrap());
    let task = tokio::spawn(refused.clone().run());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!refused.is_ready());
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    refused.shutdown();
    task.abort();
    server.shutdown_and_wait().await.unwrap();

    // With opt-in the console scope replaces a local boundary that would block it.
    let port = free_port().await;
    let mut managed = desired(port, backend);
    managed.target_policy = Some(opened);
    let mut server = server_config();
    server.managed.insert("features".into(), managed);
    let mut config = agent("");
    config.remote.enabled = true;
    config.remote.console_scope = true;
    config.remote.target_policy = blocked.clone();
    let running = start(server, config, TransportKind::Tls).await;
    assert!(body(port).await.contains("console-scope-backend"));
    running.agent.shutdown();
    running.server.shutdown_and_wait().await.unwrap();

    // An opted-in agent keeps its own boundary when the console sends none.
    let untouched = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = free_port().await;
    let mut server = server_config();
    server.managed.insert(
        "features".into(),
        desired(port, untouched.local_addr().unwrap().port()),
    );
    let mut config = agent("");
    config.remote.enabled = true;
    config.remote.console_scope = true;
    config.remote.target_policy = blocked;
    let running = start(server, config, TransportKind::Tls).await;
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let _ = io
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), untouched.accept())
            .await
            .is_err()
    );
    running.agent.shutdown();
    running.server.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn an_agent_started_from_a_launch_uri_runs_only_server_managed_services() {
    let backend = web_backend("uri-managed-backend").await;
    let port = free_port().await;
    let mut managed = desired(port, backend);
    // The URI allows all destinations; the console can narrow it to these targets.
    managed.target_policy = Some(TargetPolicy {
        allow: vec!["ip:127.0.0.0/8".into(), "domain:localhost".into()],
        ..Default::default()
    });
    let mut server = server_config();
    server.managed.insert("features".into(), managed);
    let server = Arc::new(felucca_server::Server::new(server).unwrap());
    let bridge = server.clone().run().await.unwrap();
    let vkey = server_config().clients["features"].vkey.expose().to_owned();
    let uri = format!(
        "felucca://127.0.0.1:{bridge}?key_id=features&vkey={}&transport=tls&fingerprint={}&remote=1",
        url::form_urlencoded::byte_serialize(vkey.as_bytes()).collect::<String>(),
        server.fingerprint(),
    );
    let config: felucca_core::AgentConfig = felucca_cli::config_source::load(&uri, false, None)
        .await
        .unwrap();
    assert!(config.tunnels.is_empty());
    let agent = Arc::new(Agent::new(config).unwrap());
    let task = tokio::spawn(agent.clone().run());
    ready(&agent).await;
    assert!(body(port).await.contains("uri-managed-backend"));
    agent.shutdown();
    task.abort();
    server.shutdown_and_wait().await.unwrap();
}

#[test]
fn remote_opt_in_requires_confidential_verified_transports() {
    for transport in [TransportKind::Tcp, TransportKind::Ws, TransportKind::Kcp] {
        let mut config = agent("");
        config.remote.enabled = true;
        config.transport = transport;
        assert!(config.validate().is_err());
    }
    let mut config = agent("");
    config.remote.enabled = true;
    config.trust = TrustPolicy::Insecure;
    assert!(config.validate().is_err());
}
