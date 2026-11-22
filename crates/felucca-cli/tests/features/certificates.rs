use super::*;
#[tokio::test]
async fn bridge_certificate_reload_updates_tls_and_quic_without_dropping_existing_tunnels() {
    let original = identity("bridge.test").await;
    let replacement = identity("bridge.test").await;
    let port = free_port().await;
    let public = free_port().await;
    let backend = echo().await;
    let reservation = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let quic_port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let mut server = server_config();
    server.bridge.bind = format!("127.0.0.1:{port}");
    server.bridge.quic_bind = Some(format!("127.0.0.1:{quic_port}"));
    server.bridge.tls = Some(original.config.clone());
    server.bridge.cert_reload_interval = Duration::from_millis(50);
    let running = start(
        server,
        agent(&format!(
            r#"
[[tunnel]]
mode = "tcp"
port = {public}
backend = [{{addr = "127.0.0.1:{backend}"}}]
"#
        )),
        TransportKind::Tls,
    )
    .await;
    let old = running.server.registry().get("features").unwrap().id;
    tokio::fs::copy(&replacement.config.key, &original.config.key)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(160)).await;
    assert_eq!(
        running.server.fingerprint(),
        original.identity.fingerprint()
    );
    tokio::fs::copy(&replacement.config.cert, &original.config.cert)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while running.server.fingerprint() != replacement.identity.fingerprint() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let policy = felucca_transport::Verification::Pinned {
        fingerprint: replacement.identity.fingerprint(),
    };
    let tls = felucca_transport::tls::connect(&format!("127.0.0.1:{port}"), &policy)
        .await
        .unwrap();
    drop(tls);
    let quic = felucca_transport::quic::connect(&format!("127.0.0.1:{quic_port}"), &policy)
        .await
        .unwrap();
    quic.close(0_u32.into(), b"checked");
    roundtrip(TcpStream::connect(("127.0.0.1", public)).await.unwrap()).await;
    assert_eq!(running.server.registry().get("features").unwrap().id, old);
}
#[tokio::test]
async fn tcp_proxy_headers_preserve_trusted_addresses_through_a_tls_backend() {
    use felucca_core::config::ProxyVersion;
    use felucca_transport::proxy_protocol::{Addresses, decode};
    let certificate = identity("backend.test").await;
    for version in [ProxyVersion::V1, ProxyVersion::V2] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend = listener.local_addr().unwrap().port();
        let acceptor = certificate.identity.acceptor().unwrap();
        let expected = Addresses {
            source: "192.0.2.7:4321".parse().unwrap(),
            destination: "198.51.100.4:443".parse().unwrap(),
        };
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut io = acceptor.accept(tcp).await.unwrap();
            let mut data = Vec::new();
            loop {
                data.push(io.read_u8().await.unwrap());
                if let Ok((Some(addresses), length)) = decode(&data) {
                    assert_eq!(addresses, expected);
                    assert_eq!(length, data.len());
                    break;
                }
                assert!(data.len() <= 108);
            }
            let mut payload = [0; 5];
            io.read_exact(&mut payload).await.unwrap();
            assert_eq!(&payload, b"hello");
            io.write_all(&payload).await.unwrap();
            io.shutdown().await.unwrap();
        });
        let port = free_port().await;
        let mode = if version == ProxyVersion::V1 {
            "v1"
        } else {
            "v2"
        };
        let config = agent(&format!(
            r#"
[[tunnel]]
mode = "tcp"
port = {port}
acl_mode = "allow"
acl_rules = "192.0.2.0/24"
backend = [{{addr = "127.0.0.1:{backend}", tls = true, trust = {{mode = "pinned", fingerprint = "{pin}"}}}}]
[tunnel.options]
proxy_protocol = "{mode}"
[tunnel.options.proxy_accept]
mode = "required"
trusted = ["127.0.0.1"]
"#,
            pin = certificate.identity.fingerprint()
        ));
        let _running = start(server_config(), config, TransportKind::Tls).await;
        let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        io.write_all(b"PROXY TCP4 192.0.2.7 198.51.100.4 4321 443\r\nhello")
            .await
            .unwrap();
        io.shutdown().await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), io.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response, b"hello");
        task.await.unwrap();
    }
}
#[tokio::test]
async fn sni_passthrough_and_raw_tls_offload_share_an_https_listener() {
    let backend_identity = identity("pass.test").await;
    let frontend_identity = identity("offload.test").await;
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let acceptor = backend_identity.identity.acceptor().unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        loop {
            let (io, _) = backend.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let io = acceptor.accept(io).await.unwrap();
                assert_eq!(io.get_ref().1.server_name(), Some("pass.test"));
                let (mut r, mut w) = tokio::io::split(io);
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
    });
    let plain = echo().await;
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: None,
        tls_bind: Some(format!("127.0.0.1:{port}")),
        default_tls: Some(frontend_identity.config.clone()),
        ..Default::default()
    });
    let _running = start(
        server,
        agent(&format!(
            r#"
[[tunnel]]
mode='http'
port={port}
hosts=['pass.test']
[[tunnel.backend]]
addr='127.0.0.1:{backend_port}'
[tunnel.options.http]
tls_mode='passthrough'
[[tunnel]]
mode='http'
port={port}
hosts=['offload.test']
[[tunnel.backend]]
addr='127.0.0.1:{plain}'
[tunnel.options.http]
tls_mode='offload'
"#
        )),
        TransportKind::Tls,
    )
    .await;
    for (host, pin) in [
        ("pass.test", backend_identity.identity.fingerprint()),
        ("offload.test", frontend_identity.identity.fingerprint()),
    ] {
        let tls = felucca_transport::tls::connect_to(
            ([127, 0, 0, 1], port).into(),
            host,
            &felucca_transport::Verification::Pinned { fingerprint: pin },
        )
        .await
        .unwrap();
        roundtrip(tls).await;
    }
}
#[tokio::test]
async fn http2_frontend_and_tls_backend_negotiate_h2_and_preserve_the_request() {
    use http_body_util::BodyExt;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    let backend_identity = identity("backend.test").await;
    let frontend_identity = identity("localhost").await;
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let connection_count = accepted.clone();
    let mut tls_config = (*backend_identity.identity.server_config().unwrap()).clone();
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        loop {
            let (io, _) = backend.accept().await.unwrap();
            connection_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let io = acceptor.accept(io).await.unwrap();
                assert_eq!(io.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
                let service = hyper::service::service_fn(
                    |request: http::Request<hyper::body::Incoming>| async move {
                        let summary = format!(
                            "{:?} {} {}\n",
                            request.version(),
                            request.method(),
                            request.uri()
                        );
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        let mut data = summary.into_bytes();
                        data.extend(body);
                        Ok::<_, std::convert::Infallible>(http::Response::new(
                            http_body_util::Full::new(bytes::Bytes::from(data)),
                        ))
                    },
                );
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(io), service)
                    .await;
            });
        }
    });
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: None,
        tls_bind: Some(format!("127.0.0.1:{port}")),
        default_tls: Some(frontend_identity.config.clone()),
        ..Default::default()
    });
    let _running = start(
        server,
        agent(&format!(
            r#"
[[tunnel]]
mode='http'
port={port}
hosts=['localhost']
[[tunnel.backend]]
addr='127.0.0.1:{backend_port}'
tls=true
trust={{mode='pinned',fingerprint='{}'}}
[tunnel.options.http]
backend_http2=true
host='backend.test'
"#,
            backend_identity.identity.fingerprint()
        )),
        TransportKind::Tls,
    )
    .await;
    let tls = felucca_transport::tls::connect_to_with_alpn(
        ([127, 0, 0, 1], port).into(),
        "localhost",
        &felucca_transport::Verification::Pinned {
            fingerprint: frontend_identity.identity.fingerprint(),
        },
        &[b"h2"],
    )
    .await
    .unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    tasks.spawn(async move {
        let _ = connection.await;
    });
    for n in 0..2 {
        let payload = vec![b'x'; 128 * 1024 + n];
        let request = http::Request::builder()
            .method("POST")
            .uri("https://localhost/hello?q=1")
            .body(http_body_util::Full::new(bytes::Bytes::from(
                payload.clone(),
            )))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        assert_eq!(response.status(), 200);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let prefix = b"HTTP/2.0 POST https://backend.test/hello?q=1\n";
        assert!(
            body.starts_with(prefix),
            "{}",
            String::from_utf8_lossy(&body[..body.len().min(200)])
        );
        assert_eq!(&body[prefix.len()..], payload);
    }
    let request = http::Request::builder()
        .uri("https://wrong.test/")
        .body(http_body_util::Full::new(bytes::Bytes::new()))
        .unwrap();
    assert_eq!(sender.send_request(request).await.unwrap().status(), 421);
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "HTTP/2 requests must share their backend connection"
    );
}
#[tokio::test]
async fn static_certificates_reload_and_a_mismatched_intermediate_update_keeps_the_old_key() {
    let original = identity("reload.test").await;
    let replacement = identity("reload.test").await;
    let port = free_port().await;
    let backend = web_backend("reload").await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: None,
        tls_bind: Some(format!("127.0.0.1:{port}")),
        certificates: [("reload.test".into(), original.config.clone())].into(),
        cert_reload_interval: Duration::from_millis(50),
        ..Default::default()
    });
    let _running = start(server, agent(&format!(
        "[[tunnel]]\nmode='http'\nport={port}\nhosts=['reload.test']\n[[tunnel.backend]]\naddr='127.0.0.1:{backend}'"
    )), TransportKind::Tls).await;
    let old_pin = felucca_transport::Verification::Pinned {
        fingerprint: original.identity.fingerprint(),
    };
    let new_pin = felucca_transport::Verification::Pinned {
        fingerprint: replacement.identity.fingerprint(),
    };
    let address = ([127, 0, 0, 1], port).into();
    let io = felucca_transport::tls::connect_to(address, "reload.test", &old_pin)
        .await
        .unwrap();
    drop(io);
    tokio::fs::copy(&replacement.config.key, &original.config.key)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(160)).await;
    let io = felucca_transport::tls::connect_to(address, "reload.test", &old_pin)
        .await
        .unwrap();
    drop(io);
    tokio::fs::copy(&replacement.config.cert, &original.config.cert)
        .await
        .unwrap();
    let mut io = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(io) =
                felucca_transport::tls::connect_to(address, "reload.test", &new_pin).await
            {
                break io;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    io.write_all(b"GET / HTTP/1.1\r\nHost: reload.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    assert!(head(&mut io).await.starts_with("HTTP/1.1 200"));
}
#[tokio::test]
async fn tls_backend_and_sni_certificate_work_end_to_end() {
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert = generated.cert.pem();
    let key = generated.signing_key.serialize_pem();
    let identity = felucca_transport::Identity::from_pem(cert.as_bytes(), key.as_bytes()).unwrap();
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let acceptor = identity.acceptor().unwrap();
    tokio::spawn(async move {
        while let Ok((io, _)) = backend.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let mut tls = acceptor.accept(io).await.unwrap();
                let _ = head(&mut tls).await;
                tls.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecure",
                )
                .await
                .unwrap();
                tls.shutdown().await.unwrap();
            });
        }
    });
    let root = std::env::temp_dir().join(format!("felucca-tls-{}", free_port().await));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let cert_file = root.join("cert.pem");
    let key_file = root.join("key.pem");
    tokio::fs::write(&cert_file, cert).await.unwrap();
    tokio::fs::write(&key_file, key).await.unwrap();
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: None,
        tls_bind: Some(format!("127.0.0.1:{port}")),
        certificates: [(
            "localhost".into(),
            TlsConfig {
                cert: cert_file,
                key: key_file,
            },
        )]
        .into(),
        ..Default::default()
    });
    let config = agent(&format!(
        "[[tunnel]]\nmode='http'\nport={port}\nhosts=['localhost']\n[[tunnel.backend]]\naddr='127.0.0.1:{backend_port}'\ntls=true\n[tunnel.backend.trust]\nmode='pinned'\nfingerprint='{}'",
        identity.fingerprint()
    ));
    let _running = start(server, config, TransportKind::Tls).await;
    let policy = felucca_transport::Verification::Pinned {
        fingerprint: identity.fingerprint(),
    };
    let mut tls = felucca_transport::tls::connect(&format!("localhost:{port}"), &policy)
        .await
        .unwrap();
    tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(5), tls.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.ends_with("secure"), "{response}");
    let mut tls = felucca_transport::tls::connect(&format!("localhost:{port}"), &policy)
        .await
        .unwrap();
    tls.write_all(b"GET / HTTP/1.1\r\nHost: other.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(5), tls.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 421"), "{response}");
    tokio::fs::remove_dir_all(root).await.unwrap();
}
