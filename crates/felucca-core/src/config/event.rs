//! Bounded event selectors and JSON templates shared by webhook/WS subscriptions.
use crate::ConfigError;
use felucca_i18n::{Message, Value as MessageValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventSelector {
    pub event_names: Vec<String>,
    pub resources: Vec<String>,
    pub actions: Vec<String>,
    pub users: Vec<String>,
    pub clients: Vec<String>,
    pub tunnels: Vec<u32>,
    pub hosts: Vec<String>,
}
impl EventSelector {
    pub fn validate(&self) -> Result<(), ConfigError> {
        for values in [
            &self.event_names,
            &self.resources,
            &self.actions,
            &self.users,
            &self.clients,
            &self.hosts,
        ] {
            if values.len() > 64
                || values
                    .iter()
                    .any(|v| v.is_empty() || v.len() > 253 || v.chars().any(char::is_control))
            {
                return Err(ConfigError::invalid(
                    "event selectors allow <=64 nonempty values of <=253 bytes per field",
                ));
            }
        }
        if self.tunnels.len() > 64 || self.tunnels.contains(&0) {
            return Err(ConfigError::invalid(
                "event tunnel selectors require <=64 nonzero ids",
            ));
        }
        Ok(())
    }
    pub fn matches(&self, event: &Value) -> bool {
        let names = self.event_names.is_empty()
            || event
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|name| {
                    self.event_names.iter().any(|pattern| {
                        pattern
                            .strip_suffix('*')
                            .map_or(pattern == name, |prefix| name.starts_with(prefix))
                    })
                });
        names
            && includes(&self.resources, event, "resource")
            && includes(&self.actions, event, "action")
            && includes(&self.users, event, "user")
            && includes(&self.clients, event, "client")
            && includes(&self.hosts, event, "host")
            && (self.tunnels.is_empty()
                || event
                    .get("tunnel")
                    .and_then(Value::as_u64)
                    .is_some_and(|id| self.tunnels.iter().any(|&n| u64::from(n) == id)))
    }
}
fn includes(values: &[String], event: &Value, key: &str) -> bool {
    values.is_empty()
        || event
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| values.iter().any(|v| v == s))
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventContent {
    /// JSON template. An exact placeholder preserves the substituted JSON type.
    pub body_template: Option<Value>,
    pub header_templates: BTreeMap<String, String>,
}
impl EventContent {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if serde_json::to_vec(self)
            .map_err(|_| ConfigError::invalid("invalid event content"))?
            .len()
            > 16 * 1024
            || self.header_templates.len() > 16
        {
            return Err(ConfigError::invalid(
                "event content exceeds 16 KiB/16 headers",
            ));
        }
        if let Some(body) = &self.body_template {
            walk(body, 0, &Value::Null)?;
        }
        for (name, value) in &self.header_templates {
            let parsed = http::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ConfigError::invalid("invalid event header name"))?;
            if parsed.as_str().starts_with("x-felucca-")
                || matches!(
                    parsed.as_str(),
                    "authorization"
                        | "cookie"
                        | "host"
                        | "content-length"
                        | "transfer-encoding"
                        | "connection"
                        | "upgrade"
                        | "proxy-authorization"
                )
            {
                return Err(ConfigError::invalid(
                    "event template cannot override authority, framing or signature headers",
                ));
            }
            http::HeaderValue::from_str(value)
                .map_err(|_| ConfigError::invalid("invalid event header template"))?;
            interpolate(value, &Value::Null)?;
        }
        Ok(())
    }
    pub fn render(
        &self,
        envelope: &Value,
    ) -> Result<(Value, BTreeMap<String, String>), ConfigError> {
        let body = match &self.body_template {
            Some(value) => walk(value, 0, envelope)?,
            None => envelope.clone(),
        };
        if serde_json::to_vec(&body)
            .map_err(|_| ConfigError::invalid("invalid event JSON"))?
            .len()
            > 64 * 1024
        {
            return Err(ConfigError::invalid("rendered event body exceeds 64 KiB"));
        }
        let mut headers = BTreeMap::new();
        for (name, value) in &self.header_templates {
            let value = interpolate(value, envelope)?;
            if value.len() > 8192 || http::HeaderValue::from_str(&value).is_err() {
                return Err(ConfigError::invalid("rendered event header is invalid"));
            }
            headers.insert(name.clone(), value);
        }
        Ok((body, headers))
    }
}
fn lookup(name: &str, envelope: &Value) -> Result<Value, ConfigError> {
    let value = match name {
        "node" | "id" | "epoch" | "event" => envelope.get(name),
        "event.kind" | "event.target" | "event.resource" | "event.action" | "event.user"
        | "event.client" | "event.tunnel" | "event.host" | "event.sequence" | "event.revision"
        | "event.timestamp_ms" | "event.traffic" => envelope
            .get("event")
            .and_then(|event| event.get(name.strip_prefix("event.").expect("event field"))),
        _ => {
            return Err(ConfigError::invalid(Message::with(
                "unsupported event placeholder: {name}",
                [MessageValue::new("name", name)],
            )));
        }
    };
    Ok(value.cloned().unwrap_or(Value::Null))
}
fn interpolate(template: &str, envelope: &Value) -> Result<String, ConfigError> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let end = rest[start + 2..]
            .find("}}")
            .ok_or_else(|| ConfigError::invalid("unclosed event placeholder"))?
            + start
            + 2;
        let value = lookup(rest[start + 2..end].trim(), envelope)?;
        match value {
            Value::Null => {}
            Value::String(text) => output.push_str(&text),
            value => output.push_str(&value.to_string()),
        }
        if output.len() > 64 * 1024 {
            return Err(ConfigError::invalid("rendered template exceeds limit"));
        }
        rest = &rest[end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}
fn walk(value: &Value, depth: usize, envelope: &Value) -> Result<Value, ConfigError> {
    if depth > 16 {
        return Err(ConfigError::invalid("event template nesting exceeds 16"));
    }
    Ok(match value {
        Value::String(text) => {
            if let Some(name) = text
                .strip_prefix("{{")
                .and_then(|s| s.strip_suffix("}}"))
                .filter(|s| !s.contains("{{") && !s.contains("}}"))
            {
                lookup(name.trim(), envelope)?
            } else {
                Value::String(interpolate(text, envelope)?)
            }
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|v| walk(v, depth + 1, envelope))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(k, v)| Ok((k.clone(), walk(v, depth + 1, envelope)?)))
                .collect::<Result<_, ConfigError>>()?,
        ),
        value => value.clone(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn json_templates_preserve_types_escape_strings_and_never_expand_inserted_placeholders() {
        let content:EventContent=serde_json::from_value(json!({"body_template":{"event":"{{event}}","message":"seen {{event.client}}","count":"{{event.sequence}}"}})).unwrap();
        content.validate().unwrap();
        let (value, _) = content
            .render(&json!({"event":{"client":"a\"{{node}}","sequence":42},"node":"secret"}))
            .unwrap();
        assert_eq!(value["message"], "seen a\"{{node}}");
        assert_eq!(value["count"], 42);
        assert_eq!(value["event"]["client"], "a\"{{node}}");
        let encoded = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), value);
    }
    #[test]
    fn selector_fields_intersect_and_templates_cannot_override_authority() {
        let filter: EventSelector = serde_json::from_value(
            json!({"event_names":["client.*"],"clients":["own"],"users":["team"]}),
        )
        .unwrap();
        filter.validate().unwrap();
        assert!(filter.matches(&json!({"kind":"client.updated","client":"own","user":"team"})));
        assert!(
            !filter.matches(&json!({"kind":"client.updated","client":"foreign","user":"team"}))
        );
        for name in ["Authorization", "X-Felucca-Signature", "Content-Length"] {
            let content = EventContent {
                header_templates: [(name.into(), "x".into())].into(),
                ..Default::default()
            };
            assert!(content.validate().is_err());
        }
        let content = EventContent {
            body_template: Some(json!("{{secret}}")),
            ..Default::default()
        };
        assert!(content.validate().is_err());
    }
}
