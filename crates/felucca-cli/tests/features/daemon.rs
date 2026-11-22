use super::*;
use felucca_server::Daemon;

fn local(port: u16, backend: u16) -> ServerConfig {
    toml::from_str(&format!("[bridge]\nbind='127.0.0.1:0'\n[[local.office.tunnel]]\nid=1\nmode='tcp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{backend}'}}]")).unwrap()
}
async fn get(port: u16) -> String {
    http_request(
        port,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await
}

#[tokio::test]
async fn runtime_configuration_commits_reloads_and_survives_restart_with_meters() {
    let files = identity("runtime.test").await;
    let config_path = files.root.join("runtime.json");
    let port = free_port().await;
    let original = local(port, web_backend("original").await);
    let daemon = Daemon::start(original.clone(), Some(&config_path))
        .await
        .unwrap();
    let pin = daemon.server().fingerprint();
    assert!(get(port).await.contains("original"));
    let next = local(port, web_backend("replacement").await);
    let applied = daemon.apply(next, 0).await.unwrap();
    assert_eq!(applied.revision, 1);
    assert!(applied.durable);
    assert_eq!(
        pin,
        daemon.server().fingerprint(),
        "ephemeral identity must survive an in-process replacement"
    );
    assert!(get(port).await.contains("replacement"));
    assert!(daemon.healthy());
    let stats = daemon
        .server()
        .registry()
        .store
        .stats(&felucca_server::resources::ResourceKey::Client {
            name: "local.office".into(),
        })
        .unwrap();
    assert!(stats.inlet > 0 && stats.export > 0);
    daemon.shutdown().await.unwrap();
    drop(daemon);
    let restarted = Daemon::start(original, Some(&config_path)).await.unwrap();
    assert_eq!(
        pin,
        restarted.server().fingerprint(),
        "default identity must survive a process restart"
    );
    assert_eq!(restarted.config().0, 1);
    assert!(
        get(port).await.contains("replacement"),
        "saved desired state must override the bootstrap"
    );
    restarted.shutdown().await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&config_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn damaged_persistent_identity_is_not_silently_replaced() {
    let files = identity("damaged-identity.test").await;
    let path = files.root.join("runtime.json");
    let config = local(free_port().await, echo().await);
    let daemon = Daemon::start(config.clone(), Some(&path)).await.unwrap();
    daemon.shutdown().await.unwrap();
    drop(daemon);
    let identity_path = files.root.join("runtime.json.identity.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&identity_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    std::fs::write(&identity_path, b"damaged").unwrap();
    assert!(Daemon::start(config, Some(&path)).await.is_err());
    assert_eq!(std::fs::read(identity_path).unwrap(), b"damaged");
}

#[tokio::test]
async fn failed_listener_activation_rolls_back_without_committing_or_resetting_usage() {
    let files = identity("rollback.test").await;
    let path = files.root.join("runtime.json");
    let port = free_port().await;
    let original = local(port, web_backend("working").await);
    let daemon = Daemon::start(original.clone(), Some(&path)).await.unwrap();
    assert!(get(port).await.contains("working"));
    let before = std::fs::read(&path).unwrap();
    let old_usage = daemon
        .server()
        .registry()
        .get("local.office")
        .unwrap()
        .traffic
        .total();
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bad = local(occupied.local_addr().unwrap().port(), echo().await);
    let error = daemon.apply(bad, 0).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("previous configuration restored"),
        "{error}"
    );
    assert_eq!(daemon.config().0, 0);
    assert!(daemon.healthy());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        daemon
            .server()
            .registry()
            .get("local.office")
            .unwrap()
            .traffic
            .total(),
        old_usage
    );
    assert!(get(port).await.contains("working"));
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_config_commit_rolls_back_and_prepared_files_do_not_leak() {
    let files = identity("commit.test").await;
    let path = files.root.join("runtime.json");
    let port = free_port().await;
    let config = local(port, web_backend("retained").await);
    let daemon = Daemon::start(config.clone(), Some(&path)).await.unwrap();
    let saved = files.root.join("saved.json");
    std::fs::rename(&path, &saved).unwrap();
    std::fs::create_dir(&path).unwrap();
    let instance = daemon.server();
    daemon.apply(config, 0).await.unwrap_err();
    assert!(
        Arc::ptr_eq(&instance, &daemon.server()),
        "a failed live commit must not replace the runtime"
    );
    assert!(get(port).await.contains("retained"));
    assert_eq!(daemon.config().0, 0);
    assert!(!std::fs::read_dir(&files.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".felucca-config-")
    }));
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&saved, &path).unwrap();
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn config_revisions_serialize_writers_and_caller_cancellation_does_not_interrupt_commit() {
    let port = free_port().await;
    let original = local(port, echo().await);
    let daemon = Daemon::start(original.clone(), None).await.unwrap();
    let (a, b) = tokio::join!(
        daemon.apply(original.clone(), 0),
        daemon.apply(original.clone(), 0)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let error = a.err().or(b.err()).unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    {
        use std::future::Future;
        let operation = daemon.apply(original, 1);
        tokio::pin!(operation);
        std::future::poll_fn(|cx| {
            assert!(operation.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        // Drop the requesting future while the daemon still owns the transaction.
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while daemon.config().0 != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    roundtrip(TcpStream::connect(("127.0.0.1", port)).await.unwrap()).await;
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn runtime_config_lock_and_corruption_fail_closed_without_overwriting_state() {
    let files = identity("config-lock.test").await;
    let path = files.root.join("runtime.json");
    let last_good = files.root.join("runtime.json.last-good");
    let original = local(free_port().await, echo().await);
    let daemon = Daemon::start(original.clone(), Some(&path)).await.unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert_eq!(std::fs::read(&last_good).unwrap(), saved);
    assert!(Daemon::start(original.clone(), Some(&path)).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    daemon.shutdown().await.unwrap();
    drop(daemon);
    // Without a last-good copy an unreadable file still fails closed.
    std::fs::remove_file(&last_good).unwrap();
    std::fs::write(&path, b"{broken").unwrap();
    assert!(Daemon::start(original.clone(), Some(&path)).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    assert!(Daemon::load_configuration(original, Some(&path)).is_err());
}

#[tokio::test]
async fn an_unreadable_runtime_config_starts_from_the_last_good_copy_and_is_kept() {
    let files = identity("config-recover.test").await;
    let path = files.root.join("runtime.json");
    let port = free_port().await;
    let original = local(port, web_backend("original").await);
    let daemon = Daemon::start(original.clone(), Some(&path)).await.unwrap();
    let next = local(port, web_backend("saved").await);
    assert_eq!(daemon.apply(next, 0).await.unwrap().revision, 1);
    let saved = std::fs::read(&path).unwrap();
    daemon.shutdown().await.unwrap();
    drop(daemon);

    // A boot from an intact file does not rewrite it, so a full disk cannot
    // stop the server from starting.
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let daemon = Daemon::start(original.clone(), Some(&path)).await.unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    daemon.shutdown().await.unwrap();
    drop(daemon);

    std::fs::write(&path, b"{broken").unwrap();
    let recovered = Daemon::load_configuration(original.clone(), Some(&path)).unwrap();
    let last_good = files.root.join("runtime.json.last-good");
    let expected = Daemon::load_configuration(original.clone(), Some(&last_good)).unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let daemon = Daemon::start(original.clone(), Some(&path)).await.unwrap();
    assert_eq!(daemon.config().0, 1);
    assert!(get(port).await.contains("saved"));
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    let kept: Vec<_> = std::fs::read_dir(&files.root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("runtime.json.corrupt-")
        })
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read(kept[0].path()).unwrap(), b"{broken");
    daemon.shutdown().await.unwrap();
}
