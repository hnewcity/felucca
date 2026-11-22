//! Outbound WebSocket RPC uses exactly the ordinary management request handler.
use super::{Node, ReverseIdentity, store, subscriptions::Subscriptions};
#[path = "ws_authority.rs"]
mod authority;
pub(super) use authority::Authority;
use bytes::Bytes;
use felucca_core::config::node::PlatformConfig;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    io,
    sync::Arc,
    time::Duration,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
const MAX_MESSAGE: usize = 2 * 1024 * 1024;
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Incoming {
    Request {
        id: String,
        method: String,
        path: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        body: serde_json::Value,
    },
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
}
#[derive(Debug, Serialize)]
struct Response {
    #[serde(rename = "type")]
    kind: &'static str,
    id: String,
    status: u16,
    headers: BTreeMap<String, String>,
    body: serde_json::Value,
}
struct Call {
    id: String,
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: serde_json::Value,
}
impl Response {
    fn error(id: String, status: u16, message: &str) -> Self {
        Self {
            kind: "response",
            id,
            status,
            headers: Default::default(),
            body: serde_json::json!({"error":message}),
        }
    }
}
async fn send(
    socket: &mut WebSocketStream<felucca_transport::BoxedConn>,
    value: &impl Serialize,
) -> io::Result<()> {
    let text = serde_json::to_string(value).map_err(io::Error::other)?;
    if text.len() > MAX_MESSAGE {
        return Err(io::ErrorKind::InvalidData.into());
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(text.into())),
    )
    .await
    .map_err(|_| io::ErrorKind::TimedOut)?
    .map_err(io::Error::other)
}
async fn dial(
    config: &PlatformConfig,
) -> io::Result<WebSocketStream<felucca_transport::BoxedConn>> {
    let url = config
        .reverse_url
        .as_deref()
        .ok_or(io::ErrorKind::InvalidInput)?;
    let uri = felucca_core::config::node::endpoint(url, true).map_err(io::Error::other)?;
    let host = uri
        .host()
        .ok_or(io::ErrorKind::InvalidInput)?
        .trim_matches(['[', ']']);
    let secure = uri.scheme_str() == Some("wss");
    let address = if host.contains(':') {
        format!(
            "[{host}]:{}",
            uri.port_u16().unwrap_or(if secure { 443 } else { 80 })
        )
    } else {
        format!(
            "{host}:{}",
            uri.port_u16().unwrap_or(if secure { 443 } else { 80 })
        )
    };
    let network = felucca_transport::network::Network::default();
    let tcp = network
        .bridge_tcp(&address)
        .await
        .map_err(io::Error::other)?;
    let io: felucca_transport::BoxedConn = if secure {
        let policy = felucca_service::dial::verification(&config.trust, &address);
        Box::new(
            felucca_transport::tls::handshake(tcp, host, &policy, &[b"http/1.1"])
                .await
                .map_err(io::Error::other)?,
        )
    } else {
        Box::new(tcp)
    };
    let mut request = url.into_client_request().map_err(io::Error::other)?;
    let mut token = http::HeaderValue::from_str(&format!("Bearer {}", config.token.expose()))
        .map_err(io::Error::other)?;
    token.set_sensitive(true);
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, token);
    request.headers_mut().insert(
        http::header::SEC_WEBSOCKET_PROTOCOL,
        http::HeaderValue::from_static("felucca-node-v1"),
    );
    let (socket, response) =
        tokio_tungstenite::client_async_with_config(request, io, Some(options()))
            .await
            .map_err(io::Error::other)?;
    if response
        .headers()
        .get(http::header::SEC_WEBSOCKET_PROTOCOL)
        .is_none_or(|h| h != "felucca-node-v1")
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(socket)
}
pub(super) async fn run(node: Arc<Node>, id: String, config: PlatformConfig, generation: [u8; 32]) {
    let mut backoff = Duration::from_secs(1);
    loop {
        node.note(&id, generation, |status| {
            status.attempts = status.attempts.saturating_add(1)
        });
        let connect = tokio::select! {
            ()=node.cancel.cancelled()=>return,
            result=tokio::time::timeout(Duration::from_secs(15),dial(&config))=>result,
        };
        if let Ok(Ok(socket)) = connect {
            node.note(&id, generation, |status| {
                status.connected = true;
                status.last_connected_ms = Some(store::now());
                status.last_error = None;
            });
            let authority = Authority::Reverse {
                identity: ReverseIdentity {
                    id: id.clone(),
                    generation,
                },
                config: Arc::new(config.clone()),
            };
            let result = tokio::select! { ()=node.cancel.cancelled()=>return,result=drive(node.clone(),authority,socket)=>result };
            node.note(&id, generation, |status| {
                status.connected = false;
                status.last_error = result.err().map(|_| "reverse channel closed".into());
            });
            backoff = Duration::from_secs(1);
        } else {
            node.note(&id, generation, |status| {
                status.connected = false;
                status.last_error = Some("reverse connection failed".into());
            });
        }
        tokio::select! { ()=node.cancel.cancelled()=>return,()=tokio::time::sleep(backoff)=>{} }
        backoff = backoff.saturating_mul(2).min(Duration::from_secs(30));
    }
}
pub(super) fn options() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE))
        .max_write_buffer_size(MAX_MESSAGE * 2)
}
// Erase the future type at the upgrade/RPC recursion boundary.
pub(super) fn drive(
    node: Arc<Node>,
    authority: Authority,
    socket: WebSocketStream<felucca_transport::BoxedConn>,
) -> futures_util::future::BoxFuture<'static, io::Result<()>> {
    Box::pin(async move { session(&node, authority, socket).await })
}
async fn session(
    node: &Arc<Node>,
    authority: Authority,
    mut socket: WebSocketStream<felucca_transport::BoxedConn>,
) -> io::Result<()> {
    authority.current(node)?;
    let heartbeat = authority.heartbeat();
    let state = node.store.snapshot();
    send(&mut socket,&serde_json::json!({"type":"hello","version":1,"node":node.id,"boot":node.boot,"epoch":state.epoch,
        "cursor":format!("{}:{}",state.epoch,state.next-1),"max_message":MAX_MESSAGE,"max_inflight":2,"max_subscriptions":32,"heartbeat_ms":heartbeat.as_millis()})).await?;
    let mut events = node.events.subscribe();
    let mut reports = node.reports.subscribe();
    let mut requests = tokio::task::JoinSet::new();
    let mut active = HashSet::new();
    let mut subscriptions = Subscriptions::default();
    let mut reauthorize = tokio::time::interval(Duration::from_secs(1));
    reauthorize.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ping = tokio::time::interval(heartbeat);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ping.tick().await;
    let mut pending: Option<(u64, tokio::time::Instant)> = None;
    let mut nonce = 0u64;
    let usage_interval = node.config()?.1.usage_interval;
    let mut usage = tokio::time::interval(usage_interval.unwrap_or(Duration::from_secs(3600)));
    usage.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    usage.tick().await;
    loop {
        tokio::select! {
            item=socket.next()=>{
                let item=item.ok_or(io::ErrorKind::UnexpectedEof)?.map_err(io::Error::other)?;
                let text=match item {
                    Message::Text(text)=>text,
                    Message::Ping(bytes)=>{socket.send(Message::Pong(bytes)).await.map_err(io::Error::other)?;continue;},
                    Message::Pong(_)=>continue,
                    Message::Close(_)=>return Ok(()),
                    _=>return Err(io::ErrorKind::InvalidData.into()),
                };
                let value:Incoming=serde_json::from_str(&text).map_err(|_|io::ErrorKind::InvalidData)?;
                match value {
                    Incoming::Ping{nonce}=>send(&mut socket,&serde_json::json!({"type":"pong","nonce":nonce})).await?,
                    Incoming::Pong{nonce}=>{
                        if pending.is_some_and(|(expected,_)|expected==nonce) {
                            pending=None;authority.note(node,|status|status.last_pong_ms=Some(store::now()));
                        }
                    }
                    Incoming::Request{id:request_id,method,path,headers,body}=>{
                        if request_id.is_empty() || request_id.len()>128 || active.contains(&request_id) {return Err(io::ErrorKind::InvalidData.into());}
                        let principal=authority.current(node)?;
                        if path.starts_with("/api/realtime/subscriptions") {
                            let daemon=node.daemon.upgrade().ok_or(io::ErrorKind::NotConnected)?;
                            let (status,body)=subscriptions.request(&method,&path,body,&principal,&daemon.config().1);
                            send(&mut socket,&Response{kind:"response",id:request_id,status,body,headers:Default::default()}).await?;
                            continue;
                        }
                        let slot=node.rpc_slots.clone().try_acquire_owned();
                        if requests.len()>=2 || slot.is_err() {
                            send(&mut socket,&Response::error(request_id,429,"node RPC capacity reached")).await?;continue;
                        }
                        active.insert(request_id.clone());
                        let node=node.clone();let authority=authority.clone();
                        authority.note(&node,|status|status.requests=status.requests.saturating_add(1));
                        requests.spawn(async move {
                            let _slot=slot;
                            rpc(node,authority,Call{id:request_id,method,path,headers,body}).await
                        });
                    }
                }
            }
            response=requests.join_next(),if !requests.is_empty()=>{
                let response=response.expect("nonempty tasks").map_err(io::Error::other)?;
                active.remove(&response.id);send(&mut socket,&response).await?;
                authority.note(node,|status|status.responses=status.responses.saturating_add(1));
            }
            event=events.recv()=>{
                match event {
                    Ok(event)=>{
                        let principal=authority.current(node)?;
                        let event=serde_json::to_value(event).expect("event");
                        if visible(&event,&principal) {
                            emit(&mut socket,&subscriptions,serde_json::json!({"type":"event","node":node.id,"id":uuid::Uuid::new_v4(),"epoch":node.store.snapshot().epoch,"ephemeral":event.get("ephemeral").and_then(serde_json::Value::as_bool).unwrap_or(false),"event":event})).await?;
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>send(&mut socket,&serde_json::json!({"type":"resync","epoch":node.store.snapshot().epoch})).await?,
                    Err(_)=>return Ok(()),
                }
            }
            report=reports.recv()=>{
                match report {
                    Ok(report)=>{
                        let principal=authority.current(node)?;
                        if crate::control::resources::visible(&principal,report.user.as_deref()) {
                            emit(&mut socket,&subscriptions,serde_json::json!({"type":"event","node":node.id,"id":uuid::Uuid::new_v4(),"ephemeral":true,"event":report})).await?;
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{},
                    Err(_)=>return Ok(()),
                }
            }
            _=ping.tick()=>{
                if let Some((_,started))=pending {
                    if started.elapsed()>=heartbeat*2 {return Err(io::ErrorKind::TimedOut.into());}
                } else {
                    nonce=nonce.checked_add(1).ok_or(io::ErrorKind::InvalidData)?;
                    pending=Some((nonce,tokio::time::Instant::now()));
                    send(&mut socket,&serde_json::json!({"type":"ping","nonce":nonce})).await?;
                }
            }
            _=usage.tick(),if usage_interval.is_some()=>{
                let snapshot=node.usage(&authority.current(node)?).map_err(|_|io::ErrorKind::PermissionDenied)?;
                send(&mut socket,&serde_json::json!({"type":"usage","snapshot":snapshot})).await?;
            }
            _=reauthorize.tick()=>{authority.current(node)?;},
        }
    }
}
fn visible(event: &serde_json::Value, principal: &crate::control::Principal) -> bool {
    event.get("resource").is_none_or(serde_json::Value::is_null)
        || crate::control::resources::visible(
            principal,
            event.get("user").and_then(serde_json::Value::as_str),
        )
}
async fn emit(
    socket: &mut WebSocketStream<felucca_transport::BoxedConn>,
    subscriptions: &Subscriptions,
    envelope: serde_json::Value,
) -> io::Result<()> {
    send(socket, &envelope).await?;
    for callback in subscriptions.callbacks(&envelope) {
        send(socket, &callback).await?;
    }
    Ok(())
}
async fn rpc(node: Arc<Node>, authority: Authority, call: Call) -> Response {
    let Call {
        id,
        method,
        path,
        headers,
        body,
    } = call;
    let result = tokio::time::timeout(Duration::from_secs(40), async {
        let Some(admin) = node.admin.upgrade() else {
            return Response::error(id.clone(), 503, "management unavailable");
        };
        let uri: http::Uri = match path.parse() {
            Ok(uri) => uri,
            Err(_) => return Response::error(id.clone(), 400, "invalid RPC path"),
        };
        if uri.authority().is_some()
            || uri.scheme().is_some()
            || !uri.path().starts_with("/api/")
            || matches!(
                uri.path(),
                "/api/login" | "/api/logout" | "/api/events" | "/api/profile" | "/api/ws"
            )
            || path.len() > 4096
            || headers.len() > 8
        {
            return Response::error(id.clone(), 400, "RPC endpoint is not permitted");
        }
        let Some(daemon) = admin.daemon.upgrade() else {
            return Response::error(id.clone(), 503, "daemon unavailable");
        };
        let daemon_config = daemon.config().1;
        let Some(settings) = daemon_config.admin.as_ref() else {
            return Response::error(id.clone(), 503, "management unavailable");
        };
        let host = settings
            .origin
            .as_deref()
            .and_then(|origin| origin.strip_prefix("https://"))
            .unwrap_or(&settings.bind);
        let bytes = match serde_json::to_vec(&body) {
            Ok(bytes) if bytes.len() <= MAX_MESSAGE => bytes,
            _ => return Response::error(id.clone(), 413, "RPC body too large"),
        };
        let mut request = http::Request::builder()
            .method(method.as_str())
            .uri(uri)
            .header(http::header::HOST, host)
            .header(http::header::CONTENT_TYPE, "application/json");
        for (name, value) in authority.headers().iter() {
            request = request.header(name, value);
        }
        for (name, value) in headers {
            if !matches!(
                name.to_ascii_lowercase().as_str(),
                "if-match" | "idempotency-key" | "accept" | "x-felucca-csrf"
            ) || value.len() > 256
            {
                return Response::error(id.clone(), 400, "RPC header is not permitted");
            }
            request = request.header(name, value);
        }
        let body = Full::new(Bytes::from(bytes))
            .map_err(
                |error: std::convert::Infallible| -> crate::http_support::BoxError {
                    match error {}
                },
            )
            .boxed_unsync();
        let request = match request.body(body) {
            Ok(request) => request,
            Err(_) => return Response::error(id.clone(), 400, "invalid RPC request"),
        };
        let reverse = match &authority {
            Authority::Reverse { identity, .. } => Some(identity),
            Authority::Direct { .. } => None,
        };
        let response = admin
            .request_body(request, std::net::Ipv4Addr::LOCALHOST.into(), reverse)
            .await;
        let (parts, body) = response.into_parts();
        let bytes = match crate::admin::api::collect(body, MAX_MESSAGE - 1024).await {
            Ok(body) => body,
            Err(_) => return Response::error(id.clone(), 413, "RPC response exceeds limit"),
        };
        let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
        });
        let headers = parts
            .headers
            .iter()
            .filter(|(key, _)| matches!(key.as_str(), "content-type" | "etag"))
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.to_string(), v.to_owned()))
            })
            .collect();
        Response {
            kind: "response",
            id: id.clone(),
            status: parts.status.as_u16(),
            headers,
            body,
        }
    })
    .await;
    result.unwrap_or_else(|_| {
        Response::error(
            id,
            504,
            "RPC timed out; retry mutations with the same idempotency key",
        )
    })
}
