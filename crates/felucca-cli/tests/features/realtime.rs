use super::*;
use crate::support::admin::*;
use crate::support::platform::*;
use base64::Engine;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
type Socket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
fn request(port: u16, token: Option<&str>) -> http::Request<()> {
    let mut request = format!("ws://127.0.0.1:{port}/api/ws")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        http::header::SEC_WEBSOCKET_PROTOCOL,
        http::HeaderValue::from_static("felucca-node-v1"),
    );
    if let Some(token) = token {
        request.headers_mut().insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    request
}
async fn open(request: http::Request<()>) -> Socket {
    let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.status(), 101);
    assert_eq!(
        response.headers()["sec-websocket-protocol"],
        "felucca-node-v1"
    );
    assert_eq!(message(&mut socket, "hello").await["max_subscriptions"], 32);
    socket
}
async fn closed(socket: &mut Socket) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = socket.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await
    .expect("revoked WS must close");
}
fn code(error: tokio_tungstenite::tungstenite::Error) -> u16 {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => response.status().as_u16(),
        e => panic!("expected HTTP rejection, got {e}"),
    }
}
#[tokio::test]
async fn direct_ws_filters_tenant_events_renders_subscription_and_webhook_and_releases_state() {
    let mut receiver = receiver().await;
    receiver.success.store(true, Ordering::Release);
    let mut fixture = Fixture::new().await;
    configure(
        &mut fixture,
        json!({"webhook":{"selected":{"user":"team-a","delivery":{
            "url":format!("http://127.0.0.1:{}/selected",receiver.port),"signing_key":SIGNING,
            "selector":{"event_names":["client.updated"],"clients":["features"]},
            "content":{"body_template":{"id":"{{id}}","notice":"{{event.kind}}","data":"{{event}}"},"header_templates":{"x-selected-user":"{{event.user}}"}}
        }}}}),
    );
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let tenant = fixture.api(TENANT);
    let mut socket = open(request(fixture.port, Some(TENANT))).await;
    assert_eq!(
        rpc(
            &mut socket,
            json!({"type":"request","id":"clients","method":"GET","path":"/api/clients"})
        )
        .await["body"]["config"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(tenant.get("/api/realtime/subscriptions").await.status, 404);
    let subscription = json!({"enabled":true,"selector":{"event_names":["client.updated"],"clients":["features"]},
        "content":{"body_template":{"message":"changed {{event.client}}","sequence":"{{event.sequence}}","resource":"{{event}}"},"header_templates":{"x-event":"{{event.kind}}"}}});
    assert_eq!(rpc(&mut socket,json!({"type":"request","id":"create","method":"PUT","path":"/api/realtime/subscriptions/own","body":subscription})).await["status"],200);
    assert_eq!(rpc(&mut socket,json!({"type":"request","id":"foreign-sub","method":"PUT","path":"/api/realtime/subscriptions/foreign","body":{"selector":{"clients":["other"]}}})).await["status"],403);
    let mut other = root.get("/api/clients/other").await.body["config"].clone();
    other["remark"] = json!("foreign-event-value");
    assert_eq!(
        root.send(
            "PUT",
            "/api/clients/other",
            Some(other),
            0,
            "foreign-update"
        )
        .await
        .status,
        200
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let history = root.get("/api/node/changes").await;
            if history.body["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["client"] == "other")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let history = tenant.get("/api/node/changes").await;
    assert!(!history.raw.contains("other"));
    assert!(!history.raw.contains("foreign-event-value"));
    // Only the global revision notice is visible for another tenant's update.
    while let Ok(Some(Ok(item))) =
        tokio::time::timeout(Duration::from_millis(100), socket.next()).await
    {
        if let Message::Text(text) = item {
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_ne!(value["type"], "callback");
            assert!(!text.contains("other"));
        }
    }
    let mut own = root.get("/api/clients/features").await.body["config"].clone();
    own["remark"] = json!("own-update");
    assert_eq!(
        root.send("PUT", "/api/clients/features", Some(own), 1, "own-update")
            .await
            .status,
        200
    );
    let callback = message(&mut socket, "callback").await;
    assert_eq!(callback["subscription"], "own");
    assert_eq!(callback["body"]["message"], "changed features");
    assert!(callback["body"]["sequence"].is_number());
    assert_eq!(callback["body"]["resource"]["client"], "features");
    assert_eq!(callback["headers"]["x-event"], "client.updated");
    let (headers, body) = tokio::time::timeout(Duration::from_secs(5), receiver.records.recv())
        .await
        .unwrap()
        .unwrap();
    verify_signature(&headers, &body);
    assert_eq!(headers["x-selected-user"], "team-a");
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["notice"], "client.updated");
    assert_eq!(body["data"]["client"], "features");
    assert!(!body.to_string().contains("feature-secret"));
    let live = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = message(&mut socket, "event").await;
            if event["event"]["kind"] == "webhook.delivery_succeeded" {
                return event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(live["ephemeral"], true);
    assert!(
        !tenant
            .get("/api/node/changes")
            .await
            .raw
            .contains("webhook.delivery_succeeded")
    );
    for index in 1..32 {
        assert_eq!(
            rpc(
                &mut socket,
                json!({"type":"request","id":format!("sub-{index}"),"method":"PUT",
            "path":format!("/api/realtime/subscriptions/sub-{index}"),"body":{}})
            )
            .await["status"],
            200
        );
    }
    assert_eq!(rpc(&mut socket,json!({"type":"request","id":"limit","method":"PUT","path":"/api/realtime/subscriptions/overflow","body":{}})).await["status"],429);
    socket.close(None).await.unwrap();
    let mut socket = open(request(fixture.port, Some(TENANT))).await;
    assert_eq!(rpc(&mut socket,json!({"type":"request","id":"empty","method":"GET","path":"/api/realtime/subscriptions"})).await["body"]["subscriptions"].as_array().unwrap().len(),0);
    assert_eq!(
        rpc(
            &mut socket,
            json!({"type":"request","id":"recursive","method":"GET","path":"/api/ws"})
        )
        .await["status"],
        400
    );
    stopped(&daemon).await;
}

#[tokio::test]
async fn incoming_ws_checks_browser_token_handshake_and_closes_rotated_credentials() {
    let mut fixture = Fixture::new().await;
    configure(&mut fixture, json!({}));
    let daemon = fixture.start().await;
    assert_eq!(
        code(
            tokio_tungstenite::connect_async(request(fixture.port, Some(REVERSE)))
                .await
                .unwrap_err()
        ),
        401
    );
    assert_eq!(
        code(
            tokio_tungstenite::connect_async(request(fixture.port, None))
                .await
                .unwrap_err()
        ),
        401
    );
    let mut invalid = request(fixture.port, Some(TENANT));
    invalid.headers_mut().insert(
        http::header::SEC_WEBSOCKET_KEY,
        http::HeaderValue::from_static("broken"),
    );
    assert_eq!(
        code(tokio_tungstenite::connect_async(invalid).await.unwrap_err()),
        400
    );
    let mut browser = request(fixture.port, None);
    browser.headers_mut().insert(
        http::header::SEC_WEBSOCKET_PROTOCOL,
        http::HeaderValue::from_str(&format!(
            "felucca-node-v1, felucca-token.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(TENANT)
        ))
        .unwrap(),
    );
    browser.headers_mut().insert(
        http::header::ORIGIN,
        http::HeaderValue::from_str(&format!("http://127.0.0.1:{}", fixture.port)).unwrap(),
    );
    let mut socket = open(browser).await;
    let root = fixture.api(&fixture.root_token);
    let mut platform = root.get("/api/platforms/team").await.body["config"].clone();
    platform["token"] = json!("rotated-token-new-authority");
    assert_eq!(
        root.send("PUT", "/api/platforms/team", Some(platform), 0, "rotate")
            .await
            .status,
        200
    );
    closed(&mut socket).await;
    assert_eq!(
        code(
            tokio_tungstenite::connect_async(request(fixture.port, Some(TENANT)))
                .await
                .unwrap_err()
        ),
        401
    );
    stopped(&daemon).await;
}

#[tokio::test]
async fn browser_cookie_ws_requires_origin_and_csrf_on_mutations_and_reauthorizes_password() {
    let mut fixture = Fixture::new().await;
    configure(&mut fixture, json!({}));
    let daemon = fixture.start().await;
    let anonymous = Api {
        port: fixture.port,
        headers: vec![],
    };
    let (cookie, csrf) = anonymous.login().await;
    let token = cookie
        .headers
        .iter()
        .find(|(k, _)| k == "Cookie")
        .unwrap()
        .1
        .clone();
    let mut handshake = request(fixture.port, None);
    handshake.headers_mut().insert(
        http::header::COOKIE,
        http::HeaderValue::from_str(&token).unwrap(),
    );
    assert_eq!(
        code(
            tokio_tungstenite::connect_async(handshake.clone())
                .await
                .unwrap_err()
        ),
        403
    );
    handshake.headers_mut().insert(
        http::header::ORIGIN,
        http::HeaderValue::from_static("https://foreign.invalid"),
    );
    assert_eq!(
        code(
            tokio_tungstenite::connect_async(handshake.clone())
                .await
                .unwrap_err()
        ),
        403
    );
    handshake.headers_mut().insert(
        http::header::ORIGIN,
        http::HeaderValue::from_str(&format!("http://127.0.0.1:{}", fixture.port)).unwrap(),
    );
    let mut socket = open(handshake).await;
    let root = fixture.api(&fixture.root_token);
    let mut client = root.get("/api/clients/features").await.body["config"].clone();
    client["remark"] = json!("cookie-ws-write");
    let mut write = json!({"type":"request","id":"no-csrf","method":"PUT","path":"/api/clients/features",
        "headers":{"if-match":"\"0\"","idempotency-key":"cookie-write"},"body":client});
    assert_eq!(rpc(&mut socket, write.clone()).await["status"], 403);
    write["id"] = json!("with-csrf");
    write["headers"]["x-felucca-csrf"] = json!(csrf);
    assert_eq!(rpc(&mut socket, write).await["status"], 200);
    assert_eq!(
        daemon.config().1.clients["features"].remark,
        "cookie-ws-write"
    );
    let mut account = root.get("/api/accounts").await.body["config"]["root"].clone();
    account["password"] = json!("new-password-invalidates-cookie");
    assert_eq!(
        root.send(
            "PUT",
            "/api/accounts/root",
            Some(account),
            1,
            "rotate-password"
        )
        .await
        .status,
        200
    );
    closed(&mut socket).await;
    stopped(&daemon).await;
}
