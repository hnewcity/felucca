use super::*;
#[tokio::test]
async fn port_mappings_expand_into_working_tcp_and_udp_listeners_with_udp_proxy_v2() {
    let echo = echo().await;
    let udp = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let target = udp.local_addr().unwrap().port();
    let udp_backend = udp.clone();
    let task = tokio::spawn(async move {
        let mut buffer = [0; 1024];
        let (n, peer) = udp_backend.recv_from(&mut buffer).await.unwrap();
        let (metadata, length) = felucca_transport::proxy_protocol::decode(&buffer[..n]).unwrap();
        let metadata = metadata.unwrap();
        assert!(metadata.source.ip().is_loopback());
        assert_eq!(buffer[13], 0x12, "PROXY v2 must identify a datagram");
        assert_eq!(&buffer[length..n], b"datagram");
        udp_backend.send_to(&buffer[length..n], peer).await.unwrap();
    });
    let first = free_port().await;
    let second = free_port().await;
    let udp_port = free_port().await;
    // Repeated targets are valid across separate mappings; within one mapping
    // source/target lists are ordered and unambiguous.
    let config = agent(&format!(
        r#"
[[mapping]]
mode = "tcp"
server_ports = "{first}"
target_ports = "{echo}"
[[mapping]]
mode = "tcp"
server_ports = "{second}"
target_ports = "{echo}"
[[mapping]]
mode = "udp"
bind = "127.0.0.1"
server_ports = "{udp_port}"
target_ports = "{target}"
[mapping.options]
proxy_protocol = "v2"
"#
    ));
    let _running = start(server_config(), config, TransportKind::Tls).await;
    for port in [first, second] {
        roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
    }
    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client
        .send_to(b"datagram", ("127.0.0.1", udp_port))
        .await
        .unwrap();
    let mut reply = [0; 1024];
    let (n, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&reply[..n], b"datagram");
    task.await.unwrap();
}
#[tokio::test]
async fn every_bridge_transport_carries_real_forwarded_traffic() {
    let backend = echo().await;
    for transport in [
        TransportKind::Tcp,
        TransportKind::Tls,
        TransportKind::Ws,
        TransportKind::Wss,
        TransportKind::Quic,
    ] {
        let port = free_port().await;
        let config = agent(&format!(
            "[[tunnel]]\nmode='tcp'\nport={port}\nbind='127.0.0.1'\n[[tunnel.backend]]\naddr='127.0.0.1:{backend}'"
        ));
        let running = start(server_config(), config, transport).await;
        let io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        roundtrip(io).await;
        drop(running);
    }
}
#[tokio::test]
async fn udp_preserves_empty_large_packets_and_peer_isolation() {
    let backend = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let address = backend.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buffer = vec![0; 65_536];
        loop {
            let (n, peer) = backend.recv_from(&mut buffer).await.unwrap();
            backend.send_to(&buffer[..n], peer).await.unwrap();
        }
    });
    let reservation = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let _running = start(server_config(), agent(&format!("[[tunnel]]\nmode='udp'\nport={port}\nbind='127.0.0.1'\n[[tunnel.backend]]\naddr='{address}'")), TransportKind::Quic).await;
    let mut tasks = tokio::task::JoinSet::new();
    for tag in 1..=3 {
        tasks.spawn(async move {
            let client = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
            client.connect(("127.0.0.1", port)).await.unwrap();
            for size in [0, 1, 50000, 13] {
                let packet = vec![tag; size];
                client.send(&packet).await.unwrap();
                let mut received = vec![0; 65_536];
                let n = tokio::time::timeout(Duration::from_secs(5), client.recv(&mut received))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(&received[..n], &packet);
            }
        });
    }
    while let Some(task) = tasks.join_next().await {
        task.unwrap();
    }
}
#[tokio::test]
async fn a_reconnected_agent_can_reclaim_its_listener() {
    let backend = echo().await;
    let port = free_port().await;
    let config = agent(&format!(
        "[[tunnel]]\nmode='tcp'\nport={port}\nbind='127.0.0.1'\n[[tunnel.backend]]\naddr='127.0.0.1:{backend}'"
    ));
    let running = start(server_config(), config, TransportKind::Tls).await;
    let old = running.server.registry().get("features").unwrap();
    old.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if running
                .server
                .registry()
                .get("features")
                .is_some_and(|c| c.id != old.id)
                && running.agent.is_ready()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
}
#[tokio::test]
async fn a_public_reset_releases_an_idle_backend_and_the_connection_slot() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = listener.local_addr().unwrap();
    let port = free_port().await;
    let mut server = server_config();
    server.clients.get_mut("features").unwrap().max_connections = Some(1);
    let running = start(server, agent(&format!(
        "[[tunnel]]\nmode='tcp'\nport={port}\nbind='127.0.0.1'\n[[tunnel.backend]]\naddr='{backend}'"
    )), TransportKind::Tls).await;
    let mut public = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut backend, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    public.write_all(b"first").await.unwrap();
    let mut request = [0; 5];
    backend.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"first");
    socket2::SockRef::from(&public)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(public);
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut byte = [0; 1];
        assert_eq!(
            backend.read(&mut byte).await.unwrap(),
            0,
            "the abandoned backend must close"
        );
        while running
            .server
            .registry()
            .get("features")
            .unwrap()
            .live_connections()
            != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("RST left the backend or connection slot alive");
    let mut public = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut backend, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    public.write_all(b"next").await.unwrap();
    let mut request = [0; 4];
    backend.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"next");
}
