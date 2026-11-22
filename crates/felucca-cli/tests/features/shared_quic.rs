use super::*;
use bytes::{Buf, Bytes};

async fn h3_client(
    port: u16,
    fingerprint: String,
    tasks: &mut tokio::task::JoinSet<()>,
) -> (
    quinn::Endpoint,
    h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
) {
    let mut tls = felucca_transport::Verification::Pinned { fingerprint }
        .rustls_client_config_tls13()
        .unwrap();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(crypto)));
    let connection = endpoint
        .connect(([127, 0, 0, 1], port).into(), "shared.test")
        .unwrap()
        .await
        .unwrap();
    let (mut driver, sender) = h3::client::new(h3_quinn::Connection::new(connection))
        .await
        .unwrap();
    tasks.spawn(async move {
        driver.wait_idle().await;
    });
    (endpoint, sender)
}

async fn request(sender: &mut h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>) {
    let mut stream = sender
        .send_request(
            http::Request::builder()
                .uri("https://shared.test/hello")
                .body(())
                .unwrap(),
        )
        .await
        .unwrap();
    stream.finish().await.unwrap();
    assert_eq!(
        stream.recv_response().await.unwrap().status(),
        http::StatusCode::OK
    );
    let mut body = Vec::new();
    while let Some(mut data) = stream.recv_data().await.unwrap() {
        body.extend_from_slice(&data.copy_to_bytes(data.remaining()));
    }
    assert!(body.starts_with(b"shared QUIC website"));
}

#[tokio::test]
async fn shared_quic_dispatches_certificates_and_preserves_http3_after_bridge_reload() {
    let bridge = identity("shared.test").await;
    let web = identity("shared.test").await;
    let next_bridge = identity("shared.test").await;
    let next_web = identity("shared.test").await;
    assert_ne!(bridge.identity.fingerprint(), web.identity.fingerprint());
    let backend = web_backend("shared QUIC website").await;
    let tcp_backend = echo().await;
    let public_tcp = free_port().await;
    let port = free_port().await;
    let mut server = server_config();
    server.bridge.bind = format!("127.0.0.1:{port}");
    server.bridge.quic_bind = Some(format!("127.0.0.1:{port}"));
    server.bridge.tls = Some(bridge.config.clone());
    server.bridge.cert_reload_interval = Duration::from_millis(30);
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: Some(format!("127.0.0.1:{port}")),
        http3_bind: Some(format!("127.0.0.1:{port}")),
        certificates: [("shared.test".into(), web.config.clone())].into(),
        cert_reload_interval: Duration::from_millis(30),
        ..Default::default()
    });
    let tunnels = format!(
        r#"
[[tunnel]]
mode = "http"
port = {port}
hosts = ["shared.test"]
backend = [{{addr="127.0.0.1:{backend}"}}]
[[tunnel]]
mode = "tcp"
bind = "127.0.0.1"
port = {public_tcp}
backend = [{{addr="127.0.0.1:{tcp_backend}"}}]
"#
    );
    let running = start(server, agent(&tunnels), TransportKind::Quic).await;
    let mut tasks = tokio::task::JoinSet::new();
    let (first_endpoint, mut first) = h3_client(port, web.identity.fingerprint(), &mut tasks).await;
    request(&mut first).await;
    roundtrip(TcpStream::connect(("127.0.0.1", public_tcp)).await.unwrap()).await;
    assert!(
        http_request(
            port,
            "GET /hello HTTP/1.1\r\nHost: shared.test\r\nConnection: close\r\n\r\n"
        )
        .await
        .contains("shared QUIC website")
    );
    // Both certificate classes reload independently without rebinding the UDP socket.
    tokio::fs::copy(&next_bridge.config.key, &bridge.config.key)
        .await
        .unwrap();
    tokio::fs::copy(&next_bridge.config.cert, &bridge.config.cert)
        .await
        .unwrap();
    tokio::fs::copy(&next_web.config.key, &web.config.key)
        .await
        .unwrap();
    tokio::fs::copy(&next_web.config.cert, &web.config.cert)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while running.server.fingerprint() != next_bridge.identity.fingerprint() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Probe TLS as a readiness signal for the shared HTTP certificate resolver.
        loop {
            let policy = felucca_transport::Verification::Pinned {
                fingerprint: next_web.identity.fingerprint(),
            };
            let io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            if policy
                .connector()
                .unwrap()
                .connect("shared.test".try_into().unwrap(), io)
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let bridge_connection = felucca_transport::quic::connect(
        &format!("127.0.0.1:{port}"),
        &felucca_transport::Verification::Pinned {
            fingerprint: next_bridge.identity.fingerprint(),
        },
    )
    .await
    .unwrap();
    let (second_endpoint, mut second) =
        h3_client(port, next_web.identity.fingerprint(), &mut tasks).await;
    request(&mut second).await;
    request(&mut first).await;
    assert!(running.agent.is_ready());
    roundtrip(TcpStream::connect(("127.0.0.1", public_tcp)).await.unwrap()).await;
    bridge_connection.close(0u32.into(), b"done");
    first_endpoint.close(0u32.into(), b"done");
    second_endpoint.close(0u32.into(), b"done");
}
