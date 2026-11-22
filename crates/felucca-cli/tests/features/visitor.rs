use super::*;
use crate::support::visitor::*;
use felucca_proto::{read_frame, write_frame};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
#[tokio::test]
async fn private_tcp_on_verified_transports_preserves_bulk_half_close_and_both_meters() {
    use felucca_server::resources::ResourceKey;
    for transport in [TransportKind::Tls, TransportKind::Wss, TransportKind::Quic] {
        let rig = rig(
            server_config(),
            provider("tcp", echo().await),
            visitor("tcp").await,
            transport,
        )
        .await;
        let specs = rig.server.tunnel_specs().await;
        assert_eq!(specs.len(), 1);
        assert!(!specs[0].1.public);
        assert_eq!(specs[0].1.port, 0);
        let data: Vec<u8> = (0..2 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let stream = TcpStream::connect(&rig.bind).await.unwrap();
        let (mut read, mut write) = stream.into_split();
        let send = async {
            write.write_all(&data).await.unwrap();
            write.shutdown().await.unwrap();
        };
        let receive = async {
            let mut response = Vec::new();
            read.read_to_end(&mut response).await.unwrap();
            assert_eq!(response, data);
        };
        tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(send, receive)
        })
        .await
        .unwrap();
        released(&rig.server).await;
        for key in [
            ResourceKey::Client {
                name: "features".into(),
            },
            ResourceKey::Client {
                name: "guest".into(),
            },
            ResourceKey::Tunnel {
                client: "features".into(),
                id: 1,
            },
        ] {
            let stats = rig.server.registry().store.stats(&key).unwrap();
            assert_eq!(stats.inlet, data.len() as u64, "{key:?}");
            assert_eq!(stats.export, data.len() as u64, "{key:?}");
        }
        rig.server.shutdown_and_wait().await.unwrap();
    }
}
#[tokio::test]
async fn private_udp_preserves_empty_large_and_multiple_peer_packets_and_expires() {
    let backend = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let port = backend.local_addr().unwrap().port();
    let echo = tokio::spawn(async move {
        let mut bytes = vec![0; 65536];
        while let Ok((n, peer)) = backend.recv_from(&mut bytes).await {
            backend.send_to(&bytes[..n], peer).await.unwrap();
        }
    });
    let rig = rig(
        server_config(),
        provider("udp", port),
        visitor("udp").await,
        TransportKind::Tls,
    )
    .await;
    let mut sessions = tokio::task::JoinSet::new();
    for marker in 0..4u8 {
        let bind = rig.bind.clone();
        sessions.spawn(async move {
            let socket = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
            let mut response = vec![0; 65536];
            for size in [0, 1, 50000, 13, 0] {
                let packet = vec![marker; size];
                socket.send_to(&packet, &bind).await.unwrap();
                let (n, _) =
                    tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut response))
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(&response[..n], packet);
            }
        });
    }
    while let Some(result) = sessions.join_next().await {
        result.unwrap();
    }
    released(&rig.server).await;
    for key in ["features", "guest"] {
        let stats = rig.server.registry().get(key).unwrap().traffic.stats();
        assert_eq!(stats.inlet, 4 * 50014);
        assert_eq!(stats.export, 4 * 50014);
    }
    rig.server.shutdown_and_wait().await.unwrap();
    echo.abort();
}
#[tokio::test]
async fn secret_identity_source_rules_and_both_client_quotas_are_enforced() {
    for case in [
        "secret",
        "identity",
        "share_acl",
        "visitor_acl",
        "provider_acl",
        "visitor_quota",
        "provider_quota",
        "local_acl",
    ] {
        let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = server_config();
        let mut grant = share();
        let mut local = visitor("tcp").await;
        let mut guest = config.clients["features"].clone();
        guest.vkey = "guest-vkey".into();
        match case {
            "secret" => local.secret = "an-incorrect-share-secret".into(),
            "identity" => grant.allow_clients = vec!["features".into()],
            "share_acl" => {
                grant.acl_mode = felucca_core::AclMode::Allow;
                grant.acl_rules = "192.0.2.0/24".into();
            }
            "visitor_acl" => {
                guest.acl_mode = felucca_core::AclMode::Allow;
                guest.acl_rules = "192.0.2.0/24".into();
            }
            "provider_acl" => {
                let c = config.clients.get_mut("features").unwrap();
                c.acl_mode = felucca_core::AclMode::Allow;
                c.acl_rules = "192.0.2.0/24".into();
            }
            "visitor_quota" => guest.quota.flow_limit = Some(0),
            "provider_quota" => {
                config.clients.get_mut("features").unwrap().quota.flow_limit = Some(0)
            }
            "local_acl" => {
                local.acl_mode = felucca_core::AclMode::Allow;
                local.acl_rules = "192.0.2.0/24".into();
            }
            _ => unreachable!(),
        }
        config.clients.insert("guest".into(), guest);
        config.shares.insert("office".into(), grant);
        let rig = rig(
            config,
            provider("tcp", backend.local_addr().unwrap().port()),
            local,
            TransportKind::Tls,
        )
        .await;
        rejected(&rig.bind).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), backend.accept())
                .await
                .is_err(),
            "{case}"
        );
        released(&rig.server).await;
        rig.server.shutdown_and_wait().await.unwrap();
    }
}
#[tokio::test]
async fn dynamic_share_checks_destination_acl_and_does_not_bypass_proxy_auth() {
    for denied in [false, true] {
        let backend = echo().await;
        let mut config = server_config();
        let mut grant = share();
        grant.dynamic = true;
        config.shares.insert("office".into(), grant);
        let mut source = provider("socks5", 0);
        if denied {
            source.tunnels[0].target_policy.deny = vec!["127.0.0.0/8".into()];
        }
        let mut local = visitor("tcp").await;
        local.target = Some(format!("localhost:{backend}"));
        let rig = rig(config, source, local, TransportKind::Tls).await;
        if denied {
            rejected(&rig.bind).await;
        } else {
            roundtrip(TcpStream::connect(&rig.bind).await.unwrap()).await;
        }
        rig.server.shutdown_and_wait().await.unwrap();
    }
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = server_config();
    let mut grant = share();
    grant.dynamic = true;
    config.shares.insert("office".into(), grant);
    let mut source = provider("socks5", 0);
    source.tunnels[0]
        .options
        .auth
        .users
        .insert("reader".into(), "another-password".into());
    let mut local = visitor("tcp").await;
    local.target = Some(backend.local_addr().unwrap().to_string());
    let rig = rig(config, source, local, TransportKind::Tls).await;
    rejected(&rig.bind).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), backend.accept())
            .await
            .is_err()
    );
    rig.server.shutdown_and_wait().await.unwrap();
}
#[tokio::test]
async fn visitor_file_share_keeps_file_auth_read_only_and_directory_confinement() {
    let files = identity("visitor-file.test").await;
    let root = files.root.join("public");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("readme"), "private file content").unwrap();
    std::fs::write(files.root.join("outside"), "outside secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(files.root.join("outside"), root.join("escape")).unwrap();
    let source = agent(&format!(
        "[[tunnel]]\nid=1\nmode='file'\npublic=false\n[tunnel.file]\nroot='{}'\nread_only=true\n[tunnel.file.auth.users]\nreader='password'",
        root.display()
    ));
    let rig = rig(
        server_config(),
        source,
        visitor("tcp").await,
        TransportKind::Tls,
    )
    .await;
    let port: SocketAddr = rig.bind.parse().unwrap();
    let no_auth = http_request(
        port.port(),
        "GET /readme HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(no_auth.starts_with("HTTP/1.1 401"));
    let get = |path| {
        format!(
            "GET /{path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic cmVhZGVyOnBhc3N3b3Jk\r\nConnection: close\r\n\r\n"
        )
    };
    assert!(
        http_request(port.port(), &get("readme"))
            .await
            .contains("private file content")
    );
    assert!(
        !http_request(port.port(), &get("escape"))
            .await
            .contains("outside secret")
    );
    let put = http_request(port.port(), "PUT /new HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic cmVhZGVyOnBhc3N3b3Jk\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad").await;
    assert!(!put.starts_with("HTTP/1.1 201"));
    assert!(!root.join("new").exists());
    rig.server.shutdown_and_wait().await.unwrap();
}
#[tokio::test]
async fn replay_and_plaintext_visitors_cannot_open_a_private_service() {
    use felucca_proto::visitor::{Side, VisitChallenge, VisitRequest, VisitResult};
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rig = rig(
        server_config(),
        provider("tcp", backend.local_addr().unwrap().port()),
        visitor("tcp").await,
        TransportKind::Tls,
    )
    .await;
    // Free the identity so its ordinary reconnect loop cannot replace our peer.
    rig._visitor.agent.shutdown();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let request = VisitRequest {
        connection: Default::default(),
        share: "office".into(),
        nonce: [7; 32],
        datagram: false,
        target: None,
    };
    let tunnel = raw_guest(&rig.server, &rig.address, true).await;
    let mut first = tunnel.open().await.unwrap();
    write_frame(
        &mut first,
        &felucca_proto::StreamRequest::Visit(request.clone()),
    )
    .await
    .unwrap();
    let old: VisitChallenge = read_frame(&mut first).await.unwrap();
    let proof =
        felucca_proto::visitor::prove(SECRET.as_bytes(), &request, &old, "guest", Side::Visitor);
    drop(first);
    let mut second = tunnel.open().await.unwrap();
    write_frame(
        &mut second,
        &felucca_proto::StreamRequest::Visit(request.clone()),
    )
    .await
    .unwrap();
    let fresh: VisitChallenge = read_frame(&mut second).await.unwrap();
    assert_ne!(old.nonce, fresh.nonce);
    write_frame(&mut second, &proof).await.unwrap();
    assert!(matches!(
        read_frame::<_, VisitResult>(&mut second).await.unwrap(),
        VisitResult::Err(_)
    ));
    tunnel.close(felucca_mux::CloseReason::Shutdown);
    let tunnel = raw_guest(&rig.server, &rig.address, false).await;
    let mut stream = tunnel.open().await.unwrap();
    write_frame(
        &mut stream,
        &felucca_proto::StreamRequest::Visit(request.clone()),
    )
    .await
    .unwrap();
    let fresh: VisitChallenge = read_frame(&mut stream).await.unwrap();
    let proof =
        felucca_proto::visitor::prove(SECRET.as_bytes(), &request, &fresh, "guest", Side::Visitor);
    write_frame(&mut stream, &proof).await.unwrap();
    assert!(matches!(
        read_frame::<_, VisitResult>(&mut stream).await.unwrap(),
        VisitResult::Err(_)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), backend.accept())
            .await
            .is_err()
    );
    tunnel.close(felucca_mux::CloseReason::Shutdown);
    rig.server.shutdown_and_wait().await.unwrap();
}
#[tokio::test]
async fn local_port_is_reserved_at_startup_and_released_when_agent_run_is_cancelled() {
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut local = visitor("tcp").await;
    local.bind = occupied.local_addr().unwrap().to_string();
    let mut config = agent("");
    config.visitors = vec![local];
    assert!(
        Agent::new(config)
            .unwrap_err()
            .to_string()
            .contains("cannot bind")
    );
    let rig = rig(
        server_config(),
        provider("tcp", echo().await),
        visitor("tcp").await,
        TransportKind::Tls,
    )
    .await;
    roundtrip(TcpStream::connect(&rig.bind).await.unwrap()).await;
    rig._visitor.task.abort();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if TcpListener::bind(&rig.bind).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    released(&rig.server).await;
    rig.server.shutdown_and_wait().await.unwrap();
}
#[tokio::test]
async fn disabling_a_client_ends_visitors_and_reenable_reconnects_without_rebinding_local_port() {
    let mut config = server_config();
    let bridge = free_port().await;
    config.bridge.bind = format!("127.0.0.1:{bridge}");
    let address = format!("127.0.0.1:{bridge}");
    let mut guest = config.clients["features"].clone();
    guest.vkey = "guest-vkey".into();
    guest.allow_config_push = false;
    config.clients.insert("guest".into(), guest);
    config.shares.insert("office".into(), share());
    let daemon = felucca_server::Daemon::start(config, None).await.unwrap();
    let source = worker(
        provider("tcp", echo().await),
        &daemon.server(),
        &address,
        TransportKind::Tls,
    )
    .await;
    let local = visitor("tcp").await;
    let bind = local.bind.clone();
    let mut config = agent("");
    config.key_id = Some("guest".into());
    config.vkey = "guest-vkey".into();
    config.visitors.push(local);
    let guest = worker(config, &daemon.server(), &address, TransportKind::Tls).await;
    roundtrip(TcpStream::connect(&bind).await.unwrap()).await;
    let mut held = TcpStream::connect(&bind).await.unwrap();
    held.write_all(b"x").await.unwrap();
    assert_eq!(held.read_u8().await.unwrap(), b'x');
    let (rev, mut next) = daemon.config();
    next.clients.get_mut("guest").unwrap().enabled = false;
    daemon.apply(next, rev).await.unwrap();
    let mut tail = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), held.read_to_end(&mut tail))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(!guest.agent.is_ready());
    assert!(daemon.server().registry().get("guest").is_none());
    assert!(
        TcpListener::bind(&bind).await.is_err(),
        "local listener must survive reconnect"
    );
    rejected(&bind).await;
    let (rev, mut next) = daemon.config();
    next.clients.get_mut("guest").unwrap().enabled = true;
    daemon.apply(next, rev).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !guest.agent.is_ready() || !source.agent.is_ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    roundtrip(TcpStream::connect(&bind).await.unwrap()).await;
    daemon.shutdown().await.unwrap();
}
#[tokio::test]
async fn visitor_connection_caps_release_after_a_reset() {
    let mut local = visitor("tcp").await;
    local.max_connections = 1;
    let rig = rig(
        server_config(),
        provider("tcp", echo().await),
        local,
        TransportKind::Tls,
    )
    .await;
    let mut held = TcpStream::connect(&rig.bind).await.unwrap();
    held.write_all(b"x").await.unwrap();
    assert_eq!(held.read_u8().await.unwrap(), b'x');
    rejected(&rig.bind).await;
    socket2::SockRef::from(&held)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(held);
    released(&rig.server).await;
    roundtrip(TcpStream::connect(&rig.bind).await.unwrap()).await;
    rig.server.shutdown_and_wait().await.unwrap();
}
#[tokio::test]
async fn socks_visitor_authenticates_connect_and_owns_its_udp_association_locally() {
    let backend = echo().await;
    let udp = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let udp_port = udp.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let mut data = vec![0; 65536];
        while let Ok((n, peer)) = udp.recv_from(&mut data).await {
            udp.send_to(&data[..n], peer).await.unwrap();
        }
    });
    let mut config = server_config();
    let mut grant = share();
    grant.dynamic = true;
    config.shares.insert("office".into(), grant);
    let mut local = visitor("tcp").await;
    local.mode = felucca_core::config::VisitorMode::Socks5;
    local.auth.users.insert("reader".into(), "pass".into());
    let rig = rig(config, provider("socks5", 0), local, TransportKind::Tls).await;
    let mut denied = TcpStream::connect(&rig.bind).await.unwrap();
    denied.write_all(&[5, 1, 0]).await.unwrap();
    let mut answer = [0; 2];
    denied.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, [5, 255]);
    async fn auth(bind: &str) -> TcpStream {
        let mut io = TcpStream::connect(bind).await.unwrap();
        io.write_all(&[5, 1, 2]).await.unwrap();
        let mut answer = [0; 2];
        io.read_exact(&mut answer).await.unwrap();
        assert_eq!(answer, [5, 2]);
        io.write_all(b"\x01\x06reader\x04pass").await.unwrap();
        io.read_exact(&mut answer).await.unwrap();
        assert_eq!(answer, [1, 0]);
        io
    }
    let mut connect = auth(&rig.bind).await;
    let mut request = vec![5, 1, 0, 3, 9];
    request.extend(b"localhost");
    request.extend(backend.to_be_bytes());
    connect.write_all(&request).await.unwrap();
    let mut response = [0; 10];
    connect.read_exact(&mut response).await.unwrap();
    assert_eq!(response[1], 0);
    roundtrip(connect).await;
    let mut control = auth(&rig.bind).await;
    control
        .write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0])
        .await
        .unwrap();
    control.read_exact(&mut response).await.unwrap();
    assert_eq!(&response[..4], &[5, 0, 0, 1]);
    let relay = SocketAddr::new(
        std::net::Ipv4Addr::new(response[4], response[5], response[6], response[7]).into(),
        u16::from_be_bytes([response[8], response[9]]),
    );
    assert_eq!(relay.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
    let socket = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let mut packet = vec![0, 0, 0, 1, 127, 0, 0, 1];
    packet.extend(udp_port.to_be_bytes());
    let mut buffer = vec![0; 65536];
    for n in [0, 50000, 13] {
        let mut packet = packet.clone();
        packet.extend(vec![42; n]);
        socket.send_to(&packet, relay).await.unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer[..n], packet);
    }
    let intruder = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    intruder.send_to(&packet, relay).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), intruder.recv_from(&mut buffer))
            .await
            .is_err()
    );
    let mut fragmented = packet.clone();
    fragmented[2] = 1;
    socket.send_to(&fragmented, relay).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), socket.recv_from(&mut buffer))
            .await
            .is_err()
    );
    drop(control);
    released(&rig.server).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if UdpSocket::bind(relay).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    rig.server.shutdown_and_wait().await.unwrap();
    task.abort();
}
