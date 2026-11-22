use super::*;
use crate::copy::Direction;

fn key(name: &str) -> ResourceKey {
    ResourceKey::User { name: name.into() }
}
fn capped(store: &MeterStore, name: &str, max: usize) -> Arc<Meter> {
    store.meter(
        key(name),
        ResourceLimits {
            max_connections: Some(max),
            ..Default::default()
        },
    )
}
struct StateFile {
    root: PathBuf,
    config: felucca_core::config::server::StateConfig,
}
impl StateFile {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("felucca-state-test-{}", uuid::Uuid::new_v4()));
        Self {
            config: felucca_core::config::server::StateConfig {
                path: root.join("usage.json"),
                save_interval: Duration::from_secs(1),
            },
            root,
        }
    }
}
impl Drop for StateFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn failed_admission_rolls_back_every_earlier_scope() {
    let store = MeterStore::default();
    let client = capped(&store, "client", 10);
    let user = capped(&store, "user", 1);
    let blocked = capped(&store, "blocked", 0);
    let first = MeterGroup(vec![client.clone(), user.clone()])
        .acquire()
        .unwrap();
    assert!(
        MeterGroup(vec![client.clone(), user.clone()])
            .acquire()
            .is_none()
    );
    assert_eq!(client.live(), 1);
    drop(first);
    assert!(
        MeterGroup(vec![client.clone(), user.clone(), blocked])
            .acquire()
            .is_none()
    );
    assert_eq!(client.live(), 0);
    assert_eq!(user.live(), 0);
}

#[test]
fn users_share_a_hard_connection_cap_across_concurrent_clients() {
    let store = MeterStore::default();
    let user = capped(&store, "team", 1);
    let live = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for index in 0..16 {
            let group = MeterGroup(vec![
                capped(&store, &format!("client{index}"), 1),
                user.clone(),
            ]);
            let live = live.clone();
            scope.spawn(move || {
                for _ in 0..1000 {
                    if let Some(_lease) = group.acquire() {
                        assert_eq!(live.fetch_add(1, Ordering::SeqCst), 0);
                        std::thread::yield_now();
                        assert_eq!(live.fetch_sub(1, Ordering::SeqCst), 1);
                    }
                }
            });
        }
    });
    assert_eq!(user.live(), 0);
}

#[test]
fn limits_change_without_resetting_usage_or_inflight_leases() {
    let store = MeterStore::default();
    let old = capped(&store, "team", 1);
    let lease = MeterGroup(vec![old.clone()]).acquire().unwrap();
    old.traffic.add_inlet(500);
    let new = store.meter(
        key("team"),
        ResourceLimits {
            quota: felucca_core::Quota {
                flow_limit: Some(500),
                expires_at: None,
            },
            max_connections: Some(3),
            ..Default::default()
        },
    );
    assert_eq!(new.live(), 1);
    assert_eq!(new.traffic.total(), 500);
    assert!(!MeterGroup(vec![new.clone()]).permits());
    drop(lease);
    assert_eq!(new.live(), 0);
}

#[test]
fn identical_scopes_do_not_multiply_the_delay_and_all_are_metered() {
    let store = MeterStore::default();
    let limits = ResourceLimits {
        rate_limit: Some(1024),
        ..Default::default()
    };
    let a = store.meter(key("a"), limits);
    let b = store.meter(key("b"), limits);
    let group = MeterGroup(vec![a.clone(), b.clone()]);
    let delay = group.delay(2048).unwrap();
    assert!(
        delay >= Duration::from_millis(900) && delay <= Duration::from_millis(1100),
        "{delay:?}"
    );
    group.record(Direction::Inlet, 40);
    group.record(Direction::Export, 80);
    assert_eq!(
        a.traffic.stats(),
        TrafficStats {
            inlet: 40,
            export: 80
        }
    );
    assert_eq!(b.traffic.stats(), a.traffic.stats());
}

#[test]
fn atomic_snapshots_preserve_offline_resources_and_reject_concurrent_writers() {
    let file = StateFile::new();
    let store = MeterStore::open(Some(&file.config)).unwrap();
    assert!(MeterStore::open(Some(&file.config)).is_err());
    let meter = store.meter(key("team"), ResourceLimits::default());
    meter.traffic.add_inlet(1234);
    store.save().unwrap();
    meter.traffic.add_export(5678);
    store.save().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file.config.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(store);
    let restored = MeterStore::open(Some(&file.config)).unwrap();
    assert_eq!(
        restored.stats(&key("team")).unwrap(),
        TrafficStats {
            inlet: 1234,
            export: 5678
        }
    );
    restored.save().unwrap();
    drop(restored);
    let restored = MeterStore::open(Some(&file.config)).unwrap();
    let group = MeterGroup(vec![restored.meter(
        key("team"),
        ResourceLimits {
            quota: felucca_core::Quota {
                flow_limit: Some(6000),
                expires_at: None,
            },
            ..Default::default()
        },
    )]);
    assert!(!group.permits());
}

#[test]
fn future_state_fails_closed_without_overwriting_the_file() {
    let file = StateFile::new();
    std::fs::create_dir_all(&file.root).unwrap();
    let data = r#"{"version":99,"counters":[]}"#;
    std::fs::write(&file.config.path, data).unwrap();
    assert!(MeterStore::open(Some(&file.config)).is_err());
    assert_eq!(std::fs::read_to_string(&file.config.path).unwrap(), data);
}

#[test]
fn damaged_state_is_kept_aside_and_replaced_by_the_previous_snapshot_or_empty_counters() {
    let corrupt = |file: &StateFile| -> Vec<_> {
        std::fs::read_dir(&file.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("usage.json.corrupt-")
            })
            .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
            .collect()
    };
    let file = StateFile::new();
    std::fs::create_dir_all(&file.root).unwrap();
    for data in [
        "broken",
        r#"{"version":1,"counters":[[{"kind":"user","name":"a"},{"inlet_bytes":1}],[{"kind":"user","name":"a"},{"inlet_bytes":2}]]}"#,
    ] {
        std::fs::write(&file.config.path, data).unwrap();
        let store = MeterStore::open(Some(&file.config)).unwrap();
        assert!(store.stats(&key("a")).is_none());
        drop(store);
        assert_eq!(corrupt(&file), [data]);
        for entry in std::fs::read_dir(&file.root).unwrap().flatten() {
            if entry.file_name().to_string_lossy().contains(".corrupt-") {
                std::fs::remove_file(entry.path()).unwrap();
            }
        }
    }

    // With two saves behind it, the snapshot replaced by the latest is used.
    let file = StateFile::new();
    let store = MeterStore::open(Some(&file.config)).unwrap();
    let meter = store.meter(key("team"), ResourceLimits::default());
    meter.traffic.add_inlet(7);
    store.save().unwrap();
    meter.traffic.add_inlet(5);
    store.save().unwrap();
    drop((meter, store));
    std::fs::write(&file.config.path, "broken").unwrap();
    let store = MeterStore::open(Some(&file.config)).unwrap();
    assert_eq!(store.stats(&key("team")).unwrap().inlet, 7);
    assert_eq!(corrupt(&file), ["broken"]);
}

#[test]
fn unused_tunnel_meters_are_pruned_but_used_or_held_ones_are_kept() {
    let file = StateFile::new();
    let store = MeterStore::open(Some(&file.config)).unwrap();
    let tunnel = |id| ResourceKey::Tunnel {
        client: "c".into(),
        id,
    };
    drop(store.meter(tunnel(1), ResourceLimits::default()));
    let held = store.meter(tunnel(2), ResourceLimits::default());
    store
        .meter(tunnel(3), ResourceLimits::default())
        .traffic
        .add_inlet(1);
    drop(store.meter(key("idle-user"), ResourceLimits::default()));
    store.save().unwrap();
    assert!(store.stats(&tunnel(1)).is_none());
    assert!(store.stats(&tunnel(2)).is_some());
    assert_eq!(store.stats(&tunnel(3)).unwrap().inlet, 1);
    assert!(store.stats(&key("idle-user")).is_some());
    drop(held);
}

#[test]
fn traffic_import_commits_totals_and_retry_marker_together_and_survives_restart() {
    let file = StateFile::new();
    let store = MeterStore::open(Some(&file.config)).unwrap();
    let live = store.meter(key("team"), ResourceLimits::default());
    live.traffic.add_inlet(5);
    let deltas = [(
        key("team"),
        (
            ResourceLimits::default(),
            TrafficStats {
                inlet: 100,
                export: 200,
            },
        ),
    )]
    .into();
    assert!(
        !store
            .import("platform".into(), "report-1".into(), [1; 32], deltas)
            .unwrap()
    );
    assert_eq!(
        live.traffic.stats(),
        TrafficStats {
            inlet: 105,
            export: 200
        }
    );
    live.traffic.add_export(8);
    store.save().unwrap();
    drop(store);
    let store = MeterStore::open(Some(&file.config)).unwrap();
    assert!(
        store
            .import(
                "platform".into(),
                "report-1".into(),
                [1; 32],
                BTreeMap::new()
            )
            .unwrap()
    );
    assert!(
        store
            .import(
                "platform".into(),
                "report-1".into(),
                [2; 32],
                BTreeMap::new()
            )
            .is_err()
    );
    assert_eq!(
        store.stats(&key("team")).unwrap(),
        TrafficStats {
            inlet: 105,
            export: 208
        }
    );
}

#[test]
fn failed_traffic_import_does_not_change_usage_or_consume_idempotency_key() {
    let file = StateFile::new();
    let store = MeterStore::open(Some(&file.config)).unwrap();
    let meter = store.meter(key("team"), ResourceLimits::default());
    meter.traffic.add_inlet(5);
    std::fs::create_dir(&file.config.path).unwrap();
    let deltas: BTreeMap<_, _> = [(
        key("team"),
        (
            ResourceLimits::default(),
            TrafficStats {
                inlet: 7,
                export: 0,
            },
        ),
    )]
    .into();
    assert!(
        store
            .import("a".into(), "k".into(), [1; 32], deltas.clone())
            .is_err()
    );
    assert_eq!(meter.traffic.total(), 5);
    assert!(store.imports.lock().is_empty());
    assert!(std::fs::read_dir(&file.root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    }));
    std::fs::remove_dir(&file.config.path).unwrap();
    assert!(
        !store
            .import("a".into(), "k".into(), [1; 32], deltas)
            .unwrap()
    );
    assert_eq!(meter.traffic.total(), 12);
}
