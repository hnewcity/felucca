//! Authenticated incoming management WebSockets.
use super::{Node, ws};
use crate::admin::{Result, bad, forbidden};
use crate::control::Principal;
use crate::http_support::Body;
use base64::Engine;
use http::{Request, Response, StatusCode, header};
use std::{sync::Arc, time::Duration};

/// Browsers cannot set Authorization on WebSocket handshakes. A token can be
/// carried in a non-selected subprotocol, never in a URL or echoed in a response.
pub(crate) fn browser_token(headers: &mut http::HeaderMap) -> Result<()> {
    let mut token = None;
    for value in headers.get_all(header::SEC_WEBSOCKET_PROTOCOL) {
        let value = value.to_str().map_err(|_| bad("invalid WS subprotocol"))?;
        for protocol in value.split(',').map(str::trim) {
            if let Some(encoded) = protocol.strip_prefix("felucca-token.") {
                if token.is_some() || encoded.len() > 5500 {
                    return Err(bad("invalid WS token protocol"));
                }
                let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(encoded)
                    .map_err(|_| bad("invalid WS token encoding"))?;
                if !(16..=4096).contains(&bytes.len()) || !bytes.iter().all(u8::is_ascii_graphic) {
                    return Err(bad("invalid WS token"));
                }
                let mut value =
                    http::HeaderValue::from_bytes(&[b"Bearer ".as_slice(), &bytes].concat())
                        .map_err(|_| bad("invalid WS token"))?;
                value.set_sensitive(true);
                token = Some(value);
            }
        }
    }
    if let Some(token) = token {
        if headers.contains_key(header::AUTHORIZATION) {
            return Err(bad("multiple WS credentials"));
        }
        headers.insert(header::AUTHORIZATION, token);
    }
    Ok(())
}
impl Node {
    pub(crate) fn upgrade(
        self: Arc<Self>,
        mut request: Request<Body>,
        principal: Principal,
    ) -> Result<Response<Body>> {
        if request.version() != http::Version::HTTP_11
            || request.method() != http::Method::GET
            || !crate::http::wants_upgrade(request.headers())
            || request
                .headers()
                .get(header::UPGRADE)
                .is_none_or(|v| !v.as_bytes().eq_ignore_ascii_case(b"websocket"))
            || request
                .headers()
                .get_all(header::SEC_WEBSOCKET_VERSION)
                .iter()
                .count()
                != 1
            || request
                .headers()
                .get(header::SEC_WEBSOCKET_VERSION)
                .is_none_or(|v| v != "13")
            || request
                .headers()
                .get_all(header::SEC_WEBSOCKET_KEY)
                .iter()
                .count()
                != 1
        {
            return Err(bad(
                "management WS requires an HTTP/1.1 WebSocket v13 handshake",
            ));
        }
        if request.headers().contains_key(header::COOKIE)
            && !request.headers().contains_key(header::ORIGIN)
        {
            return Err(forbidden());
        }
        let key = request.headers()[header::SEC_WEBSOCKET_KEY]
            .to_str()
            .map_err(|_| bad("invalid WS key"))?;
        if !base64::engine::general_purpose::STANDARD
            .decode(key)
            .is_ok_and(|bytes| bytes.len() == 16)
        {
            return Err(bad("invalid WS key"));
        }
        let protocols = request
            .headers()
            .get_all(header::SEC_WEBSOCKET_PROTOCOL)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .collect::<Vec<_>>();
        if !protocols.contains(&"felucca-node-v1") || protocols.len() > 8 {
            return Err(bad("WS requires felucca-node-v1 subprotocol"));
        }
        let slot = self.ws_slots.clone().try_acquire_owned().map_err(|_| {
            crate::admin::Error(
                StatusCode::TOO_MANY_REQUESTS,
                "management WS capacity reached".into(),
            )
        })?;
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
        let mut headers = http::HeaderMap::new();
        for name in [header::AUTHORIZATION, header::COOKIE, header::ORIGIN] {
            if let Some(value) = request.headers().get(&name) {
                headers.insert(name, value.clone());
            }
        }
        let authority = ws::Authority::Direct { headers, principal };
        let upgraded = hyper::upgrade::on(&mut request);
        let node = self.clone();
        self.tasks.spawn(async move {
            let _slot=slot;
            let upgraded=tokio::select! {
                ()=node.cancel.cancelled()=>return,
                upgraded=tokio::time::timeout(Duration::from_secs(15),upgraded)=>match upgraded {Ok(Ok(io))=>io,_=>return},
            };
            let io:felucca_transport::BoxedConn=Box::new(hyper_util::rt::TokioIo::new(upgraded));
            let socket=tokio_tungstenite::WebSocketStream::from_raw_socket(io,tokio_tungstenite::tungstenite::protocol::Role::Server,Some(ws::options())).await;
            tokio::select! {
                ()=node.cancel.cancelled()=>{},
                _=ws::drive(node.clone(),authority,socket)=>{},
            }
        });
        Ok(Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_ACCEPT, accept)
            .header(header::SEC_WEBSOCKET_PROTOCOL, "felucca-node-v1")
            .body(crate::http_support::response(StatusCode::OK, "").into_body())
            .expect("valid WS handshake"))
    }
}
