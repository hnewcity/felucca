//! Shared platform fixtures; test cases live in the feature suites.
use super::admin::*;
use super::*;
use felucca_core::config::node::NodeConfig;
use felucca_server::Daemon;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
pub(crate) const FULL: &str = "full-platform-credential";
pub(crate) const TENANT: &str = "team-a-platform-credential";
pub(crate) const REVERSE: &str = "reverse-platform-credential";
pub(crate) const SIGNING: &str = "callback-signature-credential";
pub(crate) fn configure(fixture: &mut Fixture, extra: Value) {
    let mut config = json!({
        "id":"edge-test", "state_path":fixture.files.root.join("node.json"),
        "changes_window":100,
        "platform":{
            "full":{"token":FULL,"scope":"full"},
            "team":{"token":TENANT,"scope":"account","user":"team-a"},
            "reverse":{"token":REVERSE,"scope":"account","user":"team-a",
                "mode":"reverse","reverse_url":"ws://127.0.0.1:1/control"},
            "disabled":{"token":"disabled-platform-credential","scope":"full","enabled":false}
        }
    });
    for (key, value) in extra.as_object().unwrap() {
        config[key] = value.clone();
    }
    fixture.config.node = Some(serde_json::from_value::<NodeConfig>(config).unwrap());
}
pub(crate) async fn stopped(daemon: &Arc<Daemon>) {
    tokio::time::timeout(Duration::from_secs(10), daemon.shutdown())
        .await
        .expect("node shutdown must release all connectors and journal writers")
        .unwrap();
}
pub(crate) struct Receiver {
    pub(crate) port: u16,
    pub(crate) success: Arc<AtomicBool>,
    pub(crate) records: tokio::sync::mpsc::UnboundedReceiver<(http::HeaderMap, Vec<u8>)>,
    pub(crate) task: tokio::task::JoinHandle<()>,
    pub(crate) block_next: Arc<AtomicBool>,
    pub(crate) gate: Arc<tokio::sync::Semaphore>,
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(crate) async fn receiver() -> Receiver {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let success = Arc::new(AtomicBool::new(false));
    let status = success.clone();
    let (tx, records) = tokio::sync::mpsc::unbounded_channel();
    let block_next = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let held = block_next.clone();
    let release = gate.clone();
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                connection=listener.accept()=>{
                    let (io, _)=connection.unwrap();
                    let success=status.clone();let tx=tx.clone();let held=held.clone();let release=release.clone();
                    connections.spawn(async move {
                        let service=hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
                            let success=success.clone();let tx=tx.clone();let held=held.clone();let release=release.clone();
                            async move {
                                let (parts,body)=request.into_parts();
                                let bytes=body.collect().await.unwrap().to_bytes();
                                let status=if success.load(Ordering::Acquire){200}else{503};
                                tx.send((parts.headers,bytes.to_vec())).unwrap();
                                if held.swap(false,Ordering::AcqRel){release.acquire_owned().await.unwrap().forget();}
                                Ok::<_, std::convert::Infallible>(http::Response::builder().status(status)
                                    .body(http_body_util::Full::new(bytes::Bytes::new())).unwrap())
                            }
                        });
                        let _=hyper::server::conn::http1::Builder::new()
                            .serve_connection(hyper_util::rt::TokioIo::new(io),service).await;
                    });
                },
                _=connections.join_next(),if !connections.is_empty()=>{}
            }
        }
    });
    Receiver {
        port,
        success,
        records,
        task,
        block_next,
        gate,
    }
}
pub(crate) async fn failed_delivery(api: &Api) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = api.get("/api/node/callbacks").await;
            assert_eq!(response.status, 200, "{}", response.raw);
            for queue in response.body["queues"].as_array().unwrap() {
                for item in queue["items"].as_array().unwrap() {
                    if item["failed"] == true {
                        return item.clone();
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}
pub(crate) fn verify_signature(headers: &http::HeaderMap, body: &[u8]) {
    let prefix = format!(
        "felucca-callback-v1\n{}\n{}\n",
        headers["x-felucca-timestamp"].to_str().unwrap(),
        headers["x-felucca-event-id"].to_str().unwrap()
    );
    let mut data = prefix.into_bytes();
    data.extend_from_slice(body);
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, SIGNING.as_bytes());
    let digest = ring::hmac::sign(&key, &data);
    let expected = format!(
        "v1={}",
        digest
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    assert_eq!(headers["x-felucca-signature"], expected);
    let event: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(
        event["id"].as_str().unwrap(),
        headers["x-felucca-event-id"].to_str().unwrap()
    );
}
#[allow(clippy::result_large_err)] // tungstenite's handshake callback fixes the error type.
pub(crate) async fn accept_reverse(listener: &TcpListener) -> WebSocketStream<TcpStream> {
    let (io, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    tokio_tungstenite::accept_hdr_async(
        io,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
         mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            assert_eq!(
                request.headers()["authorization"],
                format!("Bearer {REVERSE}")
            );
            assert_eq!(
                request.headers()["sec-websocket-protocol"],
                "felucca-node-v1"
            );
            response.headers_mut().insert(
                http::header::SEC_WEBSOCKET_PROTOCOL,
                http::HeaderValue::from_static("felucca-node-v1"),
            );
            Ok(response)
        },
    )
    .await
    .unwrap()
}
pub(crate) async fn message<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    kind: &str,
) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let item = socket.next().await.unwrap().unwrap();
            if let Message::Text(text) = item {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["type"] == kind {
                    return value;
                }
                if value["type"] == "ping" {
                    socket
                        .send(Message::Text(
                            json!({"type":"pong","nonce":value["nonce"]})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                }
            }
        }
    })
    .await
    .unwrap()
}
pub(crate) async fn rpc<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    request: Value,
) -> Value {
    socket
        .send(Message::Text(request.to_string().into()))
        .await
        .unwrap();
    let response = message(socket, "response").await;
    assert_eq!(response["id"], request["id"]);
    response
}
