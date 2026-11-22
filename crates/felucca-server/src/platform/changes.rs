//! Resource metadata only. Configuration values and credentials never enter events.
use felucca_core::ServerConfig;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default)]
pub(super) struct Change {
    pub resource: &'static str,
    pub action: &'static str,
    pub user: Option<String>,
    pub client: Option<String>,
    pub tunnel: Option<u32>,
    pub host: Option<String>,
}
pub(super) fn between(old: &ServerConfig, new: &ServerConfig) -> Vec<Change> {
    let mut result = Vec::new();
    for id in keys(&old.clients, &new.clients) {
        if changed(old.clients.get(&id), new.clients.get(&id)) {
            let value = new
                .clients
                .get(&id)
                .or_else(|| old.clients.get(&id))
                .expect("union key");
            result.push(Change {
                resource: "client",
                action: action(old.clients.contains_key(&id), new.clients.contains_key(&id)),
                user: value.user.clone(),
                client: Some(id),
                ..Default::default()
            });
        }
    }
    for id in keys(&old.users, &new.users) {
        if changed(old.users.get(&id), new.users.get(&id)) {
            result.push(Change {
                resource: "user",
                action: action(old.users.contains_key(&id), new.users.contains_key(&id)),
                user: Some(id),
                ..Default::default()
            });
        }
    }
    for client in keys(&old.managed, &new.managed) {
        let before = old
            .managed
            .get(&client)
            .map(|m| {
                m.tunnels
                    .iter()
                    .filter_map(|t| t.id.map(|id| (id, t)))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let after = new
            .managed
            .get(&client)
            .map(|m| {
                m.tunnels
                    .iter()
                    .filter_map(|t| t.id.map(|id| (id, t)))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        for id in before
            .keys()
            .chain(after.keys())
            .copied()
            .collect::<BTreeSet<_>>()
        {
            if changed(before.get(&id), after.get(&id)) {
                let user = new
                    .clients
                    .get(&client)
                    .or_else(|| old.clients.get(&client))
                    .and_then(|c| c.user.clone());
                let op = action(before.contains_key(&id), after.contains_key(&id));
                result.push(Change {
                    resource: "tunnel",
                    action: op,
                    user: user.clone(),
                    client: Some(client.clone()),
                    tunnel: Some(id),
                    ..Default::default()
                });
                let hosts = before
                    .get(&id)
                    .into_iter()
                    .flat_map(|t| t.hosts.iter())
                    .chain(after.get(&id).into_iter().flat_map(|t| t.hosts.iter()))
                    .collect::<BTreeSet<_>>();
                for host in hosts {
                    result.push(Change {
                        resource: "host",
                        action: op,
                        user: user.clone(),
                        client: Some(client.clone()),
                        tunnel: Some(id),
                        host: Some(host.clone()),
                    });
                }
            }
        }
    }
    // Bound work persisted for one bulk replacement. The accompanying global
    // config.changed event always tells subscribers to refresh current state.
    result.truncate(128);
    result
}
fn keys<T>(
    a: &std::collections::HashMap<String, T>,
    b: &std::collections::HashMap<String, T>,
) -> BTreeSet<String> {
    a.keys().chain(b.keys()).cloned().collect()
}
fn changed<T: serde::Serialize>(a: Option<T>, b: Option<T>) -> bool {
    let value = |v: Option<T>| {
        v.map(|v| serde_json::to_value(v).expect("config JSON"))
            .unwrap_or(Value::Null)
    };
    value(a) != value(b)
}
fn action(before: bool, after: bool) -> &'static str {
    match (before, after) {
        (false, true) => "created",
        (true, false) => "deleted",
        _ => "updated",
    }
}
