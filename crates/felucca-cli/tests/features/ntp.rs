use super::*;
use felucca_core::config::NtpConfig;
#[path = "../../../felucca-transport/tests/support/ntp.rs"]
mod ntp;

#[tokio::test]
async fn calibrated_clocks_authenticate_both_sides_and_reject_uncalibrated_peers() {
    let ntp = ntp::Ntp::start(600, true).await;
    let config = NtpConfig {
        server: ntp.address.to_string(),
        interval: Duration::from_secs(1),
        timeout: Duration::from_millis(200),
        max_offset: Duration::from_secs(3600),
    };
    for transport in [TransportKind::Tls, TransportKind::Quic] {
        let backend = echo().await;
        let port = free_port().await;
        let mut agent = agent(&format!(
            "[[tunnel]]\nmode='tcp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{backend}'}}]"
        ));
        agent.ntp = Some(config.clone());
        let mut server_config = server_config();
        server_config.ntp = Some(config.clone());
        server_config.bridge.strict_clock = true;
        let running = start(server_config, agent, transport).await;
        roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
        let count = ntp.hits.load(std::sync::atomic::Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(3), async {
            while ntp.hits.load(std::sync::atomic::Ordering::SeqCst) < count + 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        running.agent.shutdown();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !running.agent_task.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    // Real bridge handshake: server calibrated 10 minutes ahead rejects a
    // normal system-time proof, even when the peer has the correct key.
    let mut server_config = server_config();
    server_config.ntp = Some(config);
    server_config.bridge.strict_clock = true;
    let server = Arc::new(Server::new(server_config).unwrap());
    let bridge = server.clone().run().await.unwrap();
    let mut io = TcpStream::connect(("127.0.0.1", bridge)).await.unwrap();
    assert!(
        felucca_proto::session::connect_info(
            &mut io,
            "features",
            &"feature-secret".into(),
            felucca_proto::ConnectionRole::Tunnel,
        )
        .await
        .is_err()
    );
    server.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn unavailable_ntp_falls_back_and_agent_shutdown_interrupts_initial_query() {
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let config = NtpConfig {
        server: silent.local_addr().unwrap().to_string(),
        interval: Duration::from_secs(40),
        timeout: Duration::from_millis(30),
        max_offset: Duration::from_secs(3600),
    };
    let mut server = server_config();
    server.ntp = Some(config.clone());
    server.bridge.strict_clock = true;
    let mut agent_config = agent("");
    agent_config.ntp = Some(config.clone());
    let running = start(server, agent_config, TransportKind::Tls).await;
    assert!(running.agent.is_ready());
    let mut packet = [0; 48];
    while silent.try_recv_from(&mut packet).is_ok() {}
    let mut pending = agent("");
    pending.ntp = Some(NtpConfig {
        timeout: Duration::from_secs(30),
        ..config
    });
    let pending = Arc::new(Agent::new(pending).unwrap());
    let task = tokio::spawn(pending.clone().run());
    // Observe this agent's initial query before cancelling its pending receive.
    silent.recv_from(&mut packet).await.unwrap();
    pending.shutdown();
    tokio::time::timeout(Duration::from_millis(200), task)
        .await
        .unwrap()
        .unwrap();
}
