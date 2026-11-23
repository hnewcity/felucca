use super::*;
use felucca_core::config::network::{NetworkConfig, ProxyCredentials, ProxyKind, UpstreamProxy};
#[path = "../../../felucca-transport/tests/support/dns.rs"]
mod dns;

async fn upstream(kind: ProxyKind) {
    let proxy_port = free_port().await;
    let mode = if kind == ProxyKind::Http {
        "httpproxy"
    } else {
        "socks5"
    };
    let _proxy = start(server_config(), agent(&format!(
        "[[tunnel]]\nmode='{mode}'\nport={proxy_port}\nbind='127.0.0.1'\n[tunnel.options.auth.users]\nupstream='proxy-password'"
    )), TransportKind::Tls).await;
    let network = NetworkConfig {
        local_ip: Some("127.0.0.1".parse().unwrap()),
        proxy: Some(UpstreamProxy {
            kind,
            server: format!("127.0.0.1:{proxy_port}"),
            remote_dns: true,
            auth: Some(ProxyCredentials {
                username: "upstream".into(),
                password: "proxy-password".into(),
            }),
        }),
        ..Default::default()
    };
    for transport in [
        TransportKind::Tcp,
        TransportKind::Tls,
        TransportKind::Ws,
        TransportKind::Wss,
    ] {
        let backend = echo().await;
        let port = free_port().await;
        let mut config = agent(&format!(
            "[[tunnel]]\nmode='tcp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{backend}'}}]"
        ));
        config.network = network.clone();
        let _running = start(server_config(), config, transport).await;
        roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
    }
    let mut wrong = network.clone();
    wrong
        .proxy
        .as_mut()
        .unwrap()
        .auth
        .as_mut()
        .unwrap()
        .password = "wrong".into();
    let error = felucca_transport::network::Network::new(&wrong, Duration::from_secs(2))
        .unwrap()
        .bridge_tcp("127.0.0.1:9")
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    let mut invalid = agent("");
    invalid.network = network;
    invalid.transport = TransportKind::Quic;
    assert!(invalid.validate().is_err());
}

#[tokio::test]
async fn upstream_http_connect_carries_tcp_tls_ws_and_wss_with_auth() {
    upstream(ProxyKind::Http).await;
}
#[tokio::test]
async fn upstream_socks5_carries_tcp_tls_ws_and_wss_with_auth() {
    upstream(ProxyKind::Socks5).await;
}

#[tokio::test]
async fn custom_dns_reaches_bridge_backends_and_probes_without_bypassing_target_acl() {
    let dns = dns::Dns::start(true).await;
    for transport in [TransportKind::Tls, TransportKind::Quic, TransportKind::Kcp] {
        let backend = echo().await;
        let port = free_port().await;
        let socks = free_port().await;
        let mut server_config = server_config();
        let quic = if matches!(transport, TransportKind::Quic | TransportKind::Kcp) {
            let port = free_port().await;
            if transport == TransportKind::Kcp {
                server_config.bridge.kcp_bind = Some(format!("127.0.0.1:{port}"));
            } else {
                server_config.bridge.quic_bind = Some(format!("127.0.0.1:{port}"));
            }
            Some(port)
        } else {
            None
        };
        let server = Arc::new(Server::new(server_config).unwrap());
        let bridge = server.clone().run().await.unwrap();
        let mut config = agent(&format!(
            r#"
[[tunnel]]
mode='tcp'
port={port}
bind='127.0.0.1'
balance='failover'
backend=[{{addr='backend.test:{backend}'}}, {{addr='health.test:{backend}'}}]
[tunnel.health]
type='tcp'
interval='100ms'
timeout='80ms'
[[tunnel]]
mode='socks5'
port={socks}
bind='127.0.0.1'
[tunnel.target_policy]
deny=['127.0.0.0/8', '::1']
"#
        ));
        config.server = format!("bridge.test:{}", quic.unwrap_or(bridge));
        config.transport = transport;
        config.trust = TrustPolicy::Pinned {
            fingerprint: server.fingerprint(),
        };
        config.network = NetworkConfig {
            dns: vec![dns.address],
            local_ip: Some("127.0.0.1".parse().unwrap()),
            forward_local_ip: true,
            ..Default::default()
        };
        let agent = Arc::new(Agent::new(config).unwrap());
        let agent_task = tokio::spawn(agent.clone().run());
        let running = Running {
            server,
            agent,
            agent_task,
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            while !running.agent.is_ready() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
        let mut io = TcpStream::connect(("127.0.0.1", socks)).await.unwrap();
        io.write_all(&[5, 1, 0]).await.unwrap();
        let mut method = [0; 2];
        io.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 0]);
        let name = b"blocked.test";
        let mut connect = vec![5, 1, 0, 3, name.len() as u8];
        connect.extend_from_slice(name);
        connect.extend_from_slice(&backend.to_be_bytes());
        io.write_all(&connect).await.unwrap();
        let mut denied = [0; 4];
        tokio::time::timeout(Duration::from_secs(5), io.read_exact(&mut denied))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            denied[1], 2,
            "DNS resolution cannot bypass the destination deny rule"
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while !dns
                .hits
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == "health.test")
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    let hits = dns.hits.lock().unwrap();
    for name in ["bridge.test", "backend.test", "health.test", "blocked.test"] {
        assert!(hits.iter().any(|hit| hit == name));
    }
    assert!(dns.tcp_hits.load(std::sync::atomic::Ordering::SeqCst) > 0);
}
