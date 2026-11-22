use super::*;
#[tokio::test]
async fn traffic_and_exhausted_quotas_survive_a_server_restart() {
    use felucca_core::{
        Quota, TrafficStats,
        config::{StateConfig, UserEntry},
        traffic::ResourceLimits,
    };
    use felucca_server::resources::ResourceKey;
    let files = identity("state.test").await;
    let public = free_port().await;
    let backend = echo().await;
    let mut server = server_config();
    server.state = Some(StateConfig {
        path: files.root.join("state.json"),
        save_interval: Duration::from_millis(40),
    });
    server.users.insert(
        "team".into(),
        UserEntry {
            limits: ResourceLimits {
                quota: Quota {
                    flow_limit: Some(10),
                    expires_at: None,
                },
                ..Default::default()
            },
            ..Default::default()
        },
    );
    server.clients.get_mut("features").unwrap().user = Some("team".into());
    let config = agent(&format!(
        r#"
[[tunnel]]
id = 42
mode = "tcp"
port = {public}
bind = "127.0.0.1"
backend = [{{addr = "127.0.0.1:{backend}"}}]
"#
    ));
    let running = start(server.clone(), config.clone(), TransportKind::Tls).await;
    roundtrip(TcpStream::connect(("127.0.0.1", public)).await.unwrap()).await;
    running.agent.shutdown();
    running.server.shutdown_and_wait().await.unwrap();
    let store = &running.server.registry().store;
    for key in [
        ResourceKey::User {
            name: "team".into(),
        },
        ResourceKey::Client {
            name: "features".into(),
        },
        ResourceKey::Tunnel {
            client: "features".into(),
            id: 42,
        },
    ] {
        assert_eq!(
            store.stats(&key),
            Some(TrafficStats {
                inlet: 5,
                export: 5
            }),
            "{key:?}"
        );
    }
    let released = Arc::downgrade(store);
    drop(running);
    tokio::time::timeout(Duration::from_secs(5), async {
        while released.upgrade().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let restarted = start(server, config, TransportKind::Tls).await;
    let mut io = TcpStream::connect(("127.0.0.1", public)).await.unwrap();
    let _ = io.write_all(b"denied after restart").await;
    let mut data = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), io.read_to_end(&mut data))
        .await
        .unwrap();
    assert!(data.is_empty());
    assert_eq!(
        restarted
            .server
            .registry()
            .get("features")
            .unwrap()
            .traffic
            .total(),
        10
    );
    restarted.server.shutdown_and_wait().await.unwrap();
}
#[tokio::test]
async fn tunnel_caps_are_independent_and_a_shared_user_cap_spans_tunnels() {
    use felucca_core::{config::UserEntry, traffic::ResourceLimits};
    let backend = echo().await;
    let ports = [free_port().await, free_port().await];
    let config = agent(&format!(
        r#"
[[tunnel]]
mode = "tcp"
port = {}
bind = "127.0.0.1"
backend = [{{addr = "127.0.0.1:{backend}"}}]
[tunnel.options.limits]
max_connections = 1
[[tunnel]]
mode = "tcp"
port = {}
bind = "127.0.0.1"
backend = [{{addr = "127.0.0.1:{backend}"}}]
[tunnel.options.limits]
max_connections = 1
"#,
        ports[0], ports[1]
    ));
    for user_cap in [None, Some(1)] {
        let mut server = server_config();
        server.users.insert(
            "team".into(),
            UserEntry {
                limits: ResourceLimits {
                    max_connections: user_cap,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        server.clients.get_mut("features").unwrap().user = Some("team".into());
        let running = start(server, config.clone(), TransportKind::Tcp).await;
        let mut held = TcpStream::connect(("127.0.0.1", ports[0])).await.unwrap();
        held.write_all(b"x").await.unwrap();
        assert_eq!(held.read_u8().await.unwrap(), b'x');
        for port in ports {
            let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            if port == ports[1] && user_cap.is_none() {
                roundtrip(io).await;
            } else {
                let _ = io.write_all(b"refused").await;
                let mut data = Vec::new();
                let _ = tokio::time::timeout(Duration::from_secs(3), io.read_to_end(&mut data))
                    .await
                    .unwrap();
                assert!(data.is_empty());
            }
        }
        drop(held);
        tokio::time::timeout(Duration::from_secs(3), async {
            while running
                .server
                .registry()
                .get("features")
                .unwrap()
                .live_connections()
                != 0
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        roundtrip(TcpStream::connect(("127.0.0.1", ports[1])).await.unwrap()).await;
        running.server.shutdown_and_wait().await.unwrap();
    }
}
#[tokio::test]
async fn host_limits_apply_after_routing_and_are_not_bypassed_by_a_rewritten_host() {
    use felucca_core::{Quota, traffic::ResourceLimits};
    let backend = web_backend("host-limits").await;
    let public = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{public}")),
        tls_bind: None,
        host_limits: [
            (
                "used.test".into(),
                ResourceLimits {
                    quota: Quota {
                        flow_limit: Some(1),
                        expires_at: None,
                    },
                    ..Default::default()
                },
            ),
            (
                "expired.test".into(),
                ResourceLimits {
                    quota: Quota {
                        expires_at: Some(1),
                        flow_limit: None,
                    },
                    ..Default::default()
                },
            ),
        ]
        .into(),
        ..Default::default()
    });
    let config = agent(&format!(
        r#"
[[tunnel]]
mode = "http"
port = {public}
hosts = ["used.test", "free.test", "expired.test"]
backend = [{{addr = "127.0.0.1:{backend}"}}]
[tunnel.options.http]
host = "same-backend.test"
"#
    ));
    let running = start(server, config, TransportKind::Tls).await;
    for host in [
        "expired.test",
        "free.test",
        "used.test",
        "used.test",
        "free.test",
    ] {
        let response = http_request(
            public,
            &format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        match host {
            "expired.test" => assert!(response.starts_with("HTTP/1.1 403"), "{response}"),
            "free.test" => assert!(response.starts_with("HTTP/1.1 200"), "{response}"),
            // The first request exhausts its byte allowance before the response;
            // subsequent requests must fail before reaching the backend.
            _ => assert!(!response.starts_with("HTTP/1.1 200"), "{response}"),
        }
    }
    let usage = running
        .server
        .registry()
        .store
        .stats(&felucca_server::resources::ResourceKey::Host {
            pattern: "used.test".into(),
        })
        .unwrap();
    assert!(usage.inlet > 0);
    assert_eq!(usage.export, 0);
}
#[tokio::test]
async fn source_acl_intersects_global_user_client_and_tunnel_without_blocking_the_agent() {
    use felucca_core::{AclMode, config::UserEntry};
    let backend = echo().await;
    for denied_layer in 0..=4 {
        let port = free_port().await;
        let mut server = server_config();
        server.acl_mode = AclMode::Allow;
        server.acl_rules = "geoip:private".into();
        server.users.insert(
            "team".into(),
            UserEntry {
                limits: Default::default(),
                acl_mode: AclMode::Allow,
                acl_rules: "geoip:private".into(),
            },
        );
        let client = server.clients.get_mut("features").unwrap();
        client.user = Some("team".into());
        client.acl_mode = AclMode::Allow;
        client.acl_rules = "geoip:private".into();
        let mut config = agent(&format!(
            r#"
[[tunnel]]
mode = "tcp"
port = {port}
bind = "127.0.0.1"
backend = [{{addr = "127.0.0.1:{backend}"}}]
acl_mode = "allow"
acl_rules = "geoip:private"
"#
        ));
        match denied_layer {
            0 => server.acl_mode = AclMode::Deny,
            1 => server.users.get_mut("team").unwrap().acl_mode = AclMode::Deny,
            2 => server.clients.get_mut("features").unwrap().acl_mode = AclMode::Deny,
            3 => config.tunnels[0].acl_mode = AclMode::Deny,
            _ => {}
        }
        let running = start(server, config, TransportKind::Tls).await;
        assert!(
            running.agent.is_ready(),
            "source ACL must not block the bridge handshake"
        );
        let mut connection = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        if denied_layer == 4 {
            roundtrip(connection).await;
        } else {
            let _ = connection.write_all(b"denied").await;
            let mut data = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(3), connection.read_to_end(&mut data))
                .await
                .unwrap();
            assert!(data.is_empty(), "layer {denied_layer} did not deny traffic");
        }
    }
}
#[tokio::test]
async fn geodata_files_drive_source_and_resolved_destination_policies() {
    // Independent protobuf fixtures: GeoIP LOOP=127/8; GeoSite LOCAL=full:localhost.
    let files = identity("geo.test").await;
    let ips = files.root.join("geoip.dat");
    let sites = files.root.join("geosite.dat");
    std::fs::write(
        &ips,
        b"\x0a\x10\x0a\x04LOOP\x12\x08\x0a\x04\x7f\x00\x00\x00\x10\x08",
    )
    .unwrap();
    std::fs::write(
        &sites,
        b"\x0a\x16\x0a\x05LOCAL\x12\x0d\x08\x03\x12\x09localhost",
    )
    .unwrap();
    let backend = echo().await;
    let allowed = free_port().await;
    let denied = free_port().await;
    let mut server = server_config();
    server.geo.geoip = Some(ips.clone());
    server.acl_mode = felucca_core::AclMode::Allow;
    server.acl_rules = "geoip:LOOP".into();
    let config = agent(&format!(
        r#"
[geo]
geoip = "{}"
geosite = "{}"
[[tunnel]]
mode = "tcp"
port = {allowed}
bind = "127.0.0.1"
backend = [{{addr = "localhost:{backend}"}}]
acl_mode = "allow"
acl_rules = "geoip:LOOP"
[tunnel.target_policy]
allow = ["geosite:LOCAL"]
[[tunnel]]
mode = "tcp"
port = {denied}
bind = "127.0.0.1"
backend = [{{addr = "localhost:{backend}"}}]
[tunnel.target_policy]
allow = ["geosite:LOCAL"]
deny = ["geoip:private"]
"#,
        ips.display(),
        sites.display()
    ));
    let _running = start(server, config, TransportKind::Quic).await;
    roundtrip(TcpStream::connect(("127.0.0.1", allowed)).await.unwrap()).await;
    let mut io = TcpStream::connect(("127.0.0.1", denied)).await.unwrap();
    let _ = io.write_all(b"forbidden").await;
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), io.read_to_end(&mut response))
        .await
        .unwrap();
    assert!(
        response.is_empty(),
        "GeoSite allow must not override a denied DNS result"
    );
}
#[tokio::test]
async fn reverse_proxy_applies_source_acl_after_trusted_proxy_header() {
    use felucca_core::config::{ProxyAccept, ProxyMode};
    let backend = web_backend("original-peer").await;
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: None,
        proxy_accept: ProxyAccept {
            mode: ProxyMode::Optional,
            trusted: vec!["127.0.0.1".into()],
        },
        ..Default::default()
    });
    let config = agent(&format!(
        r#"
[[tunnel]]
mode = "http"
port = {port}
hosts = ["source.test"]
acl_mode = "allow"
acl_rules = "192.0.2.0/24"
backend = [{{addr = "127.0.0.1:{backend}"}}]
"#
    ));
    let _running = start(server, config, TransportKind::Tls).await;
    let request = "GET / HTTP/1.1\r\nHost: source.test\r\nX-Forwarded-For: forged\r\nConnection: close\r\n\r\n";
    let denied = http_request(port, request).await;
    assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
    let allowed = http_request(
        port,
        &format!("PROXY TCP4 192.0.2.8 198.51.100.4 1234 80\r\n{request}"),
    )
    .await;
    assert!(allowed.starts_with("HTTP/1.1 200"), "{allowed}");
    assert!(
        allowed
            .to_ascii_lowercase()
            .contains("x-forwarded-for: 192.0.2.8"),
        "{allowed}"
    );
    assert!(!allowed.contains("forged"), "{allowed}");
}
#[tokio::test]
async fn destination_policy_blocks_dns_loopback_and_reports_forbidden() {
    let backend = echo().await;
    let port = free_port().await;
    let _running = start(server_config(), agent(&format!(
        "[[tunnel]]\nmode='mixedproxy'\nport={port}\nbind='127.0.0.1'\n[tunnel.target_policy]\nallow=['domain:localhost']\ndeny=['127.0.0.0/8','::1']"
    )), TransportKind::Tls).await;
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0; 2];
    io.read_exact(&mut method).await.unwrap();
    let mut request = b"\x05\x01\x00\x03\x09localhost".to_vec();
    request.extend(backend.to_be_bytes());
    io.write_all(&request).await.unwrap();
    let mut reply = [0; 10];
    io.read_exact(&mut reply).await.unwrap();
    assert_eq!(
        reply[1], 2,
        "a domain allow cannot bypass denied resolved IPs"
    );
    let response = http_request(port, &format!("CONNECT localhost:{backend} HTTP/1.1\r\nHost: localhost:{backend}\r\nConnection: close\r\n\r\n")).await;
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
}
#[tokio::test]
async fn destination_policy_allows_named_hosts_on_selected_ports() {
    let backend = echo().await;
    let port = free_port().await;
    let _running = start(server_config(), agent(&format!(
        "[[tunnel]]\nmode='httpproxy'\nport={port}\nbind='127.0.0.1'\n[tunnel.target_policy]\nallow=['domain:localhost']\nports=[{backend}]"
    )), TransportKind::Tls).await;
    for target in [format!("127.0.0.1:{backend}"), "localhost:1".into()] {
        let response = http_request(
            port,
            &format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    }
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(
        format!("CONNECT localhost:{backend} HTTP/1.1\r\nHost: localhost:{backend}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    assert!(head(&mut io).await.starts_with("HTTP/1.1 200"));
    roundtrip(io).await;
}
#[tokio::test]
async fn tunnel_acl_blocks_public_traffic_without_blocking_the_agent() {
    let port = free_port().await;
    let config = agent(&format!(
        "[[tunnel]]\nmode='socks5'\nport={port}\nbind='127.0.0.1'\nacl_mode='deny'\nacl_rules='127.0.0.0/8'"
    ));
    let running = start(server_config(), config, TransportKind::Tls).await;
    assert_eq!(running.server.registry().len(), 1);
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut byte = [0];
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), client.read(&mut byte)).await,
        Ok(Ok(0)) | Ok(Err(_))
    ));
}
