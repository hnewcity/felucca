use super::*;
#[tokio::test]
async fn socks_udp_associate_carries_real_ipv6_datagrams() {
    let backend = tokio::net::UdpSocket::bind("[::1]:0").await.unwrap();
    let address = backend.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut buffer = [0; 4096];
        for _ in 0..2 {
            let (n, peer) = backend.recv_from(&mut buffer).await.unwrap();
            assert!(peer.is_ipv6());
            backend.send_to(&buffer[..n], peer).await.unwrap();
        }
    });
    let port = free_port().await;
    let _running = start(
        server_config(),
        agent(&format!(
            "[[tunnel]]\nmode='socks5'\nport={port}\nbind='::1'\nacl_mode='allow'\nacl_rules='::1'"
        )),
        TransportKind::Tls,
    )
    .await;
    let mut control = TcpStream::connect(("::1", port)).await.unwrap();
    control.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0; 2];
    control.read_exact(&mut method).await.unwrap();
    assert_eq!(method, [5, 0]);
    let mut command = vec![5, 3, 0, 4];
    command.extend_from_slice(&[0; 18]);
    control.write_all(&command).await.unwrap();
    let mut response = [0; 22];
    control.read_exact(&mut response).await.unwrap();
    assert_eq!(&response[..4], &[5, 0, 0, 4]);
    let ip = std::net::Ipv6Addr::from(<[u8; 16]>::try_from(&response[4..20]).unwrap());
    let relay =
        std::net::SocketAddr::new(ip.into(), u16::from_be_bytes([response[20], response[21]]));
    let udp = tokio::net::UdpSocket::bind("[::1]:0").await.unwrap();
    let mut packet = vec![0, 0, 0, 4];
    packet.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
    packet.extend_from_slice(&address.port().to_be_bytes());
    for payload in [b"".as_slice(), b"IPv6 payload"] {
        packet.truncate(22);
        packet.extend_from_slice(payload);
        udp.send_to(&packet, relay).await.unwrap();
        let mut reply = [0; 4096];
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), udp.recv_from(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&reply[..n], packet);
    }
    task.await.unwrap();
}
#[tokio::test]
async fn mixed_proxy_authenticates_each_account_and_never_downgrades() {
    let port = free_port().await;
    let echo = echo().await;
    let web = web_backend("authenticated").await;
    let _running = start(server_config(), agent(&format!(
        "[[tunnel]]\nmode='mixedproxy'\nport={port}\nbind='127.0.0.1'\n[tunnel.options.auth.users]\nalice='first-password'\nbob='second-password'"
    )), TransportKind::Tls).await;
    for (name, password) in [("alice", "first-password"), ("bob", "second-password")] {
        let mut io = socks_login(port, name, password).await;
        let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
        request.extend(echo.to_be_bytes());
        io.write_all(&request).await.unwrap();
        let mut reply = [0; 10];
        io.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], 0);
        roundtrip(io).await;
    }
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(&[5, 1, 0]).await.unwrap();
    let mut response = [0; 2];
    io.read_exact(&mut response).await.unwrap();
    assert_eq!(response, [5, 255]);
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(&[5, 1, 2]).await.unwrap();
    io.read_exact(&mut response).await.unwrap();
    io.write_all(b"\x01\x05alice\x05wrong").await.unwrap();
    io.read_exact(&mut response).await.unwrap();
    assert_eq!(response, [1, 1]);
    for auth in ["", "Proxy-Authorization: Basic Ym9iOndyb25n\r\n"] {
        let response = http_request(port, &format!("GET http://127.0.0.1:{web}/ HTTP/1.1\r\nHost: example\r\n{auth}Connection: close\r\n\r\n")).await;
        assert!(response.starts_with("HTTP/1.1 407"), "{response}");
        assert!(
            response
                .to_ascii_lowercase()
                .contains("proxy-authenticate: basic")
        );
    }
    let auth = "Proxy-Authorization: Basic YWxpY2U6Zmlyc3QtcGFzc3dvcmQ=\r\n";
    let response = http_request(port, &format!("GET http://127.0.0.1:{web}/ HTTP/1.1\r\nHost: example\r\n{auth}Connection: close\r\n\r\n")).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("authenticated"));
    assert!(
        !response
            .to_ascii_lowercase()
            .contains("proxy-authorization")
    );
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(
        format!("CONNECT 127.0.0.1:{echo} HTTP/1.1\r\nHost: 127.0.0.1:{echo}\r\n{auth}\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    assert!(head(&mut io).await.starts_with("HTTP/1.1 200"));
    roundtrip(io).await;
}
#[tokio::test]
async fn socks_udp_is_bound_to_the_tcp_session_and_first_udp_source() {
    let backend = Arc::new(felucca_transport::udp::bind("127.0.0.1:0").await.unwrap());
    let target = backend.local_addr().unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        let mut buffer = vec![0; 65536];
        loop {
            let (n, peer) = backend.recv_from(&mut buffer).await.unwrap();
            backend.send_to(&buffer[..n], peer).await.unwrap();
        }
    });
    let port = free_port().await;
    let running = start(server_config(), agent(&format!(
        "[[tunnel]]\nmode='socks5'\nport={port}\nbind='127.0.0.1'\n[tunnel.options.auth.users]\nalice='password'"
    )), TransportKind::Tls).await;
    let mut control = socks_login(port, "alice", "password").await;
    control
        .write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0])
        .await
        .unwrap();
    let mut reply = [0; 10];
    control.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply[..4], &[5, 0, 0, 1]);
    let relay = std::net::SocketAddr::from((
        [reply[4], reply[5], reply[6], reply[7]],
        u16::from_be_bytes([reply[8], reply[9]]),
    ));
    let public = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let mut packet = vec![0, 0, 0, 1, 127, 0, 0, 1];
    packet.extend(target.port().to_be_bytes());
    let mut buffer = vec![0; 65536];
    for size in [0, 16, 25000] {
        packet.resize(10, 0);
        packet.extend(vec![42; size]);
        public.send_to(&packet, relay).await.unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), public.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer[..n], packet);
    }
    let foreign = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    foreign.send_to(&packet, relay).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), foreign.recv_from(&mut buffer))
            .await
            .is_err()
    );
    let mut fragment = packet.clone();
    fragment[2] = 1;
    public.send_to(&fragment, relay).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), public.recv_from(&mut buffer))
            .await
            .is_err()
    );
    drop(control);
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
    public.send_to(&packet, relay).await.unwrap();
    assert!(!matches!(
        tokio::time::timeout(Duration::from_millis(150), public.recv_from(&mut buffer)).await,
        Ok(Ok(_))
    ));
}
#[tokio::test]
async fn socks5_connects_by_ip_and_dns_and_preserves_half_close() {
    let backend = echo().await;
    let port = free_port().await;
    let _running = start(
        server_config(),
        agent(&format!(
            "[[tunnel]]\nmode='socks5'\nport={port}\nbind='127.0.0.1'"
        )),
        TransportKind::Tls,
    )
    .await;
    for dns in [false, true] {
        let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        io.write_all(&[5, 1, 0]).await.unwrap();
        let mut response = [0; 2];
        io.read_exact(&mut response).await.unwrap();
        assert_eq!(response, [5, 0]);
        let mut connect = if dns {
            let mut c = vec![5, 1, 0, 3, 9];
            c.extend_from_slice(b"localhost");
            c
        } else {
            vec![5, 1, 0, 1, 127, 0, 0, 1]
        };
        connect.extend_from_slice(&backend.to_be_bytes());
        io.write_all(&connect).await.unwrap();
        let mut response = [0; 10];
        io.read_exact(&mut response).await.unwrap();
        assert_eq!(response[1], 0);
        roundtrip(io).await;
    }
}
