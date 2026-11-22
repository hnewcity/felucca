use super::*;
use crate::support::visitor::*;
use felucca_core::config::peer::{P2pServerConfig, VisitorConnection};
use felucca_server::resources::ResourceKey;
use std::net::SocketAddr;
use tokio::net::UdpSocket;

async fn broker_config() -> ServerConfig {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap().to_string();
    let mut config = server_config();
    config.p2p = Some(P2pServerConfig {
        bind: address.clone(),
        advertise: address,
        timeout: Duration::from_secs(2),
        max_pending: 16,
    });
    let mut shared = share();
    shared.direct = true;
    config.shares.insert("office".into(), shared);
    config.clients.get_mut("features").unwrap().max_connections = Some(1);
    config
}

async fn totals(server: &Server, bytes: u64) {
    released(server).await;
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
        let stats = server.registry().store.stats(&key).unwrap();
        assert_eq!(stats.inlet, bytes, "{key:?}");
        assert_eq!(stats.export, bytes, "{key:?}");
    }
}

#[tokio::test]
async fn revoking_one_share_ends_its_live_relay_and_direct_sessions_without_reconnecting_devices() {
    for connection in [VisitorConnection::Relay, VisitorConnection::Direct] {
        let mut config = broker_config().await;
        config.clients.get_mut("features").unwrap().max_connections = None;
        let mut guest = config.clients["features"].clone();
        guest.vkey = "guest-vkey".into();
        guest.allow_config_push = false;
        config.clients.insert("guest".into(), guest);
        config
            .shares
            .insert("unrelated".into(), config.shares["office"].clone());
        config.bridge.bind = format!("127.0.0.1:{}", free_port().await);
        let address = config.bridge.bind.clone();
        let daemon = felucca_server::Daemon::start(config, None).await.unwrap();
        let server = daemon.server();
        let mut providing = provider("tcp", echo().await);
        providing.p2p.enabled = true;
        let _provider = worker(providing, &server, &address, TransportKind::Tls).await;
        let mut first = visitor("tcp").await;
        first.connection = connection;
        let mut second = visitor("tcp").await;
        second.name = "second".into();
        second.share = "unrelated".into();
        second.connection = connection;
        let mut receiving = agent("");
        receiving.key_id = Some("guest".into());
        receiving.vkey = "guest-vkey".into();
        receiving.p2p.enabled = true;
        receiving.visitors = vec![first.clone(), second.clone()];
        let _receiver = worker(receiving, &server, &address, TransportKind::Tls).await;
        let provider_id = server.registry().get("features").unwrap().id;
        let visitor_id = server.registry().get("guest").unwrap().id;
        let mut revoked = TcpStream::connect(&first.bind).await.unwrap();
        let mut unaffected = TcpStream::connect(&second.bind).await.unwrap();
        async fn exchange(stream: &mut TcpStream) {
            stream.write_all(b"live").await.unwrap();
            let mut reply = [0; 4];
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&reply, b"live");
        }
        exchange(&mut revoked).await;
        exchange(&mut unaffected).await;
        let (revision, mut next) = daemon.config();
        next.shares.remove("office");
        daemon.apply(next, revision).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), revoked.read(&mut [0]))
            .await
            .unwrap();
        assert!(
            matches!(ended, Ok(0) | Err(_)),
            "revoked access remained live"
        );
        exchange(&mut unaffected).await;
        assert_eq!(provider_id, server.registry().get("features").unwrap().id);
        assert_eq!(visitor_id, server.registry().get("guest").unwrap().id);
        rejected(&first.bind).await;
        // Withdraw the remaining private route while the provider still owns
        // its local configuration. Enforcement must not depend on its polling.
        let (revision, mut next) = daemon.config();
        next.managed.insert("features".into(), Default::default());
        daemon.apply(next, revision).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), unaffected.read(&mut [0]))
            .await
            .unwrap();
        assert!(
            matches!(ended, Ok(0) | Err(_)),
            "withdrawn private route remained live"
        );
        assert_eq!(provider_id, server.registry().get("features").unwrap().id);
        daemon.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn direct_tcp_bulk_half_close_and_plaintext_meters_with_raw_and_encoded_payloads() {
    for encoded in [false, true] {
        let mut provider = provider("tcp", echo().await);
        provider.p2p.enabled = true;
        provider.max_connections = Some(1);
        provider.tunnels[0].options.payload.compress = encoded;
        provider.tunnels[0].options.payload.encrypt = encoded;
        let mut visitor = visitor("tcp").await;
        visitor.connection = VisitorConnection::Direct;
        let rig = rig(broker_config().await, provider, visitor, TransportKind::Tls).await;
        let data: Vec<u8> = (0..2 * 1024 * 1024).map(|n| (n % 251) as u8).collect();
        let mut stream = TcpStream::connect(&rig.bind).await.unwrap();
        let (mut read, mut write) = stream.split();
        let outbound = async {
            write.write_all(&data).await.unwrap();
            write.shutdown().await.unwrap();
        };
        let inbound = async {
            let mut got = Vec::new();
            read.read_to_end(&mut got).await.unwrap();
            assert_eq!(got, data);
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            tokio::join!(outbound, inbound);
        })
        .await
        .unwrap();
        drop(stream);
        totals(&rig.server, data.len() as u64).await;
        // A second ticket must reclaim both sides' single connection slot.
        roundtrip(TcpStream::connect(&rig.bind).await.unwrap()).await;
        totals(&rig.server, data.len() as u64 + 5).await;
    }
}

#[tokio::test]
async fn direct_udp_preserves_empty_large_datagrams_and_both_scopes() {
    let backend = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let mut provider = provider("udp", backend.local_addr().unwrap().port());
    provider.p2p.enabled = true;
    provider.tunnels[0].options.payload.compress = true;
    provider.tunnels[0].options.payload.encrypt = true;
    let echo = tokio::spawn(async move {
        let mut buffer = vec![0; 65536];
        while let Ok((n, source)) = backend.recv_from(&mut buffer).await {
            backend.send_to(&buffer[..n], source).await.unwrap();
        }
    });
    let mut visitor = visitor("udp").await;
    visitor.connection = VisitorConnection::Direct;
    let rig = rig(
        broker_config().await,
        provider,
        visitor,
        TransportKind::Quic,
    )
    .await;
    let client = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    client.connect(&rig.bind).await.unwrap();
    let mut buffer = vec![0; 65536];
    for size in [0, 1, 50000, 13] {
        let payload: Vec<_> = (0..size).map(|n| (n % 239) as u8).collect();
        client.send(&payload).await.unwrap();
        let n = tokio::time::timeout(Duration::from_secs(10), client.recv(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer[..n], &payload);
    }
    tokio::time::sleep(Duration::from_millis(1300)).await;
    totals(&rig.server, 50014).await;
    echo.abort();
}

#[tokio::test]
async fn blocked_rendezvous_and_provider_opt_out_relay_without_leaking_single_slots() {
    for opted_out in [false, true] {
        let blackhole = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut config = broker_config().await;
        config.p2p.as_mut().unwrap().advertise = blackhole.local_addr().unwrap().to_string();
        config.p2p.as_mut().unwrap().timeout = Duration::from_millis(500);
        let mut provider = provider("tcp", echo().await);
        provider.p2p.enabled = !opted_out;
        provider.max_connections = Some(1);
        let mut visitor = visitor("tcp").await;
        visitor.connection = VisitorConnection::PreferDirect;
        let rig = rig(config, provider, visitor, TransportKind::Tls).await;
        for expected in [5, 10] {
            roundtrip(TcpStream::connect(&rig.bind).await.unwrap()).await;
            totals(&rig.server, expected).await;
        }
    }
}

#[tokio::test]
async fn direct_only_refuses_unavailable_direct_path_and_source_target_access_denials() {
    let mut provider = provider("tcp", echo().await);
    // This provider did not opt in, so Direct must never silently relay.
    let mut visitor = visitor("tcp").await;
    visitor.connection = VisitorConnection::Direct;
    let fixture = rig(
        broker_config().await,
        provider.clone(),
        visitor.clone(),
        TransportKind::Tls,
    )
    .await;
    rejected(&fixture.bind).await;
    totals(&fixture.server, 0).await;
    drop(fixture);
    provider.p2p.enabled = true;
    provider.tunnels[0].acl_mode = felucca_core::AclMode::Deny;
    provider.tunnels[0].acl_rules = "127.0.0.1".into();
    let fixture = rig(broker_config().await, provider, visitor, TransportKind::Tls).await;
    rejected(&fixture.bind).await;
    totals(&fixture.server, 0).await;
}

#[tokio::test]
async fn observed_peers_with_blocked_quic_fall_back_once_and_release_provider_slots() {
    let gateway = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut config = broker_config().await;
    let broker: SocketAddr = config.p2p.as_ref().unwrap().bind.parse().unwrap();
    config.p2p.as_mut().unwrap().advertise = gateway.local_addr().unwrap().to_string();
    config.p2p.as_mut().unwrap().timeout = Duration::from_millis(500);
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = dropped.clone();
    // Both authenticated observations succeed. This deliberately advertises a
    // mapping which drops QUIC, exercising fallback after the direct offer.
    let proxy = tokio::spawn(async move {
        let mut peers = std::collections::HashMap::new();
        let mut buffer = vec![0; 65536];
        loop {
            let (n, source) = gateway.recv_from(&mut buffer).await.unwrap();
            if n != 256 || &buffer[..4] != b"\0FP1" {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            let key = buffer[5..38].to_vec();
            if source == broker {
                if let Some(target) = peers.get(&key) {
                    gateway.send_to(&buffer[..n], target).await.unwrap();
                }
            } else {
                peers.insert(key, source);
                gateway.send_to(&buffer[..n], broker).await.unwrap();
            }
        }
    });
    let mut provider = provider("tcp", echo().await);
    provider.p2p.enabled = true;
    provider.max_connections = Some(1);
    let mut visitor = visitor("tcp").await;
    visitor.connection = VisitorConnection::PreferDirect;
    let rig = rig(config, provider, visitor, TransportKind::Wss).await;
    for expected in [5, 10] {
        roundtrip(TcpStream::connect(&rig.bind).await.unwrap()).await;
        totals(&rig.server, expected).await;
    }
    assert!(dropped.load(std::sync::atomic::Ordering::Relaxed) > 0);
    rig.server.shutdown_and_wait().await.unwrap();
    proxy.abort();
}

#[tokio::test]
async fn direct_dynamic_targets_keep_resolved_ip_acl_and_proxy_auth() {
    for case in ["allowed", "denied_ip", "proxy_auth"] {
        let mut config = broker_config().await;
        config.shares.get_mut("office").unwrap().dynamic = true;
        let mut source = provider("socks5", 0);
        source.p2p.enabled = true;
        if case == "denied_ip" {
            source.tunnels[0].target_policy.deny = vec!["127.0.0.0/8".into()];
        }
        if case == "proxy_auth" {
            source.tunnels[0]
                .options
                .auth
                .users
                .insert("reader".into(), "password".into());
        }
        let mut local = visitor("tcp").await;
        local.connection = VisitorConnection::Direct;
        local.target = Some(format!("localhost:{}", echo().await));
        let fixture = rig(config, source, local, TransportKind::Tls).await;
        if case == "allowed" {
            roundtrip(TcpStream::connect(&fixture.bind).await.unwrap()).await;
            totals(&fixture.server, 5).await;
        } else {
            rejected(&fixture.bind).await;
            totals(&fixture.server, 0).await;
        }
    }
}

#[tokio::test]
async fn direct_file_service_keeps_auth_and_directory_capabilities() {
    let files = identity("peer-file.test").await;
    let root = files.root.join("public");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("readme"), "direct private file").unwrap();
    std::fs::write(files.root.join("outside"), "outside secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(files.root.join("outside"), root.join("escape")).unwrap();
    let mut source = agent(&format!(
        "[[tunnel]]\nid=1\nmode='file'\npublic=false\n[tunnel.file]\nroot='{}'\nread_only=true\n[tunnel.file.auth.users]\nreader='password'",
        root.display()
    ));
    source.p2p.enabled = true;
    let mut local = visitor("tcp").await;
    local.connection = VisitorConnection::Direct;
    let rig = rig(broker_config().await, source, local, TransportKind::Tls).await;
    let address: SocketAddr = rig.bind.parse().unwrap();
    let no_auth = http_request(
        address.port(),
        "GET /readme HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(no_auth.starts_with("HTTP/1.1 401"));
    released(&rig.server).await;
    for (path, allowed) in [("readme", true), ("escape", false)] {
        let request = format!(
            "GET /{path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic cmVhZGVyOnBhc3N3b3Jk\r\nConnection: close\r\n\r\n"
        );
        let response = http_request(address.port(), &request).await;
        assert_eq!(response.contains("direct private file"), allowed);
        assert!(!response.contains("outside secret"));
        released(&rig.server).await;
    }
    let put = http_request(address.port(), "PUT /new HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic cmVhZGVyOnBhc3N3b3Jk\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad").await;
    assert!(!put.starts_with("HTTP/1.1 201"));
    assert!(!root.join("new").exists());
    rig.server.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn direct_quota_exhaustion_closes_live_data_and_does_not_fall_back() {
    for owner in ["features", "guest"] {
        let mut config = broker_config().await;
        let mut guest = config.clients["features"].clone();
        guest.vkey = "guest-vkey".into();
        config.clients.insert("guest".into(), guest);
        config.clients.get_mut(owner).unwrap().quota.flow_limit = Some(32768);
        let mut source = provider("tcp", echo().await);
        source.p2p.enabled = true;
        let mut local = visitor("tcp").await;
        local.connection = VisitorConnection::PreferDirect;
        let rig = rig(config, source, local, TransportKind::Tls).await;
        let mut stream = TcpStream::connect(&rig.bind).await.unwrap();
        let (mut read, mut write) = stream.split();
        let sender = async {
            let _ = write.write_all(&vec![7; 1024 * 1024]).await;
            let _ = write.shutdown().await;
        };
        let receiver = async {
            let mut data = Vec::new();
            let _ = read.read_to_end(&mut data).await;
            assert!(data.len() < 1024 * 1024);
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(sender, receiver);
        })
        .await
        .unwrap();
        drop(stream);
        released(&rig.server).await;
        let stats = rig.server.registry().get(owner).unwrap().traffic.stats();
        assert!(stats.inlet + stats.export >= 32768);
        assert!(stats.inlet + stats.export <= 32768 + 2 * 16384);
        let provider = rig
            .server
            .registry()
            .get("features")
            .unwrap()
            .traffic
            .stats();
        let visitor = rig.server.registry().get("guest").unwrap().traffic.stats();
        assert_eq!(provider, visitor);
        rejected(&rig.bind).await;
        assert_eq!(
            rig.server.registry().get(owner).unwrap().traffic.stats(),
            stats
        );
    }
}

#[tokio::test]
async fn direct_quic_crosses_two_simulated_port_filtered_nat_mappings() {
    let mut config = broker_config().await;
    let broker: SocketAddr = config.p2p.as_ref().unwrap().bind.parse().unwrap();
    let gateway = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    config.p2p.as_mut().unwrap().advertise = gateway.local_addr().unwrap().to_string();
    let mappings = [
        UdpSocket::bind("127.0.0.1:0").await.unwrap(),
        UdpSocket::bind("127.0.0.1:0").await.unwrap(),
    ];
    let forwarded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = forwarded.clone();
    // Each peer has a distinct external mapping. Its incoming port filter opens
    // only after it sends to the other mapping; the provider's punch is required.
    let router = tokio::spawn(async move {
        let mut internal: [Option<SocketAddr>; 2] = [None, None];
        let mut outbound = [false; 2];
        let mut ingress = vec![0; 65536];
        let mut first = vec![0; 65536];
        let mut second = vec![0; 65536];
        loop {
            let (port, n, source, bytes) = tokio::select! {
                result = gateway.recv_from(&mut ingress) => {
                    let (n, source) = result.unwrap();
                    (2, n, source, &ingress[..n])
                }
                result = mappings[0].recv_from(&mut first) => {
                    let (n, source) = result.unwrap();
                    (0, n, source, &first[..n])
                }
                result = mappings[1].recv_from(&mut second) => {
                    let (n, source) = result.unwrap();
                    (1, n, source, &second[..n])
                }
            };
            if port == 2 {
                if n != 256 || &bytes[..4] != b"\0FP1" || bytes[5] > 1 {
                    continue;
                }
                let role = usize::from(bytes[5]);
                internal[role] = Some(source);
                mappings[role].send_to(bytes, broker).await.unwrap();
            } else if source == broker {
                if let Some(target) = internal[port] {
                    gateway.send_to(bytes, target).await.unwrap();
                }
            } else {
                let sender = 1 - port;
                if internal[sender] != Some(source) {
                    continue;
                }
                outbound[sender] = true;
                if outbound[port]
                    && let Some(target) = internal[port]
                {
                    // Source is the sender's external mapping at the recipient.
                    mappings[sender].send_to(bytes, target).await.unwrap();
                    counter.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
    });
    let mut provider = provider("tcp", echo().await);
    provider.p2p.enabled = true;
    let mut visitor = visitor("tcp").await;
    visitor.connection = VisitorConnection::Direct;
    let rig = rig(config, provider, visitor, TransportKind::Tls).await;
    let mut stream = TcpStream::connect(&rig.bind).await.unwrap();
    let (mut read, mut write) = stream.split();
    let bytes = vec![3; 2 * 1024 * 1024];
    tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            async {
                write.write_all(&bytes).await.unwrap();
                write.shutdown().await.unwrap();
            },
            async {
                let mut data = Vec::new();
                read.read_to_end(&mut data).await.unwrap();
                assert_eq!(data, bytes);
            }
        );
    })
    .await
    .unwrap();
    drop(stream);
    totals(&rig.server, bytes.len() as u64).await;
    assert!(forwarded.load(std::sync::atomic::Ordering::Relaxed) > 2 * bytes.len());
    rig.server.shutdown_and_wait().await.unwrap();
    router.abort();
}

#[tokio::test]
async fn direct_reset_and_provider_disconnect_release_backends_and_broker_listener() {
    let config = broker_config().await;
    let broker = config.p2p.as_ref().unwrap().bind.clone();
    let mut provider = provider("tcp", echo().await);
    provider.p2p.enabled = true;
    provider.max_connections = Some(1);
    let mut visitor = visitor("tcp").await;
    visitor.connection = VisitorConnection::Direct;
    let rig = rig(config, provider, visitor, TransportKind::Tls).await;
    let mut first = TcpStream::connect(&rig.bind).await.unwrap();
    first.write_all(b"first").await.unwrap();
    let mut bytes = [0; 5];
    tokio::time::timeout(Duration::from_secs(5), first.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"first");
    rejected(&rig.bind).await;
    socket2::SockRef::from(&first)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(first);
    released(&rig.server).await;
    let mut second = TcpStream::connect(&rig.bind).await.unwrap();
    second.write_all(b"second").await.unwrap();
    let mut bytes = [0; 6];
    tokio::time::timeout(Duration::from_secs(5), second.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"second");
    rig._provider.agent.shutdown();
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), second.read_to_end(&mut response))
        .await
        .unwrap();
    released(&rig.server).await;
    rig.server.shutdown_and_wait().await.unwrap();
    // The old Server Arc remains alive, so only correct runtime cleanup frees it.
    let _rebound = UdpSocket::bind(&broker).await.unwrap();
}

#[tokio::test]
async fn direct_transfers_obey_either_clients_rate_limit() {
    for owner in ["features", "guest"] {
        let mut config = broker_config().await;
        let mut guest = config.clients["features"].clone();
        guest.vkey = "guest-vkey".into();
        config.clients.insert("guest".into(), guest);
        config.clients.get_mut(owner).unwrap().rate_limit = Some(65536);
        let mut source = provider("tcp", echo().await);
        source.p2p.enabled = true;
        let mut local = visitor("tcp").await;
        local.connection = VisitorConnection::Direct;
        let rig = rig(config, source, local, TransportKind::Tls).await;
        let mut stream = TcpStream::connect(&rig.bind).await.unwrap();
        let (mut read, mut write) = stream.split();
        let payload = vec![5; 65536];
        let started = tokio::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                async {
                    write.write_all(&payload).await.unwrap();
                    write.shutdown().await.unwrap();
                },
                async {
                    let mut data = Vec::new();
                    read.read_to_end(&mut data).await.unwrap();
                    assert_eq!(data, payload);
                }
            );
        })
        .await
        .unwrap();
        // One second's initial burst, then another second for the echoed bytes.
        assert!(
            started.elapsed() >= Duration::from_millis(850),
            "{owner}: {:?}",
            started.elapsed()
        );
        drop(stream);
        totals(&rig.server, payload.len() as u64).await;
    }
}
