use super::*;
#[tokio::test]
async fn http3_streams_to_http2_backends_and_isolates_bad_requests() {
    use bytes::Buf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let certificate = identity("h3.test").await;
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let backend_hits = hits.clone();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        while let Ok((io, _)) = backend.accept().await {
            let hits = backend_hits.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: http::Request<hyper::body::Incoming>| {
                        hits.fetch_add(1, Ordering::Relaxed);
                        assert_eq!(request.version(), http::Version::HTTP_2);
                        assert_eq!(request.uri().path_and_query().unwrap(), "/v2/bulk?x=1");
                        assert_eq!(request.uri().authority().unwrap(), "backend.test");
                        assert_eq!(request.headers()["x-forwarded-proto"], "https");
                        async move {
                            Ok::<_, std::convert::Infallible>(http::Response::new(
                                request.into_body(),
                            ))
                        }
                    },
                );
                let _ =
                    hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
                        .serve_connection(hyper_util::rt::TokioIo::new(io), service)
                        .await;
            });
        }
    });
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: None,
        tls_bind: None,
        http3_bind: Some(format!("127.0.0.1:{port}")),
        certificates: [("h3.test".into(), certificate.config.clone())].into(),
        ..Default::default()
    });
    let config = agent(&format!(
        r#"
[[tunnel]]
mode = "http"
port = {port}
hosts = ["h3.test"]
backend = [{{addr = "127.0.0.1:{backend_port}"}}]
[tunnel.options.http]
path = "/api/"
rewrite_prefix = "/v2/"
host = "backend.test"
backend_http2 = true
"#
    ));
    let _running = start(server, config, TransportKind::Tls).await;
    let mut tls = felucca_transport::Verification::Pinned {
        fingerprint: certificate.identity.fingerprint(),
    }
    .rustls_client_config_tls13()
    .unwrap();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(crypto)));
    let connection = endpoint
        .connect(([127, 0, 0, 1], port).into(), "h3.test")
        .unwrap()
        .await
        .unwrap();
    let (mut driver, mut sender) = h3::client::new(h3_quinn::Connection::new(connection))
        .await
        .unwrap();
    tasks.spawn(async move {
        driver.wait_idle().await;
    });
    let mut requests = tokio::task::JoinSet::new();
    for index in 0..8 {
        let mut sender = sender.clone();
        requests.spawn(async move {
            let size = if index == 0 { 2 * 1024 * 1024 } else { 16384 };
            let request = http::Request::builder()
                .method(http::Method::POST)
                .uri("https://h3.test/api/bulk?x=1")
                .header("content-length", size)
                .body(())
                .unwrap();
            let stream = sender.send_request(request).await.unwrap();
            let (mut write, mut read) = stream.split();
            let sending = async {
                for _ in 0..size / 16384 {
                    write
                        .send_data(bytes::Bytes::from(vec![index as u8; 16384]))
                        .await
                        .unwrap();
                }
                write.finish().await.unwrap();
            };
            let receiving = async {
                let response = read.recv_response().await.unwrap();
                assert_eq!(response.status(), http::StatusCode::OK);
                assert!(
                    response.headers()["alt-svc"]
                        .to_str()
                        .unwrap()
                        .starts_with("h3=")
                );
                let mut received = 0;
                while let Some(mut data) = read.recv_data().await.unwrap() {
                    received += data.remaining();
                    while data.has_remaining() {
                        assert_eq!(data.get_u8(), index as u8);
                    }
                }
                assert_eq!(received, size);
            };
            tokio::time::timeout(Duration::from_secs(15), async {
                tokio::join!(sending, receiving);
            })
            .await
            .unwrap();
        });
    }
    while let Some(result) = requests.join_next().await {
        result.unwrap();
    }
    assert_eq!(hits.load(Ordering::Relaxed), 8);
    // A mismatched authority cannot route traffic to another TLS tenant.
    let mut mismatch = sender
        .send_request(
            http::Request::builder()
                .uri("https://other.test/api/bulk?x=1")
                .body(())
                .unwrap(),
        )
        .await
        .unwrap();
    mismatch.finish().await.unwrap();
    assert_eq!(
        mismatch.recv_response().await.unwrap().status(),
        http::StatusCode::MISDIRECTED_REQUEST
    );
    while mismatch.recv_data().await.unwrap().is_some() {}
    // Duplicate lengths must fail the stream before a backend sees it.
    let mut invalid = sender
        .send_request(
            http::Request::builder()
                .uri("https://h3.test/api/bulk?x=1")
                .header("content-length", "1")
                .header("content-length", "2")
                .body(())
                .unwrap(),
        )
        .await
        .unwrap();
    let _ = invalid.finish().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), invalid.recv_response())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(hits.load(Ordering::Relaxed), 8);
    endpoint.close(0_u32.into(), b"test complete");
}
#[tokio::test]
async fn shared_https_accepts_bridge_websocket_with_standard_http_alpn() {
    use felucca_proto::{Control, StreamRequest, read_frame, write_frame};
    let certificate = identity("fronted.test").await;
    let port = free_port().await;
    let mut config = server_config();
    config.bridge.bind = format!("127.0.0.1:{port}");
    config.http = Some(HttpProxyConfig {
        bind: None,
        tls_bind: Some(config.bridge.bind.clone()),
        certificates: [("fronted.test".into(), certificate.config.clone())].into(),
        ..Default::default()
    });
    let server = Arc::new(Server::new(config).unwrap());
    server.clone().run().await.unwrap();
    let tls = felucca_transport::tls::connect_to_with_alpn(
        ([127, 0, 0, 1], port).into(),
        "fronted.test",
        &felucca_transport::Verification::Pinned {
            fingerprint: certificate.identity.fingerprint(),
        },
        &[b"http/1.1"],
    )
    .await
    .unwrap();
    let mut ws = felucca_transport::WsConn::connect("wss://fronted.test/ws?via=proxy", tls)
        .await
        .unwrap();
    felucca_proto::session::connect(
        &mut ws,
        "features",
        &felucca_core::Secret::new("feature-secret"),
        felucca_proto::ConnectionRole::Tunnel,
    )
    .await
    .unwrap();
    let tunnel = felucca_transport::Tunnel::over_stream(
        ws,
        felucca_mux::Role::Client,
        felucca_mux::Config::default(),
    );
    let mut stream = tunnel.open().await.unwrap();
    write_frame(&mut stream, &StreamRequest::Control)
        .await
        .unwrap();
    write_frame(&mut stream, &Control::Ping { nonce: 42 })
        .await
        .unwrap();
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame::<_, Control>(&mut stream),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reply, Control::Pong { nonce: 42 });
    server.shutdown();
}
#[tokio::test]
async fn bridge_and_http_https_share_one_port_with_scheme_routing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("felucca=debug,felucca_server=debug,felucca_transport=debug")
        .with_test_writer()
        .try_init();
    let certificate = identity("shared.test").await;
    let backend_http = web_backend("plain-route").await;
    let backend_https = web_backend("secure-route").await;
    let backend_echo = echo().await;
    for transport in [
        TransportKind::Tcp,
        TransportKind::Tls,
        TransportKind::Ws,
        TransportKind::Wss,
    ] {
        let port = free_port().await;
        let tcp_port = free_port().await;
        let mut server = server_config();
        server.bridge.bind = format!("127.0.0.1:{port}");
        server.http = Some(HttpProxyConfig {
            bind: Some(server.bridge.bind.clone()),
            tls_bind: Some(server.bridge.bind.clone()),
            certificates: [("shared.test".into(), certificate.config.clone())].into(),
            ..Default::default()
        });
        let config = agent(&format!(
            r#"
[[tunnel]]
mode = "http"
hosts = ["shared.test"]
port = {port}
backend = [{{addr = "127.0.0.1:{backend_http}"}}]
[tunnel.options.http]
scheme = "http"
[[tunnel]]
mode = "http"
hosts = ["shared.test"]
port = {port}
backend = [{{addr = "127.0.0.1:{backend_https}"}}]
[tunnel.options.http]
scheme = "https"
[[tunnel]]
mode = "tcp"
port = {tcp_port}
backend = [{{addr = "127.0.0.1:{backend_echo}"}}]
"#
        ));
        let _running = start(server, config, transport).await;
        roundtrip(TcpStream::connect(("127.0.0.1", tcp_port)).await.unwrap()).await;
        // A normal GET at the bridge's reserved path remains an HTTP request.
        let response = http_request(
            port,
            "GET /ws?q=1 HTTP/1.1\r\nHost: shared.test\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "{transport:?}: {response}"
        );
        assert!(response.contains("plain-route"), "{response}");
        let mut tls = felucca_transport::tls::connect_to(
            ([127, 0, 0, 1], port).into(),
            "shared.test",
            &felucca_transport::Verification::Pinned {
                fingerprint: certificate.identity.fingerprint(),
            },
        )
        .await
        .unwrap();
        tls.write_all(b"GET /ws HTTP/1.1\r\nHost: shared.test\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        tls.read_to_string(&mut response).await.unwrap();
        assert!(
            response.contains("secure-route"),
            "{transport:?}: {response}"
        );
    }
}
#[tokio::test]
async fn http_and_https_can_share_a_port_separate_from_bridge() {
    let certificate = identity("combined.test").await;
    let backend = web_backend("combined").await;
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: Some(format!("127.0.0.1:{port}")),
        certificates: [("combined.test".into(), certificate.config.clone())].into(),
        ..Default::default()
    });
    let config = agent(&format!(
        r#"
[[tunnel]]
mode = "http"
hosts = ["combined.test"]
port = {port}
backend = [{{addr = "127.0.0.1:{backend}"}}]
"#
    ));
    let _running = start(server, config, TransportKind::Tcp).await;
    let response = http_request(
        port,
        "GET / HTTP/1.1\r\nHost: combined.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.contains("combined"));
    let mut tls = felucca_transport::tls::connect_to(
        ([127, 0, 0, 1], port).into(),
        "combined.test",
        &felucca_transport::Verification::Pinned {
            fingerprint: certificate.identity.fingerprint(),
        },
    )
    .await
    .unwrap();
    tls.write_all(b"GET / HTTP/1.1\r\nHost: combined.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    tls.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
}
#[tokio::test]
async fn separate_public_connections_reuse_a_backend_with_a_connection_cap_of_one() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = accepted.clone();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        while let Ok((io, _)) = backend.accept().await {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    |request: http::Request<hyper::body::Incoming>| async move {
                        Ok::<_, std::convert::Infallible>(http::Response::new(
                            http_body_util::Full::new(bytes::Bytes::from(format!(
                                "pooled {}",
                                request.uri()
                            ))),
                        ))
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(io), service)
                    .await;
            });
        }
    });
    let port = free_port().await;
    let mut server = server_config();
    server.clients.get_mut("features").unwrap().max_connections = Some(1);
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: None,
        ..Default::default()
    });
    let running = start(server, agent(&format!(
        "[[tunnel]]\nmode='http'\nport={port}\nhosts=['pool.test']\n[[tunnel.backend]]\naddr='127.0.0.1:{backend_port}'"
    )), TransportKind::Tls).await;
    for path in ["/first", "/second", "/third"] {
        let response = http_request(
            port,
            &format!("GET {path} HTTP/1.1\r\nHost: pool.test\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains(&format!("pooled {path}")), "{response}");
    }
    assert_eq!(accepted.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(
        running
            .server
            .registry()
            .get("features")
            .unwrap()
            .live_connections(),
        1
    );
}
#[tokio::test]
async fn http_paths_auth_rewrites_headers_cors_and_redirects_are_applied() {
    let root = web_backend("root-route").await;
    let api = web_backend("api-route").await;
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: None,
        ..Default::default()
    });
    let config = agent(&format!(
        r#"
[[tunnel]]
mode='http'
port={port}
hosts=['paths.test']
[[tunnel.backend]]
addr='127.0.0.1:{root}'
[[tunnel]]
mode='http'
port={port}
hosts=['paths.test']
[[tunnel.backend]]
addr='127.0.0.1:{api}'
[tunnel.options.auth.users]
alice='password'
[tunnel.options.http]
path='/api/'
rewrite_prefix='/v2/'
host='internal.test'
cors=true
request_headers={{x-remove='',x-source='{{remote_ip}}',x-original='{{host}}{{path}}'}}
response_headers={{x-route='api'}}
[[tunnel]]
mode='http'
port={port}
hosts=['paths.test']
[[tunnel.backend]]
addr='127.0.0.1:{root}'
[tunnel.options.http]
path='/old/'
redirect='https://new.test{{path}}'
[[tunnel]]
mode='http'
port={port}
hosts=['paths.test']
[[tunnel.backend]]
addr='127.0.0.1:1'
[tunnel.options.http]
path='/failure/'
error_pages={{'502'='<h1>maintenance</h1>'}}
"#
    ));
    let _running = start(server, config, TransportKind::Tls).await;
    let root_response = http_request(
        port,
        "GET / HTTP/1.1\r\nHost: paths.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(root_response.contains("root-route"));
    let rejected = http_request(
        port,
        "GET /api/item HTTP/1.1\r\nHost: paths.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(rejected.starts_with("HTTP/1.1 401"));
    assert!(
        rejected
            .to_ascii_lowercase()
            .contains("www-authenticate: basic")
    );
    let response = http_request(port, "GET /api/item?q=one HTTP/1.1\r\nHost: paths.test\r\nAuthorization: Basic YWxpY2U6cGFzc3dvcmQ=\r\nX-Remove: leaked\r\nConnection: close\r\n\r\n").await;
    assert!(response.contains("api-route"), "{response}");
    assert!(
        response.contains("GET /v2/item?q=one HTTP/1.1"),
        "{response}"
    );
    let lower = response.to_ascii_lowercase();
    for value in [
        "host: internal.test",
        "x-source: 127.0.0.1",
        "x-original: paths.test/api/item?q=one",
        "x-route: api",
        "access-control-allow-origin: *",
    ] {
        assert!(lower.contains(value), "{response}");
    }
    assert!(!lower.contains("authorization:"));
    assert!(!lower.contains("leaked"));
    let preflight = http_request(port, "OPTIONS /api/item HTTP/1.1\r\nHost: paths.test\r\nOrigin: https://other.test\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: x-example\r\nConnection: close\r\n\r\n").await;
    assert!(preflight.starts_with("HTTP/1.1 204"), "{preflight}");
    assert!(
        preflight
            .to_ascii_lowercase()
            .contains("access-control-allow-headers: x-example")
    );
    let redirect = http_request(port, "POST /old/item?q=1 HTTP/1.1\r\nHost: paths.test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    assert!(redirect.starts_with("HTTP/1.1 307"), "{redirect}");
    assert!(
        redirect
            .to_ascii_lowercase()
            .contains("location: https://new.test/old/item?q=1")
    );
    let failure = http_request(
        port,
        "GET /failure/ HTTP/1.1\r\nHost: paths.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(failure.starts_with("HTTP/1.1 502"), "{failure}");
    assert!(failure.contains("<h1>maintenance</h1>"), "{failure}");
}
#[tokio::test]
async fn http_proxy_supports_absolute_urls_and_connect() {
    let backend = web_backend("forwarded").await;
    let echo = echo().await;
    let port = free_port().await;
    let _running = start(
        server_config(),
        agent(&format!(
            "[[tunnel]]\nmode='mixedproxy'\nport={port}\nbind='127.0.0.1'"
        )),
        TransportKind::Tls,
    )
    .await;
    let response = http_request(port, &format!("GET http://127.0.0.1:{backend}/test?q=1 HTTP/1.1\r\nHost: untrusted\r\nProxy-Authorization: Basic secret\r\nConnection: close\r\n\r\n")).await;
    assert!(
        response.contains("forwarded\nGET /test?q=1 HTTP/1.1"),
        "{response}"
    );
    assert!(!response.contains("secret"));
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(
        format!("CONNECT 127.0.0.1:{echo} HTTP/1.1\r\nHost: 127.0.0.1:{echo}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    assert!(head(&mut io).await.starts_with("HTTP/1.1 200"));
    roundtrip(io).await;
}
#[tokio::test]
async fn reverse_proxy_prefers_exact_hosts_and_replaces_spoofed_forwarding_headers() {
    let exact = web_backend("exact").await;
    let wildcard = web_backend("wildcard").await;
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: None,
        ..Default::default()
    });
    let config = agent(&format!(
        "[[tunnel]]\nmode='http'\nport={port}\nhosts=['*.example.test']\n[[tunnel.backend]]\naddr='127.0.0.1:{wildcard}'\n[[tunnel]]\nmode='http'\nport={port}\nhosts=['api.example.test']\n[[tunnel.backend]]\naddr='127.0.0.1:{exact}'"
    ));
    let _running = start(server, config, TransportKind::Wss).await;
    for (host, expected) in [
        ("api.example.test", "exact"),
        ("web.example.test", "wildcard"),
    ] {
        let response = http_request(port, &format!("GET / HTTP/1.1\r\nHost: {host}\r\nX-Forwarded-For: 1.2.3.4\r\nConnection: close, x-remove, x-forwarded-for, host\r\nX-Remove: secret\r\n\r\n")).await;
        assert!(response.contains(expected), "{response}");
        assert!(!response.contains("1.2.3.4"));
        assert!(!response.contains("secret"));
        assert!(response.contains("127.0.0.1"));
    }
    assert!(
        http_request(
            port,
            "GET / HTTP/1.1\r\nHost: unknown.test\r\nConnection: close\r\n\r\n"
        )
        .await
        .starts_with("HTTP/1.1 404")
    );
}
#[tokio::test]
async fn reverse_proxy_upgrades_preserve_bytes_buffered_with_the_http_headers() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = backend.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut io, _) = backend.accept().await.unwrap();
        let request = head(&mut io).await;
        assert!(request.to_ascii_lowercase().contains("upgrade: websocket"));
        let key = request
            .lines()
            .find_map(|line| line.strip_prefix("sec-websocket-key: "))
            .unwrap();
        let accept = felucca_transport::ws::accept_key(key.as_bytes());
        io.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await.unwrap();
        let (mut r, mut w) = io.split();
        tokio::io::copy(&mut r, &mut w).await.unwrap();
    });
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: None,
        ..Default::default()
    });
    let _running = start(server, agent(&format!(
        "[[tunnel]]\nmode='http'\nport={port}\nhosts=['upgrade.test']\n[[tunnel.backend]]\naddr='{address}'"
    )), TransportKind::Tls).await;
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(b"GET / HTTP/1.1\r\nHost: upgrade.test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\nearly-data").await.unwrap();
    assert!(head(&mut io).await.starts_with("HTTP/1.1 101"));
    io.write_all(b"-late-data").await.unwrap();
    io.shutdown().await.unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), io.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, b"early-data-late-data");
}
#[tokio::test]
async fn reverse_proxy_streams_large_bodies_and_reuses_the_public_connection() {
    const LEN: usize = 2 * 1024 * 1024;
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = backend.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..2 {
            let (mut io, _) = backend.accept().await.unwrap();
            let request = head(&mut io).await;
            if request.starts_with("POST") {
                io.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {LEN}\r\n\r\n").as_bytes(),
                )
                .await
                .unwrap();
                let (mut r, mut w) = io.split();
                assert_eq!(
                    tokio::io::copy(&mut (&mut r).take(LEN as u64), &mut w)
                        .await
                        .unwrap(),
                    LEN as u64
                );
            } else {
                io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nagain")
                    .await
                    .unwrap();
            }
        }
    });
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: None,
        ..Default::default()
    });
    let _running = start(server, agent(&format!(
        "[[tunnel]]\nmode='http'\nport={port}\nhosts=['stream.test']\n[[tunnel.backend]]\naddr='{address}'"
    )), TransportKind::Quic).await;
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(
        format!("POST / HTTP/1.1\r\nHost: stream.test\r\nContent-Length: {LEN}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let payload: Vec<_> = (0..LEN).map(|i| (i % 251) as u8).collect();
    let (mut read, mut write) = io.split();
    let receive = async {
        assert!(head(&mut read).await.starts_with("HTTP/1.1 200"));
        let mut received = vec![0; LEN];
        read.read_exact(&mut received).await.unwrap();
        assert_eq!(received, payload);
    };
    let send = async {
        write.write_all(&payload).await.unwrap();
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(send, receive);
    })
    .await
    .unwrap();
    io.write_all(b"GET / HTTP/1.1\r\nHost: stream.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut received = String::new();
    tokio::time::timeout(Duration::from_secs(5), io.read_to_string(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert!(
        received.starts_with("HTTP/1.1 200") && received.ends_with("again"),
        "{received}"
    );
}
