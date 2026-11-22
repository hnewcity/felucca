//! Side-effect-free activation planning and credential generation.
use super::{Error, Principal, Result, bad, resources};
use felucca_core::ServerConfig;
use serde_json::{Value, json};

/// Daily management changes do not own infrastructure listeners. Everything
/// else keeps the generation replacement path, including future config fields.
pub(crate) fn live_compatible(old: &ServerConfig, next: &ServerConfig) -> bool {
    let infrastructure = |config: &ServerConfig| {
        let mut value = serde_json::to_value(config).expect("serializable configuration");
        for key in ["client", "managed", "share", "admin"] {
            value
                .as_object_mut()
                .expect("configuration object")
                .remove(key);
        }
        value
    };
    infrastructure(old) == infrastructure(next)
}

pub(crate) fn restart_fields(old: &ServerConfig, next: &ServerConfig) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if !crate::admin::compatible(old.admin.as_ref(), next.admin.as_ref()) {
        fields.push("admin");
    }
    if !crate::platform::compatible(old.node.as_ref(), next.node.as_ref()) {
        fields.push("node");
    }
    if old.log != next.log {
        fields.push("log");
    }
    if old.state.as_ref().map(|s| &s.path) != next.state.as_ref().map(|s| &s.path) {
        fields.push("state.path");
    }
    fields
}

pub(crate) fn token(principal: &Principal) -> Result<Value> {
    resources::admin(principal)?;
    let token = super::credentials::token();
    Ok(json!({"token_hash":super::credentials::token_hash(&token),"token":token}))
}

pub(crate) fn export(
    principal: &Principal,
    current: &ServerConfig,
    revision: u64,
    runtime_path: Option<&std::path::Path>,
    input: Value,
) -> Result<Value> {
    resources::admin(principal)?;
    let mut next: ServerConfig = resources::parse(input, Some(resources::value(current)))?;
    next.validate().map_err(|error| bad(error.message()))?;
    let next_revision = revision
        .checked_add(1)
        .ok_or_else(|| Error::Unavailable("configuration revision exhausted".into()))?;
    Ok(json!({
        "revision": revision,
        "runtime_path": runtime_path,
        "startup": next,
        "snapshot": {"version":1,"revision":next_revision,"config":next,"operations":[]},
        "restart_fields": restart_fields(current, &next),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plans_startup_only_changes_without_applying_them() {
        let old = ServerConfig::default();
        let mut next = old.clone();
        assert!(restart_fields(&old, &next).is_empty());
        next.log.level = "debug".into();
        assert_eq!(restart_fields(&old, &next), vec!["log"]);
        next.state = Some(felucca_core::config::StateConfig {
            path: "traffic.json".into(),
            save_interval: std::time::Duration::from_secs(5),
        });
        assert_eq!(restart_fields(&old, &next), vec!["log", "state.path"]);
        next.bridge.bind = "127.0.0.1:14028".into();
        assert_eq!(restart_fields(&old, &next), vec!["log", "state.path"]);
    }
}
