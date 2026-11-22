//! `felucca://host:port?…` launch URIs: one connection handed over as a link.
//!
//! The command line and the desktop client read the same URI, so the parser
//! lives here rather than next to either program.

use serde_json::{Map, Value};

use super::AgentConfig;
use crate::error::ConfigError;

/// The scheme a launch URI starts with, compared case-insensitively.
pub const SCHEME: &str = "felucca";

/// Whether `source` is written as a launch URI.
pub fn is_launch_uri(source: &str) -> bool {
    source
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(SCHEME))
}

/// The agent configuration fields a launch URI describes, before validation.
///
/// Returned as structured data so a caller can still expand environment
/// references before it deserializes the result.
pub fn fields(source: &str) -> Result<Value, ConfigError> {
    if source.len() > 16384 {
        return Err(ConfigError::invalid("launch URI is too long"));
    }
    let url =
        url::Url::parse(source).map_err(|_| ConfigError::invalid("invalid felucca launch URI"))?;
    if !url.scheme().eq_ignore_ascii_case(SCHEME) {
        return Err(ConfigError::invalid("invalid felucca launch URI"));
    }
    let host = url
        .host()
        .ok_or_else(|| ConfigError::invalid("launch URI requires a server host"))?;
    let port = url
        .port()
        .filter(|p| *p != 0)
        .ok_or_else(|| ConfigError::invalid("launch URI requires a nonzero server port"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(ConfigError::invalid(
            "launch URI uses query fields; userinfo, paths and fragments are not supported",
        ));
    }
    let mut fields = Map::from_iter([("server".into(), Value::String(format!("{host}:{port}")))]);
    for (name, value) in url.query_pairs() {
        if !matches!(
            name.as_ref(),
            "vkey" | "key_id" | "transport" | "fingerprint" | "ws_path" | "remote"
        ) {
            return Err(ConfigError::invalid("unknown launch URI field"));
        }
        if value.is_empty()
            || fields
                .insert(name.into_owned(), Value::String(value.into_owned()))
                .is_some()
        {
            return Err(ConfigError::invalid("empty or duplicate launch URI field"));
        }
    }
    if let Some(fingerprint) = fields.remove("fingerprint") {
        fields.insert(
            "trust".into(),
            serde_json::json!({"mode":"pinned", "fingerprint":fingerprint}),
        );
    }
    // `remote=1` hands everything but the connection to the server: tunnels and
    // the destination scope arrive as managed configuration. Destinations are
    // unrestricted unless the console supplies an explicit policy.
    match fields.remove("remote").as_ref().and_then(Value::as_str) {
        None | Some("0") => {}
        Some("1") => {
            fields.insert(
                "remote".into(),
                serde_json::json!({
                    "enabled": true,
                    "console_scope": true,
                    "target_policy": {"allow": []},
                }),
            );
        }
        Some(_) => return Err(ConfigError::invalid("launch URI remote must be 0 or 1")),
    }
    Ok(Value::Object(fields))
}

/// A validated agent configuration from a launch URI.
pub fn parse(source: &str) -> Result<AgentConfig, ConfigError> {
    let mut config: AgentConfig = serde_json::from_value(fields(source)?)
        .map_err(|_| ConfigError::invalid("configuration fields are invalid"))?;
    config.validate()?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_managed_connection_and_rejects_everything_else() {
        let fingerprint = "ab".repeat(32);
        let config = parse(&format!(
            "FELUCCA://[::1]:14028?key_id=edge&vkey=a%2Bb&fingerprint={fingerprint}&remote=1"
        ))
        .unwrap();
        assert_eq!(config.server, "[::1]:14028");
        assert_eq!(config.key_id.as_deref(), Some("edge"));
        assert!(config.remote.enabled);
        assert!(is_launch_uri("Felucca://x:1"));
        assert!(!is_launch_uri("https://x:1"));
        for source in [
            "https://localhost:14028?vkey=x",
            "felucca://localhost?vkey=x",
            "felucca://localhost:14028?vkey=a&typo=b",
            "felucca://localhost:14028?vkey=",
        ] {
            assert!(fields(source).is_err(), "{source}");
        }
    }
}
