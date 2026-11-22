//! Exercise the management boundary directly, without an HTTP listener or session.
use super::*;
use crate::Daemon;
use felucca_core::{ServerConfig, config::AdminRole};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("felucca-control-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn config() -> ServerConfig {
    serde_json::from_value(json!({
        "bridge": {"bind":"127.0.0.1:0"},
        "user": {"one":{}, "two":{}},
        "client": {
            "one": {"user":"one", "vkey":"first-client-secret", "remark":"original"},
            "two": {"user":"two", "vkey":"second-client-secret"}
        }
    }))
    .unwrap()
}
fn principal(role: AdminRole) -> Principal {
    Principal {
        account: format!("{role:?}"),
        role,
        user: (role != AdminRole::Admin).then(|| "one".into()),
    }
}
fn management(daemon: &Arc<Daemon>) -> Management {
    Management::new(
        Arc::downgrade(daemon),
        Arc::new(tokio::sync::Semaphore::new(8)),
    )
}
fn operation(key: &str, revision: u64) -> Operation {
    Operation {
        key: key.into(),
        revision,
        digest: credentials::digest(key.as_bytes()),
    }
}

#[tokio::test]
async fn typed_commands_enforce_tenant_scope_and_batch_atomicity_without_http() {
    let daemon = Daemon::start(config(), None).await.unwrap();
    let management = management(&daemon);
    let operator = principal(AdminRole::Operator);
    let mut first = serde_json::to_value(&daemon.config().1.clients["one"]).unwrap();
    first["remark"] = json!("changed");
    let changes = vec![
        Change {
            resource: Resource::Client("one".into()),
            method: Write::Put,
            value: first,
        },
        Change {
            resource: Resource::Client("two".into()),
            method: Write::Delete,
            value: Value::Null,
        },
    ];
    assert!(
        management
            .apply(&operator, operation("batch", 0), Command::Batch(changes))
            .await
            .is_err()
    );
    assert_eq!(daemon.config().0, 0);
    assert_eq!(daemon.config().1.clients["one"].remark, "original");
    assert!(daemon.config().1.clients.contains_key("two"));
    assert!(matches!(
        management
            .apply(
                &principal(AdminRole::Viewer),
                operation("viewer", 0),
                Command::Kick(vec!["one".into()])
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        management
            .apply(&operator, operation("sync", 0), Command::Sync)
            .await,
        Err(Error::Forbidden)
    ));
    let applied = management
        .apply(
            &operator,
            operation("kick", 0),
            Command::Kick(vec!["one".into()]),
        )
        .await
        .unwrap();
    assert_eq!(applied.revision, 1);
    assert!(!daemon.config().1.clients["one"].enabled);
    assert!(daemon.config().1.clients["two"].enabled);
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn typed_delete_replay_survives_restart_and_rejects_changed_digest_or_revision() {
    let directory = Directory::new();
    let path = directory.0.join("config.json");
    let bootstrap = config();
    let admin = principal(AdminRole::Admin);
    let delete = || {
        Command::Change(Change {
            resource: Resource::Client("one".into()),
            method: Write::Delete,
            value: Value::Null,
        })
    };
    let daemon = Daemon::start(bootstrap.clone(), Some(&path)).await.unwrap();
    let applied = management(&daemon)
        .apply(&admin, operation("delete", 0), delete())
        .await
        .unwrap();
    assert!(applied.durable && !applied.replayed);
    daemon.shutdown().await.unwrap();
    drop(daemon);

    let daemon = Daemon::start(bootstrap, Some(&path)).await.unwrap();
    let management = management(&daemon);
    let replay = management
        .apply(&admin, operation("delete", 0), delete())
        .await
        .unwrap();
    assert!(replay.replayed && replay.durable);
    assert_eq!(replay.revision, 1);
    assert!(!daemon.config().1.clients.contains_key("one"));
    let mut changed = operation("delete", 0);
    changed.digest = [0; 32];
    assert!(matches!(
        management.apply(&admin, changed, delete()).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        management
            .apply(
                &admin,
                operation("stale", 0),
                Command::Kick(vec!["two".into()])
            )
            .await,
        Err(Error::Conflict(_))
    ));
    daemon.shutdown().await.unwrap();
}
