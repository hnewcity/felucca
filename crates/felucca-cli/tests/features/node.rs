use super::*;
use crate::support::admin::*;
use crate::support::platform::*;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use tokio_tungstenite::tungstenite::Message;
#[tokio::test]
async fn platform_authority_rotation_and_webhook_crud_preserve_tenant_boundaries() {
    let mut fixture = Fixture::new().await;
    configure(&mut fixture, json!({}));
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let full = fixture.api(FULL);
    let tenant = fixture.api(TENANT);
    assert_eq!(
        full.get("/api/clients").await.body["config"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        tenant.get("/api/clients").await.body["config"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(tenant.get("/api/clients/other").await.status, 404);
    assert_eq!(tenant.get("/api/platforms").await.status, 403);
    assert_eq!(
        fixture.api(REVERSE).get("/api/node/status").await.status,
        401
    );
    assert_eq!(
        fixture
            .api("disabled-platform-credential")
            .get("/api/node/status")
            .await
            .status,
        401
    );
    let status = tenant.get("/api/node/status").await;
    assert_eq!(status.status, 200, "{}", status.raw);
    assert_eq!(status.body["platforms"].as_array().unwrap().len(), 1);
    assert_eq!(
        tenant.get("/api/node/usage").await.body["clients"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!root.get("/api/config").await.raw.contains(FULL));
    let webhook = json!({"user":"team-a","delivery":{"url":"http://127.0.0.1:1/hook",
        "signing_key":SIGNING,"enabled":false}});
    let added = root
        .send(
            "PUT",
            "/api/webhooks/own",
            Some(webhook.clone()),
            0,
            "create-hook",
        )
        .await;
    assert_eq!(added.status, 200, "{}", added.raw);
    let mut foreign = webhook.clone();
    foreign["user"] = json!("team-b");
    assert_eq!(
        tenant
            .send(
                "PUT",
                "/api/webhooks/foreign",
                Some(foreign),
                1,
                "foreign-hook"
            )
            .await
            .status,
        403
    );
    let mut global = webhook;
    global["user"] = Value::Null;
    assert_eq!(
        tenant
            .send(
                "PUT",
                "/api/webhooks/global",
                Some(global),
                1,
                "global-hook"
            )
            .await
            .status,
        403
    );
    let stored = tenant.get("/api/webhooks/own").await;
    assert!(!stored.raw.contains(SIGNING));
    assert_eq!(
        tenant
            .send(
                "PUT",
                "/api/webhooks/own",
                Some(stored.body["config"].clone()),
                1,
                "operator-hook"
            )
            .await
            .status,
        403
    );
    assert_eq!(
        root.send(
            "PUT",
            "/api/webhooks/own",
            Some(stored.body["config"].clone()),
            1,
            "edit-hook"
        )
        .await
        .status,
        200
    );
    assert_eq!(
        daemon.config().1.node.unwrap().webhooks["own"]
            .delivery
            .signing_key
            .expose(),
        SIGNING
    );
    let mut platform = root.get("/api/platforms/team").await.body["config"].clone();
    platform["token"] = json!("replacement-platform-credential");
    let rotated = root
        .send(
            "PUT",
            "/api/platforms/team",
            Some(platform),
            2,
            "rotate-team",
        )
        .await;
    assert_eq!(rotated.status, 200, "{}", rotated.raw);
    assert_eq!(tenant.get("/api/clients").await.status, 401);
    assert_eq!(
        fixture
            .api("replacement-platform-credential")
            .get("/api/clients")
            .await
            .status,
        200
    );
    stopped(&daemon).await;
}
#[tokio::test]
async fn manual_replay_invalidates_an_older_inflight_failure_before_new_delivery_begins() {
    let mut receiver = receiver().await;
    receiver.block_next.store(true, Ordering::Release);
    let mut fixture = Fixture::new().await;
    configure(
        &mut fixture,
        json!({"webhook":{"events":{"user":"team-a","delivery":{
            "url":format!("http://127.0.0.1:{}/hook",receiver.port),"signing_key":SIGNING,
            "retry_max":0,"timeout":"10s","queue_max":2
        }}}}),
    );
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let (headers, _) = tokio::time::timeout(Duration::from_secs(10), receiver.records.recv())
        .await
        .unwrap()
        .unwrap();
    let id = headers["x-felucca-event-id"].to_str().unwrap();
    let replay = root
        .send(
            "POST",
            "/api/node/callbacks/replay",
            Some(json!({"ids":[id]})),
            0,
            "race-replay",
        )
        .await;
    assert_eq!(replay.status, 200, "{}", replay.raw);
    receiver.success.store(true, Ordering::Release);
    receiver.gate.add_permits(1);
    let (next, body) = tokio::time::timeout(Duration::from_secs(5), receiver.records.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next["x-felucca-event-id"], id);
    verify_signature(&next, &body);
    tokio::time::timeout(Duration::from_secs(5), async {
        while root.get("/api/node/callbacks").await.raw.contains(id) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    stopped(&daemon).await;
}
#[tokio::test]
async fn signed_callbacks_retry_persist_and_replay_once_across_restart() {
    let mut receiver = receiver().await;
    let mut fixture = Fixture::new().await;
    configure(
        &mut fixture,
        json!({"webhook":{"events":{"user":"team-b","delivery":{
            "url":format!("http://127.0.0.1:{}/hook",receiver.port),"signing_key":SIGNING,
            "retry_max":1,"retry_backoff":"100ms","timeout":"1s","queue_max":4
        }}}}),
    );
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let tenant = fixture.api(TENANT);
    let failed = failed_delivery(&root).await;
    assert_eq!(failed["attempts"], 2);
    assert_eq!(failed["last_status"], 503);
    let (first_headers, first_body) = receiver.records.recv().await.unwrap();
    let (second_headers, second_body) = receiver.records.recv().await.unwrap();
    verify_signature(&first_headers, &first_body);
    verify_signature(&second_headers, &second_body);
    assert_eq!(
        first_headers["x-felucca-event-id"],
        second_headers["x-felucca-event-id"]
    );
    assert_eq!(first_body, second_body);
    let ids = json!({"ids":[failed["id"]]});
    assert_eq!(
        tenant.get("/api/node/callbacks").await.body["queues"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        tenant
            .send(
                "POST",
                "/api/node/callbacks/replay",
                Some(ids.clone()),
                0,
                "foreign-replay"
            )
            .await
            .status,
        403
    );
    let history = root.get("/api/node/changes").await;
    let epoch = history.body["epoch"].clone();
    assert!(!history.raw.contains(SIGNING));
    assert_eq!(
        root.get("/api/node/changes?after=00000000-0000-0000-0000-000000000001:0")
            .await
            .status,
        410
    );
    stopped(&daemon).await;
    drop(daemon);
    let daemon = fixture.start().await;
    let queues = root.get("/api/node/callbacks").await;
    assert!(queues.raw.contains(failed["id"].as_str().unwrap()));
    assert_eq!(root.get("/api/node/changes").await.body["epoch"], epoch);
    receiver.success.store(true, Ordering::Release);
    let replay = root
        .send(
            "POST",
            "/api/node/callbacks/replay",
            Some(ids.clone()),
            0,
            "replay-failed",
        )
        .await;
    assert_eq!(replay.status, 200, "{}", replay.raw);
    assert_eq!(replay.body["affected"], 1);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !root
                .get("/api/node/callbacks")
                .await
                .raw
                .contains(failed["id"].as_str().unwrap())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let replay = root
        .send(
            "POST",
            "/api/node/callbacks/replay",
            Some(ids),
            0,
            "replay-failed",
        )
        .await;
    assert_eq!(replay.status, 200, "{}", replay.raw);
    assert_eq!(replay.body["replayed"], true);
    assert_eq!(
        root.send(
            "POST",
            "/api/node/callbacks/delete",
            Some(json!({"ids":[failed["id"]]})),
            0,
            "replay-failed"
        )
        .await
        .status,
        400
    );
    stopped(&daemon).await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(fixture.files.root.join("node.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[tokio::test]
async fn reverse_rpc_reuses_rbac_cas_idempotency_and_closes_on_disable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut fixture = Fixture::new().await;
    configure(
        &mut fixture,
        json!({"platform":{"reverse":{"token":REVERSE,"scope":"account",
        "user":"team-a","mode":"reverse","reverse_url":format!("ws://{}/control",listener.local_addr().unwrap()),"heartbeat":"1s"}},
        "usage_interval":"1s"}),
    );
    let daemon = fixture.start().await;
    let mut socket = accept_reverse(&listener).await;
    assert_eq!(message(&mut socket, "hello").await["node"], "edge-test");
    assert_eq!(fixture.api(REVERSE).get("/api/clients").await.status, 401);
    let clients = rpc(
        &mut socket,
        json!({"type":"request","id":"read","method":"GET","path":"/api/clients"}),
    )
    .await;
    assert_eq!(clients["status"], 200, "{clients}");
    assert_eq!(clients["body"]["config"].as_array().unwrap().len(), 1);
    assert_eq!(
        rpc(
            &mut socket,
            json!({"type":"request","id":"foreign","method":"GET","path":"/api/clients/other"})
        )
        .await["status"],
        404
    );
    assert_eq!(
        rpc(
            &mut socket,
            json!({"type":"request","id":"spoof","method":"GET","path":"/api/clients",
        "headers":{"authorization":"Bearer forged-authority"}})
        )
        .await["status"],
        400
    );
    let root = fixture.api(&fixture.root_token);
    let mut client = root.get("/api/clients/features").await.body["config"].clone();
    client["remark"] = json!("written through reverse RPC");
    let mut write = json!({"type":"request","id":"write","method":"PUT","path":"/api/clients/features",
        "headers":{"if-match":"\"0\"","idempotency-key":"reverse-write"},"body":client});
    let written = rpc(&mut socket, write.clone()).await;
    assert_eq!(written["status"], 200, "{written}");
    write["id"] = json!("replay");
    assert_eq!(rpc(&mut socket, write).await["body"]["replayed"], true);
    assert_eq!(
        daemon.config().1.clients["features"].remark,
        "written through reverse RPC"
    );
    let usage = message(&mut socket, "usage").await;
    assert_eq!(usage["snapshot"]["clients"].as_array().unwrap().len(), 1);
    let mut platform = root.get("/api/platforms/reverse").await.body["config"].clone();
    platform["enabled"] = json!(false);
    assert_eq!(
        root.send(
            "PUT",
            "/api/platforms/reverse",
            Some(platform),
            1,
            "disable-reverse"
        )
        .await
        .status,
        200
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(item)) = socket.next().await {
            if matches!(item, Message::Close(_)) {
                break;
            }
        }
    })
    .await
    .expect("disabled platforms lose existing reverse sessions");
    stopped(&daemon).await;
}
#[tokio::test]
#[allow(clippy::result_large_err)] // tungstenite's handshake callback fixes the error type.
async fn reverse_wss_verifies_pin_before_exposing_the_platform_bearer_token() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cert = identity("127.0.0.1").await;
    let acceptor = cert.identity.acceptor().unwrap();
    let mut fixture = Fixture::new().await;
    configure(
        &mut fixture,
        json!({"platform":{"reverse":{"token":REVERSE,"scope":"full",
        "mode":"reverse","reverse_url":format!("wss://{}/control",listener.local_addr().unwrap()),
        "trust":{"mode":"pinned","fingerprint":cert.identity.fingerprint()}}}}),
    );
    let daemon = fixture.start().await;
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let tls = acceptor.accept(tcp).await.unwrap();
    let mut socket = tokio_tungstenite::accept_hdr_async(
        tls,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
         mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            assert_eq!(
                request.headers()["authorization"],
                format!("Bearer {REVERSE}")
            );
            response.headers_mut().insert(
                http::header::SEC_WEBSOCKET_PROTOCOL,
                http::HeaderValue::from_static("felucca-node-v1"),
            );
            Ok(response)
        },
    )
    .await
    .unwrap();
    message(&mut socket, "hello").await;
    assert_eq!(
        rpc(
            &mut socket,
            json!({"type":"request","id":"status","method":"GET","path":"/api/node/status"})
        )
        .await["status"],
        200
    );
    let root = fixture.api(&fixture.root_token);
    let mut platform = root.get("/api/platforms/reverse").await.body["config"].clone();
    platform["trust"] = json!({"mode":"pinned","fingerprint":"11".repeat(32)});
    assert_eq!(
        root.send(
            "PUT",
            "/api/platforms/reverse",
            Some(platform),
            0,
            "wrong-pin"
        )
        .await
        .status,
        200
    );
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(
        acceptor.accept(tcp).await.is_err(),
        "incorrect pin must abort TLS before HTTP authorization is sent"
    );
    stopped(&daemon).await;
}
#[tokio::test]
async fn reverse_heartbeat_deadline_does_not_reset_while_ping_is_outstanding() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut fixture = Fixture::new().await;
    configure(
        &mut fixture,
        json!({"platform":{"reverse":{"token":REVERSE,"scope":"full",
        "mode":"reverse","reverse_url":format!("ws://{}/control",listener.local_addr().unwrap()),"heartbeat":"1s"}}}),
    );
    let daemon = fixture.start().await;
    let mut socket = accept_reverse(&listener).await;
    message(&mut socket, "hello").await;
    let first = message(&mut socket, "ping").await;
    assert_eq!(first["nonce"], 1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(item)) = socket.next().await {
            if let Message::Text(text) = item {
                assert_ne!(
                    serde_json::from_str::<Value>(&text).unwrap()["type"],
                    "ping"
                );
            }
        }
    })
    .await
    .expect("missed pong must close the connection");
    let mut next = accept_reverse(&listener).await;
    assert_eq!(message(&mut next, "hello").await["node"], "edge-test");
    stopped(&daemon).await;
}
#[tokio::test]
async fn node_traffic_import_is_atomic_scoped_durable_and_kick_prevents_reconnect() {
    let mut receiver = receiver().await;
    receiver.success.store(true, Ordering::Release);
    let mut fixture = Fixture::new().await;
    configure(
        &mut fixture,
        json!({"traffic_report_step_bytes":100,"webhook":{"usage":{"user":"team-a","delivery":{
            "url":format!("http://127.0.0.1:{}/hook",receiver.port),"signing_key":SIGNING
        }}}}),
    );
    fixture.config.state = Some(felucca_core::config::server::StateConfig {
        path: fixture.files.root.join("traffic.json"),
        save_interval: Duration::from_secs(1),
    });
    fixture
        .config
        .clients
        .get_mut("features")
        .unwrap()
        .quota
        .flow_limit = Some(100);
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let tenant = fixture.api(TENANT);
    let viewer = fixture.api(&fixture.viewer_token);
    // Wait for startup delivery so both live and durable subscribers are ready.
    tokio::time::timeout(Duration::from_secs(10), receiver.records.recv())
        .await
        .unwrap()
        .unwrap();
    let report = json!({"items":[{"client":"features","inlet_bytes":30,"export_bytes":40},
        {"client":"features","inlet_bytes":10,"export_bytes":20}]});
    assert_eq!(
        viewer
            .send(
                "POST",
                "/api/node/traffic",
                Some(report.clone()),
                0,
                "viewer-traffic"
            )
            .await
            .status,
        403
    );
    let mut foreign = report.clone();
    foreign["items"][1]["client"] = json!("other");
    assert_eq!(
        tenant
            .send(
                "POST",
                "/api/node/traffic",
                Some(foreign),
                0,
                "foreign-traffic"
            )
            .await
            .status,
        403
    );
    assert!(
        daemon
            .server()
            .registry()
            .store
            .stats(&felucca_server::resources::ResourceKey::Client {
                name: "features".into()
            })
            .is_none()
    );
    let response = tenant
        .send(
            "POST",
            "/api/node/traffic",
            Some(report.clone()),
            0,
            "report-1",
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.raw);
    assert_eq!(response.body["durable"], true);
    let (headers, body) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (headers, body) = receiver.records.recv().await.unwrap();
            if serde_json::from_slice::<Value>(&body).unwrap()["event"]["kind"]
                == "client.traffic.reported"
            {
                return (headers, body);
            }
        }
    })
    .await
    .unwrap();
    verify_signature(&headers, &body);
    let report_event: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(report_event["event"]["client"], "features");
    assert_eq!(report_event["ephemeral"], true);
    assert!(
        !root
            .get("/api/node/changes")
            .await
            .raw
            .contains("client.traffic.reported")
    );
    for resource in [
        felucca_server::resources::ResourceKey::Client {
            name: "features".into(),
        },
        felucca_server::resources::ResourceKey::User {
            name: "team-a".into(),
        },
    ] {
        assert_eq!(
            daemon.server().registry().store.stats(&resource).unwrap(),
            felucca_core::traffic::TrafficStats {
                inlet: 40,
                export: 60
            }
        );
    }
    let replay = tenant
        .send(
            "POST",
            "/api/node/traffic",
            Some(report.clone()),
            0,
            "report-1",
        )
        .await;
    assert_eq!(replay.body["replayed"], true);
    stopped(&daemon).await;
    drop(daemon);
    let daemon = fixture.start().await;
    assert_eq!(
        tenant
            .send("POST", "/api/node/traffic", Some(report), 0, "report-1")
            .await
            .body["replayed"],
        true
    );
    let meter = daemon.server().registry().store.meter(
        felucca_server::resources::ResourceKey::Client {
            name: "features".into(),
        },
        felucca_core::traffic::ResourceLimits {
            quota: fixture.config.clients["features"].quota,
            ..Default::default()
        },
    );
    assert!(
        felucca_server::resources::MeterGroup(vec![meter])
            .acquire()
            .is_none()
    );
    assert_eq!(
        tenant
            .send("POST", "/api/node/sync", Some(json!({})), 0, "tenant-sync")
            .await
            .status,
        403
    );
    let kick = json!({"ids":["features"]});
    let response = tenant
        .send(
            "POST",
            "/api/node/kick",
            Some(kick.clone()),
            0,
            "kick-client",
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.raw);
    assert!(!daemon.config().1.clients["features"].enabled);
    assert_eq!(
        tenant
            .send("POST", "/api/node/kick", Some(kick), 0, "kick-client")
            .await
            .body["replayed"],
        true
    );
    let synced = root
        .send("POST", "/api/node/sync", Some(json!({})), 1, "sync-node")
        .await;
    assert_eq!(synced.status, 200, "{}", synced.raw);
    assert_eq!(synced.body["revision"], 2);
    assert_eq!(
        root.send("POST", "/api/node/sync", Some(json!({})), 1, "sync-node")
            .await
            .body["replayed"],
        true
    );
    stopped(&daemon).await;
    drop(daemon);
    let daemon = fixture.start().await;
    assert!(!daemon.config().1.clients["features"].enabled);
    assert_eq!(daemon.config().0, 2);
    stopped(&daemon).await;
}
#[tokio::test]
async fn callback_queues_evict_oldest_and_allow_pagination_without_cross_tenant_entries() {
    let mut fixture = Fixture::new().await;
    configure(
        &mut fixture,
        json!({"webhook":{"events":{"user":"team-a","delivery":{
            "url":"http://127.0.0.1:1/hook","signing_key":SIGNING,"queue_max":2,"retry_max":0,"timeout":"100ms","selector":{"event_names":["node.started","config.changed"]}
        }}}}),
    );
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let first = failed_delivery(&root).await;
    for revision in 0..3 {
        let mut client = root.get("/api/clients/features").await.body["config"].clone();
        client["remark"] = json!(format!("queue-event-{revision}"));
        assert_eq!(
            root.send(
                "PUT",
                "/api/clients/features",
                Some(client),
                revision,
                &format!("edit-{revision}")
            )
            .await
            .status,
            200
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let changes = root.get("/api/node/changes").await;
                if changes.body["events"].as_array().unwrap().last().unwrap()["revision"]
                    == revision + 1
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    let page = root
        .get("/api/node/callbacks?target=webhook%2Fevents&limit=1")
        .await;
    assert_eq!(page.status, 200, "{}", page.raw);
    assert_eq!(page.body["total"], 2);
    assert_eq!(page.body["next_offset"], 1);
    assert_eq!(page.body["queues"][0]["counters"]["dropped"], 2);
    let next = root.get("/api/node/callbacks?offset=1&limit=1").await;
    assert_eq!(next.body["next_offset"], Value::Null);
    assert_ne!(
        page.body["queues"][0]["items"][0]["id"],
        next.body["queues"][0]["items"][0]["id"]
    );
    assert!(!page.raw.contains(first["id"].as_str().unwrap()));
    assert_eq!(root.get("/api/node/callbacks?limit=513").await.status, 400);
    assert_eq!(
        root.get("/api/node/callbacks?limit=1&limit=1").await.status,
        400
    );
    stopped(&daemon).await;
}
