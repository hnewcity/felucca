//! Direct connectivity is explicit at the broker, provider and share.
use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::{net::IpAddr, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct P2pServerConfig {
    /// UDP listener used only for authenticated endpoint observations.
    pub bind: String,
    /// Public hostname/address and UDP port reachable by both agents.
    pub advertise: String,
    #[serde(default = "timeout", with = "humantime_serde")]
    pub timeout: Duration,
    #[serde(default = "pending")]
    pub max_pending: usize,
}
fn timeout() -> Duration {
    Duration::from_secs(5)
}
fn pending() -> usize {
    128
}
impl P2pServerConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let address = crate::config::parse_bind(&self.bind)?;
        crate::config::validate_endpoint(&self.advertise)?;
        if address.port() == 0
            || self.advertise.len() > 4096
            || !(1..=4096).contains(&self.max_pending)
            || self.timeout < Duration::from_millis(500)
            || self.timeout > Duration::from_secs(10)
        {
            return Err(ConfigError::invalid(
                "p2p needs a fixed UDP port, advertise endpoint, 1..4096 pending tickets and timeout 500ms..10s",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct P2pAgentConfig {
    /// Permit this agent to serve its routes over authenticated direct QUIC.
    pub enabled: bool,
    /// Local UDP address; absent uses the agent's configured outgoing IP.
    pub bind_ip: Option<IpAddr>,
    pub max_sessions: usize,
}
impl Default for P2pAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_ip: None,
            max_sessions: 64,
        }
    }
}
impl P2pAgentConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=1024).contains(&self.max_sessions)
            || self.bind_ip.is_some_and(|ip| ip.is_multicast())
        {
            return Err(ConfigError::invalid(
                "agent p2p requires 1..1024 sessions and a unicast local IP",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisitorConnection {
    #[default]
    Relay,
    PreferDirect,
    Direct,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_fixed_broker_endpoints_and_resource_bounds() {
        let good = "bind='0.0.0.0:8025'\nadvertise='server.example:8025'";
        let config: P2pServerConfig = toml::from_str(good).unwrap();
        config.validate().unwrap();
        for extra in ["\nmax_pending=0", "\ntimeout='10ms'", "\ntimeout='11s'"] {
            let config: P2pServerConfig = toml::from_str(&format!("{good}{extra}")).unwrap();
            assert!(config.validate().is_err());
        }
        for value in [
            "bind='localhost:8025'\nadvertise='server.example:8025'",
            "bind='0.0.0.0:0'\nadvertise='server.example:8025'",
            "bind='0.0.0.0:8025'\nadvertise='https://host:8025'",
        ] {
            let config: P2pServerConfig = toml::from_str(value).unwrap();
            assert!(config.validate().is_err());
        }
        assert!(toml::from_str::<P2pServerConfig>(&format!("{good}\nenabeld=true")).is_err());
        assert!(!P2pAgentConfig::default().enabled);
        for text in ["max_sessions=0", "max_sessions=1025", "bind_ip='224.0.0.1'"] {
            let config: P2pAgentConfig = toml::from_str(text).unwrap();
            assert!(config.validate().is_err());
        }
    }
}
