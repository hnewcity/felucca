//! Private service sharing and visitor-owned local listeners.
use crate::{AclMode, ConfigError, Secret, geodata::GeoResolver};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareConfig {
    #[serde(default)]
    pub direct: bool,
    pub client: String,
    pub tunnel: u32,
    pub secret: Secret,
    #[serde(default)]
    pub dynamic: bool,
    /// Empty admits any authenticated client that also proves the share secret.
    #[serde(default)]
    pub allow_clients: Vec<String>,
    #[serde(default)]
    pub acl_mode: AclMode,
    #[serde(default)]
    pub acl_rules: String,
}
impl ShareConfig {
    pub fn validate(&self, geo: &GeoResolver) -> Result<(), ConfigError> {
        if !super::local::valid_name(&self.client) || self.tunnel == 0 {
            return Err(ConfigError::invalid(
                "shares require a routable client name and nonzero tunnel id",
            ));
        }
        validate_secret(&self.secret)?;
        if self.allow_clients.len() > 1024 {
            return Err(ConfigError::invalid(
                "shares allow at most 1024 visitor identities",
            ));
        }
        crate::Acl::parse_with_geo(self.acl_mode, &self.acl_rules, geo)
            .map_err(|e| ConfigError::invalid(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisitorMode {
    #[default]
    Tcp,
    Udp,
    Socks5,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisitorConfig {
    #[serde(default)]
    pub connection: super::peer::VisitorConnection,
    pub name: String,
    pub share: String,
    pub secret: Secret,
    pub bind: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub mode: VisitorMode,
    #[serde(default)]
    pub auth: crate::access::Authentication,
    #[serde(default = "connections")]
    pub max_connections: usize,
    #[serde(default = "idle", with = "humantime_serde")]
    pub udp_idle: Duration,
    #[serde(default)]
    pub acl_mode: AclMode,
    #[serde(default)]
    pub acl_rules: String,
}
fn connections() -> usize {
    64
}
fn idle() -> Duration {
    Duration::from_secs(60)
}
fn validate_secret(secret: &Secret) -> Result<(), ConfigError> {
    if !(16..=4096).contains(&secret.len()) {
        return Err(ConfigError::invalid("share secrets require 16..4096 bytes"));
    }
    Ok(())
}
impl VisitorConfig {
    pub fn validate(&self, geo: &GeoResolver) -> Result<(), ConfigError> {
        if !super::local::valid_name(&self.name) || !super::local::valid_name(&self.share) {
            return Err(ConfigError::invalid(
                "visitor and share names require 1..128 ASCII letters, digits, dots, hyphens or underscores",
            ));
        }
        validate_secret(&self.secret)?;
        self.auth.validate()?;
        if (self.auth.required() && self.mode != VisitorMode::Socks5)
            || (self.mode == VisitorMode::Socks5 && self.target.is_some())
        {
            return Err(ConfigError::invalid(
                "visitor auth is only for SOCKS5; SOCKS5 targets come from requests",
            ));
        }
        if let Some(target) = &self.target {
            super::validate_endpoint(target)?;
        }
        if super::parse_bind(&self.bind)?.port() == 0
            || !(1..=4096).contains(&self.max_connections)
            || self.udp_idle < Duration::from_secs(1)
            || self.udp_idle > Duration::from_secs(3600)
        {
            return Err(ConfigError::invalid(
                "visitors require a fixed port, 1..4096 connections and UDP idle time 1s..1h",
            ));
        }
        crate::Acl::parse_with_geo(self.acl_mode, &self.acl_rules, geo)
            .map_err(|e| ConfigError::invalid(e.to_string()))?;
        Ok(())
    }
}

pub fn valid_name(name: &str) -> bool {
    super::local::valid_name(name)
}

#[cfg(test)]
mod tests {

    #[test]
    fn private_tunnels_reject_ambiguous_public_routing_and_visitors_require_verified_bridges() {
        let private = "server='localhost:14028'\nvkey='key'\n[[tunnel]]\nmode='tcp'\npublic=false\nbackend=[{addr='localhost:8080'}]\n";
        let mut config: crate::AgentConfig = toml::from_str(private).unwrap();
        config.validate().unwrap();
        for field in ["port=8080", "bind='127.0.0.1'", "hosts=['example.test']"] {
            let mut config: crate::AgentConfig =
                toml::from_str(&format!("{private}{field}")).unwrap();
            assert!(config.validate().is_err(), "{field}");
        }
        let input = "server='localhost:14028'\nvkey='key'\n[[visitor]]\nname='office'\nshare='office'\nsecret='a-long-test-secret'\nbind='127.0.0.1:8000'";
        let mut config: crate::AgentConfig = toml::from_str(input).unwrap();
        config.validate().unwrap();
        config.transport = crate::TransportKind::Tcp;
        assert!(config.validate().is_err());
        config.transport = crate::TransportKind::Tls;
        config.trust = crate::TrustPolicy::Insecure;
        assert!(config.validate().is_err());
        config.trust = crate::TrustPolicy::default();
        config.visitors.push(config.visitors[0].clone());
        assert!(config.validate().is_err());
        config.visitors.pop();
        config.visitors[0].secret = "short".into();
        assert!(config.validate().is_err());
    }
}
