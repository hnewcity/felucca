use super::*;

#[tokio::test]
async fn kcp_agent_reconnects_after_server_restart_and_rejects_wrong_keys() {
    let reserved = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let bridge = reserved.local_addr().unwrap();
    drop(reserved);
    let port = free_port().await;
    let backend = echo().await;
    let mut settings = server_config();
    settings.bridge.kcp_bind = Some(bridge.to_string());
    settings.bridge.disconnect_timeout = Duration::from_secs(2);
    let mut config = agent(&format!(
        "[[tunnel]]\nmode='tcp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{backend}'}}]"
    ));
    config.dial_timeout = Duration::from_secs(2);
    // Failed dials during the restart must not back off past the 10 s window below.
    config.reconnect_max = Duration::from_secs(1);
    let running = start(settings.clone(), config, TransportKind::Kcp).await;
    roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
    let mut wrong = felucca_transport::kcp::connect(&Default::default(), bridge, None)
        .await
        .unwrap();
    assert!(
        felucca_proto::session::connect_info(
            &mut wrong,
            "features",
            &"wrong-key".into(),
            felucca_proto::ConnectionRole::Tunnel,
        )
        .await
        .is_err()
    );
    drop(wrong);
    assert_eq!(running.server.registry().len(), 1);
    running.server.shutdown_and_wait().await.unwrap();
    let replacement = Arc::new(Server::new(settings).unwrap());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if replacement.clone().run().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while replacement.registry().is_empty() || !running.agent.is_ready() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
    running.agent.shutdown();
    replacement.shutdown_and_wait().await.unwrap();
}

#[test]
fn kcp_rejects_conflicting_udp_binds_and_tcp_upstream_proxies() {
    for http3 in [false, true] {
        let mut config = server_config();
        config.bridge.kcp_bind = Some("127.0.0.1:9999".into());
        if http3 {
            config.http = Some(HttpProxyConfig {
                http3_bind: Some("127.0.0.1:9999".into()),
                ..Default::default()
            });
        } else {
            config.bridge.quic_bind = Some("127.0.0.1:9999".into());
        }
        assert!(config.validate().is_err());
    }
    let mut config = agent("");
    config.transport = TransportKind::Kcp;
    config.network.proxy = Some(felucca_core::config::network::UpstreamProxy {
        kind: felucca_core::config::network::ProxyKind::Http,
        server: "127.0.0.1:1080".into(),
        auth: None,
        remote_dns: true,
    });
    assert!(config.validate().is_err());
}
