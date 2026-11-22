use crate::{Acl, AclMode, ConfigError};
use felucca_i18n::{Message, Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyVersion {
    #[default]
    Off,
    V1,
    V2,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    Off,
    Optional,
    Required,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyAccept {
    #[serde(default)]
    pub mode: ProxyMode,
    /// IP addresses/CIDRs of load balancers permitted to assert an original peer.
    #[serde(default)]
    pub trusted: Vec<String>,
}
impl ProxyAccept {
    pub fn acl(&self) -> Result<Acl, ConfigError> {
        if self.mode == ProxyMode::Off {
            return Ok(Acl::off());
        }
        if self.trusted.len() > 4096 {
            return Err(ConfigError::invalid("too many trusted PROXY senders"));
        }
        Acl::parse(AclMode::Allow, &self.trusted.join(",")).map_err(|e| {
            ConfigError::invalid(Message::with(
                "proxy_accept.trusted: {reason}",
                [Value::nested("reason", e.message())],
            ))
        })
    }
}
