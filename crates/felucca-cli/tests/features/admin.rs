use super::*;
use crate::support::admin::*;
use felucca_server::Daemon;
use serde_json::json;

#[tokio::test]
async fn connection_credentials_only_grant_downloads_for_the_enabled_device() {
    let fixture = Fixture::new().await;
    let daemon = fixture.start().await;
    let device = Api {
        port: fixture.port,
        headers: vec![(
            "Authorization".into(),
            format!(
                "Bearer {}",
                fixture.config.clients["features"].vkey.expose()
            ),
        )],
    };
    let anonymous = Api {
        port: fixture.port,
        headers: vec![],
    };
    let path = "/api/connect/features/unknown-target/sha256";
    assert_eq!(anonymous.get(path).await.status, 403);
    // Authenticated, but no such package; this credential cannot query config.
    assert_eq!(device.get(path).await.status, 404);
    assert_eq!(device.get("/api/config").await.status, 401);
    assert_eq!(
        device
            .get("/api/connect/other/unknown-target/binary")
            .await
            .status,
        403
    );
    let mut disabled = daemon.config().1;
    disabled.clients.get_mut("features").unwrap().enabled = false;
    daemon.apply(disabled, 0).await.unwrap();
    assert_eq!(device.get(path).await.status, 403);
    let script = anonymous.get("/connect.sh").await;
    assert_eq!(script.status, 200);
    assert!(script.raw.contains("felucca_connect"));
    assert!(!script.raw.contains("{{starting}}"));
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn capability_exports_tokens_certificates_and_logs_enforce_permissions() {
    let mut fixture = Fixture::new().await;
    let log = fixture.files.root.join("console.log");
    fixture.config.log.file = Some(log.clone());
    tokio::fs::write(
        &log,
        format!(
            "test {}\n",
            fixture.config.clients["features"].vkey.expose()
        ),
    )
    .await
    .unwrap();
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let operator = fixture.api(&fixture.operator_token);
    for path in ["/api/certificates", "/api/logs"] {
        assert_eq!(operator.get(path).await.status, 403);
    }
    assert_eq!(
        operator
            .send("POST", "/api/token", Some(json!({})), 0, "token")
            .await
            .status,
        403
    );
    let issued = root
        .send("POST", "/api/token", Some(json!({})), 0, "token")
        .await;
    assert_eq!(issued.status, 200, "{}", issued.raw);
    assert_eq!(
        issued.body["token_hash"],
        felucca_server::admin::credentials::token_hash(issued.body["token"].as_str().unwrap())
    );
    let snapshot = root.get("/api/config").await.body;
    let mut draft = snapshot["config"].clone();
    draft["log"]["level"] = json!("debug");
    let plan = root
        .send("POST", "/api/validate", Some(draft.clone()), 0, "plan")
        .await;
    assert_eq!(plan.body["restart_fields"], json!(["log"]));
    let exported = root
        .send(
            "POST",
            "/api/config/export",
            Some(draft.clone()),
            0,
            "export",
        )
        .await;
    assert_eq!(exported.status, 200, "{}", exported.raw);
    assert_eq!(exported.body["snapshot"]["revision"], 1);
    assert_eq!(
        exported.body["startup"]["client"]["features"]["vkey"],
        fixture.config.clients["features"].vkey.expose()
    );
    assert_eq!(
        daemon.config().0,
        0,
        "export must not apply a configuration"
    );
    assert_eq!(daemon.config().1.log.level, "info");
    assert_eq!(
        root.send(
            "POST",
            "/api/config/export",
            Some(draft),
            99,
            "stale-export"
        )
        .await
        .status,
        409
    );
    assert_eq!(
        operator
            .send(
                "POST",
                "/api/config/export",
                Some(snapshot["config"].clone()),
                0,
                "forbidden-export"
            )
            .await
            .status,
        403
    );
    let logs = root.get("/api/logs").await;
    assert!(logs.body["text"].as_str().unwrap().contains("<redacted>"));
    assert!(
        !logs
            .raw
            .contains(fixture.config.clients["features"].vkey.expose())
    );
    let certs = root.get("/api/certificates").await;
    assert!(
        certs.body["diagnostics"]["certificates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cert| cert["name"] == "bridge")
    );
    assert!(!certs.raw.contains("PRIVATE KEY"));
    let upload = json!({
        "cert":tokio::fs::read_to_string(&fixture.files.config.cert).await.unwrap(),
        "key":tokio::fs::read_to_string(&fixture.files.config.key).await.unwrap(),
    });
    assert_eq!(
        operator
            .send(
                "POST",
                "/api/certificates/upload",
                Some(upload.clone()),
                0,
                "upload"
            )
            .await
            .status,
        403
    );
    let uploaded = root
        .send(
            "POST",
            "/api/certificates/upload",
            Some(upload.clone()),
            0,
            "upload",
        )
        .await;
    assert_eq!(uploaded.status, 200, "{}", uploaded.raw);
    let replay = root
        .send(
            "POST",
            "/api/certificates/upload",
            Some(upload),
            0,
            "upload",
        )
        .await;
    assert_eq!(replay.body, uploaded.body);
    felucca_transport::Identity::from_pem_files(
        std::path::Path::new(uploaded.body["cert"].as_str().unwrap()),
        std::path::Path::new(uploaded.body["key"].as_str().unwrap()),
    )
    .unwrap();
    let (cookie, _) = root.login().await;
    let no_csrf = Api {
        port: cookie.port,
        headers: cookie
            .headers
            .into_iter()
            .filter(|(key, _)| key != "X-Felucca-CSRF")
            .collect(),
    };
    assert_eq!(
        no_csrf
            .send("POST", "/api/token", Some(json!({})), 0, "csrf")
            .await
            .status,
        403
    );
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn backend_diagnostics_read_real_agent_samples_without_cross_tenant_access() {
    let mut fixture = Fixture::new().await;
    let bridge = free_port().await;
    fixture.config.bridge.bind = format!("127.0.0.1:{bridge}");
    let daemon = fixture.start().await;
    let backend = echo().await;
    let public = free_port().await;
    let mut config = agent(&format!(
        "[[tunnel]]\nid=17\nmode='tcp'\nport={public}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{backend}'}}]\n[tunnel.health]\ntype='tcp'\ninterval='50ms'\ntimeout='20ms'"
    ));
    config.server = format!("127.0.0.1:{bridge}");
    config.trust = TrustPolicy::Pinned {
        fingerprint: daemon.server().fingerprint(),
    };
    let agent = Arc::new(Agent::new(config).unwrap());
    let task = tokio::spawn(agent.clone().run());
    let operator = fixture.api(&fixture.operator_token);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let result = operator.get("/api/diagnostics/features/17").await;
            assert_eq!(result.status, 200, "{}", result.raw);
            if result.body["diagnostics"]["report"]["backends"][0]["last_success"] == true {
                assert_eq!(
                    result.body["diagnostics"]["report"]["backends"][0]["healthy"],
                    true
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(operator.get("/api/diagnostics/other/17").await.status, 404);
    let diagnostics = operator.get("/api/diagnostics").await;
    assert!(!diagnostics.raw.contains("other"));
    assert_eq!(diagnostics.body["diagnostics"]["tunnels"][0]["id"], 17);
    roundtrip(TcpStream::connect(("127.0.0.1", public)).await.unwrap()).await;
    agent.shutdown();
    task.await.unwrap();
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn host_monitor_samples_real_resources_and_survives_configuration_reload() {
    let fixture = Fixture::new().await;
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let anonymous = Api {
        port: fixture.port,
        headers: vec![],
    };
    assert_eq!(anonymous.get("/api/monitor").await.status, 401);
    for token in [&fixture.operator_token, &fixture.viewer_token] {
        assert_eq!(fixture.api(token).get("/api/monitor").await.status, 403);
    }
    assert_eq!(root.get("/api/monitor?window=999999").await.status, 400);
    let first = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let response = root.get("/api/monitor?window=300").await;
            assert_eq!(response.status, 200, "{}", response.raw);
            if response.body["samples"]
                .as_array()
                .is_some_and(|samples| samples.len() >= 2)
            {
                break response.body;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(first["sample_interval_ms"], 5000);
    assert_eq!(first["retention_seconds"], 3600);
    assert_eq!(first["sampling_failed"], false);
    assert!(first["host"]["cpu_cores"].as_u64().unwrap() > 0);
    let samples = first["samples"].as_array().unwrap();
    assert!(
        samples[0]["cpu_percent"].is_null(),
        "CPU needs an initial baseline"
    );
    let latest = samples.last().unwrap();
    assert!(
        latest["timestamp_ms"].as_u64().unwrap() > samples[0]["timestamp_ms"].as_u64().unwrap()
    );
    assert!(latest["memory_total_bytes"].as_u64().unwrap() > 0);
    assert!(
        latest["memory_used_bytes"].as_u64().unwrap()
            <= latest["memory_total_bytes"].as_u64().unwrap()
    );
    for key in ["cpu_percent", "memory_percent"] {
        assert!((0.0..=100.0).contains(&latest[key].as_f64().unwrap()));
    }
    for key in ["network_rx_bytes_per_sec", "network_tx_bytes_per_sec"] {
        if let Some(value) = latest[key].as_f64() {
            assert!(value.is_finite() && value >= 0.0);
        }
    }
    let resource = root.get("/api/clients/features").await;
    let mut config = resource.body["config"].clone();
    config["remark"] = json!("host monitor reload");
    let changed = root
        .send(
            "PUT",
            "/api/clients/features",
            Some(config),
            resource.body["revision"].as_u64().unwrap(),
            "monitor-reload",
        )
        .await;
    assert_eq!(changed.status, 200, "{}", changed.raw);
    let after = root.get("/api/monitor?window=3600").await;
    assert_eq!(after.body["started_at_ms"], first["started_at_ms"]);
    assert_eq!(after.body["samples"][0], samples[0]);
    tokio::time::timeout(Duration::from_secs(3), daemon.shutdown())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn share_management_preserves_secrets_and_cannot_grant_another_tenants_routes() {
    let fixture = Fixture::new().await;
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let operator = fixture.api(&fixture.operator_token);
    let viewer = fixture.api(&fixture.viewer_token);
    let secret = "admin-shared-access-secret";
    let grant = json!({"client":"features","tunnel":1,"secret":secret,"allow_clients":["other"]});
    let response = operator
        .send(
            "PUT",
            "/api/shares/office",
            Some(grant.clone()),
            0,
            "share-create",
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.raw);
    let stored = operator.get("/api/shares/office").await;
    assert_eq!(stored.status, 200);
    assert!(!stored.raw.contains(secret));
    assert!(!root.get("/api/config").await.raw.contains(secret));
    assert!(root.get("/api/config?secrets=1").await.raw.contains(secret));
    let mut edited = stored.body["config"].clone();
    edited["allow_clients"] = json!(["features"]);
    assert_eq!(
        operator
            .send("PUT", "/api/shares/office", Some(edited), 1, "share-edit")
            .await
            .status,
        200
    );
    assert_eq!(daemon.config().1.shares["office"].secret.expose(), secret);
    let mut foreign = grant;
    foreign["client"] = json!("other");
    assert_eq!(
        operator
            .send(
                "PUT",
                "/api/shares/foreign",
                Some(foreign.clone()),
                2,
                "foreign-owner"
            )
            .await
            .status,
        403
    );
    assert_eq!(
        viewer
            .send("DELETE", "/api/shares/office", None, 2, "readonly")
            .await
            .status,
        403
    );
    assert_eq!(
        root.send("PUT", "/api/shares/foreign", Some(foreign), 2, "root-owner")
            .await
            .status,
        200
    );
    assert_eq!(operator.get("/api/shares/foreign").await.status, 404);
    assert_eq!(
        operator.get("/api/shares").await.body["config"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        operator
            .send("DELETE", "/api/shares/foreign", None, 3, "foreign-delete")
            .await
            .status,
        404
    );
    assert_eq!(
        operator
            .send("DELETE", "/api/shares/office", None, 3, "share-delete")
            .await
            .status,
        200
    );
    daemon.shutdown().await.unwrap();
}
#[tokio::test]
async fn prometheus_metrics_require_auth_and_report_only_the_current_tenants_real_traffic() {
    let mut fixture = Fixture::new().await;
    let bridge = free_port().await;
    fixture.config.bridge.bind = format!("127.0.0.1:{bridge}");
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let operator = fixture.api(&fixture.operator_token);
    let anonymous = Api {
        port: fixture.port,
        headers: vec![],
    };
    assert_eq!(anonymous.get("/metrics").await.status, 401);
    let backend = echo().await;
    let public = free_port().await;
    let mut config = agent(&format!(
        "[[tunnel]]\nid=17\nmode='tcp'\nport={public}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{backend}'}}]"
    ));
    config.server = format!("127.0.0.1:{bridge}");
    config.trust = TrustPolicy::Pinned {
        fingerprint: daemon.server().fingerprint(),
    };
    let agent = Arc::new(Agent::new(config).unwrap());
    let task = tokio::spawn(agent.clone().run());
    tokio::time::timeout(Duration::from_secs(10), async {
        while !agent.is_ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    roundtrip(TcpStream::connect(("127.0.0.1", public)).await.unwrap()).await;
    let response = operator.get("/metrics").await;
    assert_eq!(response.status, 200, "{}", response.raw);
    assert!(response.headers["content-type"].starts_with("text/plain; version=0.0.4"));
    assert!(!response.raw.contains("client=\"other\""));
    assert!(!response.raw.contains("team-b"));
    assert!(!response.raw.contains("feature-secret"));
    assert!(response.raw.contains(
        "felucca_traffic_bytes_total{client=\"features\",user=\"team-a\",direction=\"inlet\"} 5"
    ));
    assert!(response.raw.contains("felucca_tunnel_bytes_total{client=\"features\",user=\"team-a\",tunnel=\"17\",direction=\"export\"} 5"));
    assert!(
        root.get("/api/metrics")
            .await
            .raw
            .contains("client=\"other\"")
    );
    let mut next = daemon.config().1;
    next.log.file = Some(fixture.files.root.join("ignored.log"));
    assert!(
        daemon
            .apply(next, 0)
            .await
            .unwrap_err()
            .to_string()
            .contains("restart")
    );
    assert!(daemon.config().1.log.file.is_none());
    agent.shutdown();
    task.await.unwrap();
    daemon.shutdown().await.unwrap();
}
#[tokio::test]
async fn admin_rbac_redaction_csrf_and_atomic_batches_enforce_tenant_boundaries() {
    let fixture = Fixture::new().await;
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    let operator = fixture.api(&fixture.operator_token);
    let viewer = fixture.api(&fixture.viewer_token);
    let anonymous = Api {
        port: fixture.port,
        headers: vec![],
    };
    assert_eq!(anonymous.get("/api/clients").await.status, 401);
    let clients = operator.get("/api/clients").await;
    assert_eq!(clients.status, 200);
    assert_eq!(clients.body["config"].as_array().unwrap().len(), 1);
    assert!(!clients.raw.contains("feature-secret"));
    assert!(!clients.raw.contains("different-tenant-secret"));
    assert_eq!(operator.get("/api/clients/other").await.status, 404);
    assert_eq!(operator.get("/api/config?secrets=1").await.status, 403);
    assert_eq!(operator.get("/api/accounts").await.status, 403);
    assert!(root.get("/api/config").await.raw.contains("<redacted>"));
    assert!(!root.get("/api/config").await.raw.contains(PASSWORD));
    assert!(
        root.get("/api/config?secrets=1")
            .await
            .raw
            .contains("feature-secret")
    );
    let mut client = operator.get("/api/clients/features").await.body["config"].clone();
    client["remark"] = json!("updated by owner");
    assert_eq!(
        viewer
            .send(
                "PUT",
                "/api/clients/features",
                Some(client.clone()),
                0,
                "viewer-write"
            )
            .await
            .status,
        403
    );
    let mut moved = client.clone();
    moved["user"] = json!("team-b");
    assert_eq!(
        operator
            .send(
                "PUT",
                "/api/clients/features",
                Some(moved),
                0,
                "move-tenant"
            )
            .await
            .status,
        403
    );
    let response = operator
        .send(
            "POST",
            "/api/batch",
            Some(json!({"operations":[
                {"method":"PUT","path":"/api/clients/features","value":client},
                {"method":"DELETE","path":"/api/clients/other"}
            ]})),
            0,
            "invalid-batch",
        )
        .await;
    assert_eq!(response.status, 404, "{}", response.raw);
    assert_eq!(daemon.config().0, 0);
    assert_eq!(daemon.config().1.clients["features"].remark, "");
    let (cookie, _) = anonymous.login().await;
    let no_csrf = Api {
        port: fixture.port,
        headers: cookie
            .headers
            .iter()
            .filter(|(k, _)| k == "Cookie")
            .cloned()
            .collect(),
    };
    assert_eq!(
        no_csrf
            .send(
                "PUT",
                "/api/clients/features",
                Some(client.clone()),
                0,
                "csrf"
            )
            .await
            .status,
        403
    );
    let response = cookie
        .send(
            "PUT",
            "/api/clients/features",
            Some(client),
            0,
            "owner-write",
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.raw);
    assert_eq!(
        daemon.config().1.clients["features"].vkey.expose(),
        "feature-secret"
    );
    assert_eq!(
        daemon.config().1.clients["features"].remark,
        "updated by owner"
    );
    assert_eq!(
        operator
            .send(
                "PUT",
                "/api/locals/unsafe",
                Some(json!({"tunnel":[]})),
                1,
                "local"
            )
            .await
            .status,
        403
    );
    let mut duplicate = root.clone();
    duplicate.headers.push((
        "Authorization".into(),
        format!("Bearer {}", fixture.root_token),
    ));
    assert_eq!(duplicate.get("/api/session").await.status, 401);
    daemon.shutdown().await.unwrap();
}
#[tokio::test]
async fn admin_config_edits_reach_agent_and_replay_survives_restart() {
    let mut fixture = Fixture::new().await;
    let bridge = free_port().await;
    fixture.config.bridge.bind = format!("127.0.0.1:{bridge}");
    fixture.config.bridge.tls = Some(fixture.files.config.clone());
    let daemon = fixture.start().await;
    let api = fixture.api(&fixture.operator_token);
    let mut config = agent("");
    config.remote.enabled = true;
    config.server = format!("127.0.0.1:{bridge}");
    config.trust = TrustPolicy::Pinned {
        fingerprint: daemon.server().fingerprint(),
    };
    let agent = Arc::new(Agent::new(config).unwrap());
    let task = tokio::spawn(agent.clone().run());
    let public = free_port().await;
    let backend = web_backend("managed-through-api").await;
    let desired = json!({"tunnel":[{"id":1,"mode":"tcp","port":public,"bind":"127.0.0.1","backend":[{"addr":format!("127.0.0.1:{backend}")}]}]});
    let response = api
        .send(
            "PUT",
            "/api/managed/features",
            Some(desired.clone()),
            0,
            "create-managed",
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.raw);
    assert_eq!(response.body["revision"], 1);
    assert_eq!(response.body["durable"], true);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if TcpStream::connect(("127.0.0.1", public)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        http_request(
            public,
            "GET / HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
        )
        .await
        .contains("managed-through-api")
    );
    let client_id = daemon.server().registry().get("features").unwrap().id;
    let replay = api
        .send(
            "PUT",
            "/api/managed/features",
            Some(desired.clone()),
            0,
            "create-managed",
        )
        .await;
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body["replayed"], true);
    assert_eq!(
        daemon.server().registry().get("features").unwrap().id,
        client_id,
        "replay must not restart the data plane"
    );
    assert_eq!(
        api.send(
            "PUT",
            "/api/managed/features",
            Some(json!({"tunnel":[]})),
            0,
            "create-managed"
        )
        .await
        .status,
        400
    );
    assert_eq!(
        api.send(
            "PUT",
            "/api/managed/features",
            Some(desired.clone()),
            0,
            "stale"
        )
        .await
        .status,
        409
    );
    agent.shutdown();
    task.await.unwrap();
    daemon.shutdown().await.unwrap();
    drop(daemon);
    let daemon = fixture.start().await;
    let replay = api
        .send(
            "PUT",
            "/api/managed/features",
            Some(desired),
            0,
            "create-managed",
        )
        .await;
    assert_eq!(replay.body["replayed"], true, "{}", replay.raw);
    assert_eq!(daemon.config().0, 1);
    assert_eq!(
        api.send(
            "DELETE",
            "/api/managed/features/tunnels/1",
            None,
            1,
            "delete-tunnel"
        )
        .await
        .status,
        200
    );
    let replay = api
        .send(
            "DELETE",
            "/api/managed/features/tunnels/1",
            None,
            1,
            "delete-tunnel",
        )
        .await;
    assert_eq!(replay.status, 200, "{}", replay.raw);
    assert_eq!(replay.body["replayed"], true);
    assert!(daemon.config().1.managed["features"].tunnels.is_empty());
    daemon.shutdown().await.unwrap();
}
#[tokio::test]
async fn password_changes_invalidate_existing_cookie_and_sse_sessions() {
    let fixture = Fixture::new().await;
    let daemon = fixture.start().await;
    let anonymous = Api {
        port: fixture.port,
        headers: vec![],
    };
    let (cookie, _) = anonymous.login().await;
    assert_eq!(cookie.get("/api/session").await.status, 200);
    let mut events = TcpStream::connect(("127.0.0.1", fixture.port))
        .await
        .unwrap();
    events.write_all(format!("GET /api/events HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: {}\r\nConnection: close\r\n\r\n",fixture.port,cookie.headers[0].1).as_bytes()).await.unwrap();
    assert!(head(&mut events).await.starts_with("HTTP/1.1 200"));
    let root = fixture.api(&fixture.root_token);
    let mut account = root.get("/api/accounts").await.body["config"]["root"].clone();
    account["password"] = json!("the-new-management-password");
    let response = root
        .send(
            "PUT",
            "/api/accounts/root",
            Some(account),
            0,
            "password-change",
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.raw);
    assert_eq!(cookie.get("/api/session").await.status, 401);
    let mut final_events = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        events.read_to_end(&mut final_events),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        anonymous
            .send(
                "POST",
                "/api/login",
                Some(json!({"account":"root","password":PASSWORD})),
                0,
                "login"
            )
            .await
            .status,
        401
    );
    assert_eq!(
        anonymous
            .send(
                "POST",
                "/api/login",
                Some(json!({"account":"root","password":"the-new-management-password"})),
                0,
                "login"
            )
            .await
            .status,
        200
    );
    daemon.shutdown().await.unwrap();
}
#[tokio::test]
async fn admin_host_origin_and_startup_failures_do_not_publish_or_commit_invalid_state() {
    let mut fixture = Fixture::new().await;
    let listener = TcpListener::bind(("127.0.0.1", fixture.port))
        .await
        .unwrap();
    assert!(Daemon::start(fixture.config.clone(), None).await.is_err());
    assert!(!fixture.config.admin.as_ref().unwrap().state_path.exists());
    drop(listener);
    let daemon = fixture.start().await;
    for (host, origin) in [
        (
            format!("attacker.test:{}", fixture.port),
            format!("http://attacker.test:{}", fixture.port),
        ),
        (
            format!("127.0.0.1:{}", fixture.port),
            "https://attacker.test".into(),
        ),
    ] {
        let mut io = TcpStream::connect(("127.0.0.1", fixture.port))
            .await
            .unwrap();
        io.write_all(format!("GET /api/session HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",fixture.root_token).as_bytes()).await.unwrap();
        assert!(head(&mut io).await.starts_with("HTTP/1.1 403"));
    }
    let root = fixture.api(&fixture.root_token);
    let saved = std::fs::read(fixture.config.admin.as_ref().unwrap().state_path.clone()).unwrap();
    let mut config = daemon.config().1;
    config.admin.as_mut().unwrap().bind = format!("127.0.0.1:{}", free_port().await);
    assert_eq!(
        root.send(
            "PUT",
            "/api/config",
            Some(serde_json::to_value(config).unwrap()),
            0,
            "change-listener"
        )
        .await
        .status,
        400
    );
    assert_eq!(
        std::fs::read(fixture.config.admin.as_ref().unwrap().state_path.clone()).unwrap(),
        saved
    );
    let page = root.get("/").await;
    assert_eq!(page.status, 200);
    assert!(page.headers["content-security-policy"].contains("frame-ancestors 'none'"));
    assert_eq!(page.headers["cache-control"], "no-store");
    assert!(!page.raw.contains("feature-secret"));
    daemon.shutdown().await.unwrap();
    drop(daemon);
    for origin in [
        "ftp://plain.example",
        "plain.example",
        "http://host:bad",
        "http://host:",
        "http://host:0",
        "https://host:bad",
        "https://host:",
        "https://host:65536",
        "https://host:0",
    ] {
        fixture.config.admin.as_mut().unwrap().origin = Some(origin.into());
        assert!(fixture.config.validate().is_err(), "{origin}");
    }
    // Plaintext management is accepted for a public listener, but not alongside TLS.
    let admin = fixture.config.admin.as_mut().unwrap();
    admin.origin = Some("http://plain.example:8990".into());
    assert!(!admin.secure());
    fixture.config.validate().unwrap();
    let admin = fixture.config.admin.as_mut().unwrap();
    admin.tls = Some(fixture.files.config.clone());
    assert!(fixture.config.validate().is_err());
    let admin = fixture.config.admin.as_mut().unwrap();
    admin.origin = Some("https://secure.example".into());
    assert!(admin.secure());
    fixture.config.validate().unwrap();
}
#[tokio::test]
async fn bridge_web_routes_to_authenticated_management_and_tls_admin_accepts_http2_authority() {
    let mut fixture = Fixture::new().await;
    let forwarded = free_port().await;
    fixture.config.locals.insert("admin-entry".into(),toml::from_str(&format!(
        "[[tunnel]]\nid=1\nmode='tcp'\nport={forwarded}\nbind='127.0.0.1'\nbackend=[{{addr='bridge://web'}}]"
    )).unwrap());
    let daemon = fixture.start().await;
    // The management origin is unchanged when reached through a local alias.
    let mut io = TcpStream::connect(("127.0.0.1", forwarded)).await.unwrap();
    io.write_all(format!("GET /api/session HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",fixture.port,fixture.root_token).as_bytes()).await.unwrap();
    let mut data = String::new();
    io.read_to_string(&mut data).await.unwrap();
    assert!(data.starts_with("HTTP/1.1 200"), "{data}");
    assert!(data.contains("\"account\":\"root\""));
    daemon.shutdown().await.unwrap();
    drop(daemon);
    std::fs::remove_file(fixture.config.admin.as_ref().unwrap().state_path.clone()).unwrap();
    fixture.config.admin.as_mut().unwrap().tls = Some(fixture.files.config.clone());
    let daemon = fixture.start().await;
    let io = TcpStream::connect(("127.0.0.1", fixture.port))
        .await
        .unwrap();
    let io = felucca_transport::tls::handshake(
        io,
        "admin.test",
        &felucca_transport::tls::Verification::Pinned {
            fingerprint: fixture.files.identity.fingerprint(),
        },
        &[b"h2"],
    )
    .await
    .unwrap();
    let (mut sender, connection) = hyper::client::conn::http2::handshake(
        hyper_util::rt::TokioExecutor::new(),
        hyper_util::rt::TokioIo::new(io),
    )
    .await
    .unwrap();
    let task = tokio::spawn(connection);
    let request = http::Request::builder()
        .uri(format!("https://127.0.0.1:{}/api/session", fixture.port))
        .header("Authorization", format!("Bearer {}", fixture.root_token))
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    assert_eq!(response.status(), 200);
    drop(sender);
    task.abort();
    daemon.shutdown().await.unwrap();
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_profile_requires_opt_in_admin_and_serializes_real_sampling() {
    let mut fixture = Fixture::new().await;
    let daemon = fixture.start().await;
    let root = fixture.api(&fixture.root_token);
    assert_eq!(
        root.send(
            "POST",
            "/api/profile",
            Some(json!({"seconds":1})),
            0,
            "profile"
        )
        .await
        .status,
        403
    );
    daemon.shutdown().await.unwrap();
    drop(daemon);
    std::fs::remove_file(&fixture.config.admin.as_ref().unwrap().state_path).unwrap();
    fixture.config.admin.as_mut().unwrap().profiling = true;
    let daemon = fixture.start().await;
    for api in [
        Api {
            port: fixture.port,
            headers: vec![],
        },
        fixture.api(&fixture.operator_token),
    ] {
        let response = api
            .send(
                "POST",
                "/api/profile",
                Some(json!({"seconds":1})),
                0,
                "profile",
            )
            .await;
        assert!([401, 403].contains(&response.status));
    }
    for seconds in [0, 31] {
        assert_eq!(
            root.send(
                "POST",
                "/api/profile",
                Some(json!({"seconds":seconds})),
                0,
                "profile"
            )
            .await
            .status,
            400
        );
    }
    let busy = tokio::task::spawn_blocking(|| {
        let end = std::time::Instant::now() + Duration::from_secs(4);
        let mut n = 42u64;
        while std::time::Instant::now() < end {
            for _ in 0..10000 {
                n = std::hint::black_box(n.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        }
        std::hint::black_box(n)
    });
    let first = root.clone();
    let profile = tokio::spawn(async move {
        first
            .send(
                "POST",
                "/api/profile",
                Some(json!({"seconds":2})),
                0,
                "profile",
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        root.send(
            "POST",
            "/api/profile",
            Some(json!({"seconds":1})),
            0,
            "profile"
        )
        .await
        .status,
        409
    );
    let response = profile.await.unwrap();
    assert_eq!(response.status, 200, "{}", response.raw);
    assert_eq!(response.headers["content-type"], "image/svg+xml");
    assert!(response.headers["content-disposition"].contains("attachment"));
    assert!(response.raw.contains("<svg"));
    assert!(
        !response.raw.contains("No CPU samples"),
        "busy worker must produce actual samples"
    );
    assert_eq!(
        daemon.config().0,
        0,
        "sampling is not a configuration mutation"
    );
    busy.await.unwrap();
    // Cancel a long sample by stopping its owning daemon. The process-wide
    // sampling permit remains held until the blocking worker has dropped its guard.
    let mut stream = TcpStream::connect(("127.0.0.1", fixture.port))
        .await
        .unwrap();
    let body = r#"{"seconds":30}"#;
    stream.write_all(format!("POST /api/profile HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",fixture.port,fixture.root_token,body.len(),body).as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    tokio::time::timeout(Duration::from_secs(2), daemon.shutdown())
        .await
        .unwrap()
        .unwrap();
    drop(daemon);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let daemon = fixture.start().await;
    assert_eq!(
        root.send(
            "POST",
            "/api/profile",
            Some(json!({"seconds":1})),
            0,
            "profile"
        )
        .await
        .status,
        200
    );
    daemon.shutdown().await.unwrap();
}
