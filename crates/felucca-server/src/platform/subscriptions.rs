//! Temporary event sinks belong to one WS session and vanish on disconnect.
use crate::control::Principal;
use crate::control::resources;
use felucca_core::{
    ServerConfig,
    config::event::{EventContent, EventSelector},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Subscription {
    #[serde(default = "enabled")]
    enabled: bool,
    #[serde(default)]
    selector: EventSelector,
    #[serde(default)]
    content: EventContent,
}
fn enabled() -> bool {
    true
}
#[derive(Default)]
pub(super) struct Subscriptions(BTreeMap<String, Subscription>);
impl Subscriptions {
    pub fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Value,
        principal: &Principal,
        config: &ServerConfig,
    ) -> (u16, Value) {
        let Some(suffix) = path.strip_prefix("/api/realtime/subscriptions") else {
            return (404, json!({"error":"unknown subscription path"}));
        };
        if suffix.is_empty() && method == "GET" {
            return (
                200,
                json!({"subscriptions":self.0.iter().map(|(id,config)|json!({"id":id,"config":config})).collect::<Vec<_>>()}),
            );
        }
        let Some(id) = suffix.strip_prefix('/').filter(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        }) else {
            return (
                400,
                json!({"error":"subscription path requires a valid id"}),
            );
        };
        match method {
            "GET" => match self.0.get(id) {
                Some(value) => (200, json!({"id":id,"config":value})),
                None => (404, json!({"error":"subscription not found"})),
            },
            "DELETE" => (200, json!({"id":id,"removed":self.0.remove(id).is_some()})),
            "PUT" => {
                if !self.0.contains_key(id) && self.0.len() >= 32 {
                    return (
                        429,
                        json!({"error":"at most 32 subscriptions per connection"}),
                    );
                }
                if body.to_string().len() > 32 * 1024 {
                    return (413, json!({"error":"subscription exceeds 32 KiB"}));
                }
                let value: Subscription = match serde_json::from_value(body) {
                    Ok(value) => value,
                    Err(_) => return (400, json!({"error":"invalid subscription"})),
                };
                if let Err(error) = value
                    .selector
                    .validate()
                    .and_then(|()| value.content.validate())
                {
                    return (400, json!({"error":error.to_string()}));
                }
                // Selectors refine authority; they cannot grant access to another tenant.
                if value
                    .selector
                    .users
                    .iter()
                    .any(|user| !resources::visible(principal, Some(user)))
                    || value.selector.clients.iter().any(|id| {
                        config.clients.get(id).is_none_or(|client| {
                            !resources::visible(principal, client.user.as_deref())
                        })
                    })
                {
                    return (
                        403,
                        json!({"error":"subscription selector is outside current authority"}),
                    );
                }
                self.0.insert(id.into(), value);
                (200, json!({"id":id,"temporary":true}))
            }
            _ => (405, json!({"error":"subscriptions use GET, PUT or DELETE"})),
        }
    }
    pub fn callbacks(&self, envelope: &Value) -> impl Iterator<Item = Value> + '_ {
        let event = envelope.get("event").cloned().unwrap_or(Value::Null);
        let envelope = envelope.clone();
        self.0.iter().filter(move |(_,subscription)|subscription.enabled && subscription.selector.matches(&event))
            .map(move |(id,subscription)|match subscription.content.render(&envelope) {
                Ok((body,headers))=>json!({"type":"callback","subscription":id,"body":body,"headers":headers}),
                Err(_)=>json!({"type":"error","subscription":id,"error":"subscription template could not be rendered"}),
            })
    }
}
