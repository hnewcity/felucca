use super::*;
use bytes::{Buf, Bytes};
use futures_util::{SinkExt, StreamExt};
use http_body_util::Empty;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::{
    convert::Infallible,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

async fn echo_frames(
    io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    closed: Arc<AtomicUsize>,
) {
    let mut ws = WebSocketStream::from_raw_socket(io, Role::Server, None).await;
    ws.send(Message::Binary(Bytes::from_static(b"ready")))
        .await
        .unwrap();
    while let Some(Ok(message)) = ws.next().await {
        let result = match message {
            Message::Binary(data) => ws.send(Message::Binary(data)).await,
            Message::Ping(_) => ws.flush().await,
            Message::Close(_) => {
                let _ = ws.flush().await;
                break;
            }
            _ => Ok(()),
        };
        if result.is_err() {
            break;
        }
    }
    closed.fetch_add(1, Ordering::SeqCst);
}

#[allow(
    clippy::result_large_err,
    reason = "Tungstenite fixes its handshake callback error type to an HTTP response"
)]
async fn backend(h2: bool, tasks: &mut tokio::task::JoinSet<()>, closed: Arc<AtomicUsize>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tasks.spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        while let Ok((io, _)) = listener.accept().await {
            let closed = closed.clone();
            connections.spawn(async move {
                if h2 {
                    let service = hyper::service::service_fn(
                        move |mut request: http::Request<hyper::body::Incoming>| {
                            assert_eq!(request.method(), http::Method::CONNECT);
                            assert_eq!(
                                request
                                    .extensions()
                                    .get::<hyper::ext::Protocol>()
                                    .unwrap()
                                    .as_str(),
                                "websocket"
                            );
                            assert_eq!(request.uri().path_and_query().unwrap(), "/v2/socket?q=1");
                            assert_eq!(request.uri().host(), Some("backend.test"));
                            assert!(!request.headers().contains_key("sec-websocket-key"));
                            assert!(!request.headers().contains_key("upgrade"));
                            assert_eq!(request.headers()["sec-websocket-version"], "13");
                            let upgrade = hyper::upgrade::on(&mut request);
                            let closed = closed.clone();
                            tokio::spawn(async move {
                                if let Ok(io) = upgrade.await {
                                    echo_frames(TokioIo::new(io), closed).await;
                                }
                            });
                            async move {
                                Ok::<_, Infallible>(http::Response::new(Empty::<Bytes>::new()))
                            }
                        },
                    );
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .enable_connect_protocol()
                        .serve_connection(TokioIo::new(io), service)
                        .await;
                } else {
                    let ws = tokio_tungstenite::accept_hdr_async(
                        io,
                        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                         response| {
                            assert_eq!(request.uri(), "/v2/socket?q=1");
                            assert_eq!(request.headers()["host"], "backend.test");
                            Ok(response)
                        },
                    )
                    .await
                    .unwrap();
                    // Use the established socket directly; its read buffer may already contain frames.
                    let mut ws = ws;
                    ws.send(Message::Binary(Bytes::from_static(b"ready")))
                        .await
                        .unwrap();
                    while let Some(Ok(message)) = ws.next().await {
                        let result = match message {
                            Message::Binary(data) => ws.send(Message::Binary(data)).await,
                            Message::Ping(_) => ws.flush().await,
                            Message::Close(_) => {
                                let _ = ws.flush().await;
                                break;
                            }
                            _ => Ok(()),
                        };
                        if result.is_err() {
                            break;
                        }
                    }
                    closed.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });
    port
}

async fn exchange(io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin) {
    let ws = WebSocketStream::from_raw_socket(io, Role::Client, None).await;
    exchange_socket(ws).await;
}

async fn exchange_socket<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut ws: WebSocketStream<T>,
) {
    assert_eq!(
        ws.next().await.unwrap().unwrap(),
        Message::Binary(Bytes::from_static(b"ready"))
    );
    for len in [0, 7, 2 * 1024 * 1024] {
        let data = Bytes::from((0..len).map(|i| (i % 251) as u8).collect::<Vec<_>>());
        ws.send(Message::Binary(data.clone())).await.unwrap();
        assert_eq!(ws.next().await.unwrap().unwrap(), Message::Binary(data));
    }
    ws.send(Message::Ping(Bytes::from_static(b"ping")))
        .await
        .unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap(),
        Message::Pong(Bytes::from_static(b"ping"))
    );
    ws.close(None).await.unwrap();
    assert!(matches!(
        ws.next().await.unwrap().unwrap(),
        Message::Close(_)
    ));
}

async fn run(front: u8, back_h2: bool) {
    let certificate = identity("websocket.test").await;
    let mut tasks = tokio::task::JoinSet::new();
    let closed = Arc::new(AtomicUsize::new(0));
    let backend_port = backend(back_h2, &mut tasks, closed.clone()).await;
    let tcp_port = free_port().await;
    let udp_port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{tcp_port}")),
        http3_bind: Some(format!("127.0.0.1:{udp_port}")),
        tls_bind: None,
        certificates: [("websocket.test".into(), certificate.config.clone())].into(),
        ..Default::default()
    });
    let config = agent(&format!(
        r#"
[[tunnel]]
mode = "http"
port = {tcp_port}
hosts = ["websocket.test"]
backend = [{{addr="127.0.0.1:{backend_port}"}}]
[tunnel.options.http]
path = "/entry/"
rewrite_prefix = "/v2/"
host = "backend.test"
backend_http2 = {back_h2}
[tunnel.options.payload]
compress = true
encrypt = true
"#
    ));
    let running = start(server, config, TransportKind::Quic).await;
    let mut endpoint = None;
    let test = async {
        match front {
            1 => {
                let io = TcpStream::connect(("127.0.0.1", tcp_port)).await.unwrap();
                let (ws, response) =
                    tokio_tungstenite::client_async("ws://websocket.test/entry/socket?q=1", io)
                        .await
                        .unwrap();
                assert_eq!(response.status(), http::StatusCode::SWITCHING_PROTOCOLS);
                exchange_socket(ws).await;
            }
            2 => {
                let io = TcpStream::connect(("127.0.0.1", tcp_port)).await.unwrap();
                let (mut sender, mut driver) =
                    hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(io))
                        .await
                        .unwrap();
                tokio::time::timeout(Duration::from_secs(3), async {
                    while !driver.is_extended_connect_protocol_enabled() {
                        // Hyper exposes a snapshot, not a setting-change notification.
                        tokio::select! {
                            result = &mut driver => panic!("h2 closed before SETTINGS: {result:?}"),
                            () = tokio::time::sleep(Duration::from_millis(1)) => {},
                        }
                    }
                })
                .await
                .expect("server did not enable extended CONNECT");
                tasks.spawn(async move {
                    let _ = driver.await;
                });
                let mut request = http::Request::builder()
                    .method("CONNECT")
                    .uri("http://websocket.test/entry/socket?q=1")
                    .version(http::Version::HTTP_2)
                    .header("sec-websocket-version", "13")
                    .body(Empty::<Bytes>::new())
                    .unwrap();
                request
                    .extensions_mut()
                    .insert(hyper::ext::Protocol::from_static("websocket"));
                let mut response = sender.send_request(request).await.unwrap();
                assert_eq!(response.status(), http::StatusCode::OK);
                assert!(!response.headers().contains_key("sec-websocket-accept"));
                exchange(TokioIo::new(
                    hyper::upgrade::on(&mut response).await.unwrap(),
                ))
                .await;
            }
            3 => {
                let mut tls = felucca_transport::Verification::Pinned {
                    fingerprint: certificate.identity.fingerprint(),
                }
                .rustls_client_config_tls13()
                .unwrap();
                tls.alpn_protocols = vec![b"h3".to_vec()];
                let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
                let mut quic = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
                quic.set_default_client_config(quinn::ClientConfig::new(Arc::new(crypto)));
                let connection = quic
                    .connect(([127, 0, 0, 1], udp_port).into(), "websocket.test")
                    .unwrap()
                    .await
                    .unwrap();
                let (mut driver, mut sender) =
                    h3::client::new(h3_quinn::Connection::new(connection))
                        .await
                        .unwrap();
                endpoint = Some(quic);
                tasks.spawn(async move {
                    driver.wait_idle().await;
                });
                let mut request = http::Request::builder()
                    .method("CONNECT")
                    .uri("https://websocket.test/entry/socket?q=1")
                    .header("sec-websocket-version", "13")
                    .body(())
                    .unwrap();
                request
                    .extensions_mut()
                    .insert(h3::ext::Protocol::WEBSOCKET);
                let mut stream = sender.send_request(request).await.unwrap();
                let response = stream.recv_response().await.unwrap();
                assert_eq!(response.status(), http::StatusCode::OK);
                assert!(!response.headers().contains_key("sec-websocket-accept"));
                let (io, proxy) = tokio::io::duplex(65536);
                let (mut reader, mut writer) = tokio::io::split(proxy);
                let (mut send, mut receive) = stream.split();
                tasks.spawn(async move {
                    let upload = async {
                        let mut buffer = [0; 16384];
                        loop {
                            let n = reader.read(&mut buffer).await.unwrap();
                            if n == 0 {
                                break;
                            }
                            send.send_data(Bytes::copy_from_slice(&buffer[..n]))
                                .await
                                .unwrap();
                        }
                        let _ = send.finish().await;
                    };
                    let download = async {
                        while let Some(mut data) = receive.recv_data().await.unwrap() {
                            let data = data.copy_to_bytes(data.remaining());
                            if writer.write_all(&data).await.is_err() {
                                break;
                            }
                        }
                        let _ = writer.shutdown().await;
                    };
                    tokio::join!(upload, download);
                });
                exchange(io).await;
            }
            _ => unreachable!(),
        }
        while closed.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .unwrap();
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    assert!(
        running
            .server
            .registry()
            .clients()
            .iter()
            .any(|client| client.traffic.stats().inlet >= 2 * 1024 * 1024)
    );
    if let Some(endpoint) = endpoint {
        endpoint.close(0u32.into(), b"done");
    }
}

#[tokio::test]
async fn websocket_h1_to_h1() {
    run(1, false).await;
}
#[tokio::test]
async fn websocket_h1_to_h2() {
    run(1, true).await;
}
#[tokio::test]
async fn websocket_h2_to_h1() {
    run(2, false).await;
}
#[tokio::test]
async fn websocket_h2_to_h2() {
    run(2, true).await;
}
#[tokio::test]
async fn websocket_h3_to_h1() {
    run(3, false).await;
}
#[tokio::test]
async fn websocket_h3_to_h2() {
    run(3, true).await;
}

#[tokio::test]
async fn http3_adapter_can_cancel_and_identify_a_pending_receive() {
    use h3::quic::{BidiStream, Connection, RecvStream};
    let identity = felucca_transport::Identity::self_signed(vec!["localhost".into()]).unwrap();
    let endpoint =
        felucca_transport::quic::server(&identity, "127.0.0.1:0".parse().unwrap()).unwrap();
    let address = endpoint.local_addr().unwrap().to_string();
    let incoming = async { endpoint.accept().await.unwrap().await.unwrap() };
    let policy = felucca_transport::Verification::Pinned {
        fingerprint: identity.fingerprint(),
    };
    let (server, client) = tokio::join!(
        incoming,
        felucca_transport::quic::connect(&address, &policy)
    );
    let client = client.unwrap();
    let mut adapter = h3_quinn::Connection::new(server);
    let (mut send, _receive) = client.open_bi().await.unwrap();
    send.write_all(b"x").await.unwrap();
    let stream = std::future::poll_fn(|cx| Connection::<Bytes>::poll_accept_bidi(&mut adapter, cx))
        .await
        .unwrap();
    let (_, mut receive) = BidiStream::<Bytes>::split(stream);
    assert_eq!(
        std::future::poll_fn(|cx| receive.poll_data(cx))
            .await
            .unwrap()
            .unwrap(),
        b"x"[..]
    );
    let id = receive.recv_id();
    std::future::poll_fn(|cx| {
        assert!(receive.poll_data(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    // Both calls previously panicked because the stream was moved into the pending future.
    assert_eq!(receive.recv_id(), id);
    receive.stop_sending(42);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), send.stopped())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_inner(),
        42
    );
    let (mut second, _receive) = client.open_bi().await.unwrap();
    second.write_all(b"healthy").await.unwrap();
    let stream = std::future::poll_fn(|cx| Connection::<Bytes>::poll_accept_bidi(&mut adapter, cx))
        .await
        .unwrap();
    let (_, mut receive) = BidiStream::<Bytes>::split(stream);
    assert_eq!(
        std::future::poll_fn(|cx| receive.poll_data(cx))
            .await
            .unwrap()
            .unwrap(),
        b"healthy"[..]
    );
    endpoint.close(0u32.into(), b"done");
}

#[tokio::test]
async fn websocket_rejects_bad_frontend_keys_and_backend_accept_proofs() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        while let Ok((mut io, _)) = listener.accept().await {
            let _ = head(&mut io).await;
            counted.fetch_add(1, Ordering::SeqCst);
            io.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: wrong\r\n\r\n")
                .await.unwrap();
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
        "[[tunnel]]\nmode='http'\nport={port}\nhosts=['bad.test']\nbackend=[{{addr='127.0.0.1:{backend}'}}]"
    )), TransportKind::Tls).await;
    for headers in [
        "Sec-WebSocket-Version: 13\r\n",
        "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: invalid\r\n",
        "Sec-WebSocket-Version: 12\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
    ] {
        let response = http_request(port, &format!("GET / HTTP/1.1\r\nHost: bad.test\r\nConnection: Upgrade, close\r\nUpgrade: websocket\r\n{headers}\r\n")).await;
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(b"GET / HTTP/1.1\r\nHost: bad.test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), head(&mut io))
        .await
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 502"), "{response}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
