use super::*;

#[tokio::test]
async fn snappy_and_encryption_work_on_all_bridge_transports_and_meter_plain_bytes() {
    let backend = echo().await;
    for transport in [
        TransportKind::Tcp,
        TransportKind::Tls,
        TransportKind::Ws,
        TransportKind::Wss,
        TransportKind::Quic,
        TransportKind::Kcp,
    ] {
        for (compress, encrypt) in [(true, false), (false, true), (true, true)] {
            let port = free_port().await;
            let config = agent(&format!(
                "[[tunnel]]\nmode='tcp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{backend}'}}]\n[tunnel.options.payload]\ncompress={compress}\nencrypt={encrypt}"
            ));
            let running = start(server_config(), config, transport).await;
            let io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let (mut read, mut write) = io.into_split();
            let sent: Vec<u8> = (0..2 * 1024 * 1024)
                .map(|index| (index % 251) as u8)
                .collect();
            let mut received = Vec::new();
            tokio::time::timeout(Duration::from_secs(15), async {
                let (sent, read) = tokio::join!(
                    async {
                        write.write_all(&sent).await?;
                        write.shutdown().await
                    },
                    read.read_to_end(&mut received),
                );
                sent.unwrap();
                read.unwrap();
            })
            .await
            .unwrap();
            assert_eq!(
                received, sent,
                "{transport:?} compress={compress} encrypt={encrypt}"
            );
            let client = running.server.registry().get("features").unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while client.traffic.total() != 2 * sent.len() as u64 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
    }
}

#[tokio::test]
async fn encoded_datagrams_keep_boundaries_empty_packets_and_association_isolation() {
    let socket = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let backend = socket.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let mut buffer = vec![0; 65536];
        loop {
            let (size, source) = socket.recv_from(&mut buffer).await.unwrap();
            socket.send_to(&buffer[..size], source).await.unwrap();
        }
    });
    for transport in [TransportKind::Tcp, TransportKind::Quic, TransportKind::Kcp] {
        let port = free_port().await;
        let config = agent(&format!(
            "[[tunnel]]\nmode='udp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='{backend}'}}]\n[tunnel.options.payload]\ncompress=true\nencrypt=true"
        ));
        let _running = start(server_config(), config, transport).await;
        let mut tasks = tokio::task::JoinSet::new();
        for tag in [1, 2, 3] {
            tasks.spawn(async move {
                let io = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
                io.connect(("127.0.0.1", port)).await.unwrap();
                for size in [0, 1, 50000, 32] {
                    let sent = vec![tag; size];
                    io.send(&sent).await.unwrap();
                    let mut received = vec![0; 65536];
                    let count =
                        tokio::time::timeout(Duration::from_secs(3), io.recv(&mut received))
                            .await
                            .unwrap()
                            .unwrap();
                    assert_eq!(&received[..count], sent);
                }
            });
        }
        while let Some(task) = tasks.join_next().await {
            task.unwrap();
        }
    }
    backend_task.abort();
}

#[tokio::test]
async fn encoded_proxy_routes_still_enforce_destination_acl_and_proxy_protocol() {
    let backend = echo().await;
    let port = free_port().await;
    let config = agent(&format!(
        "[[tunnel]]\nmode='socks5'\nport={port}\nbind='127.0.0.1'\n[tunnel.target_policy]\ndeny=['127.0.0.0/8']\n[tunnel.options.payload]\ncompress=true\nencrypt=true"
    ));
    let _running = start(server_config(), config, TransportKind::Tcp).await;
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(&[5, 1, 0]).await.unwrap();
    let mut reply = [0; 2];
    io.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, [5, 0]);
    let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
    request.extend_from_slice(&backend.to_be_bytes());
    io.write_all(&request).await.unwrap();
    io.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, [5, 2]);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n") {
            header.push(io.read_u8().await.unwrap());
        }
        assert!(header.starts_with(b"PROXY TCP4 127.0.0.1 127.0.0.1 "));
        let (mut read, mut write) = io.split();
        tokio::io::copy(&mut read, &mut write).await.unwrap();
    });
    let port = free_port().await;
    let config = agent(&format!(
        "[[tunnel]]\nmode='tcp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='{backend}'}}]\n[tunnel.options]\nproxy_protocol='v1'\n[tunnel.options.payload]\ncompress=true\nencrypt=true"
    ));
    let _running = start(server_config(), config, TransportKind::Quic).await;
    roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
    task.await.unwrap();
}
