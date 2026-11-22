//! Expand bounded, ordered public/target port lists into ordinary tunnels.
use super::{Backend, HealthCheck, LoadBalance, RouteOptions, TunnelConfig, TunnelMode};
use crate::{AclMode, ConfigError, TrustPolicy};
use serde::{Deserialize, Serialize};

const MAX_PORTS: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortMapping {
    pub mode: TunnelMode,
    pub server_ports: String,
    pub target_ports: String,
    #[serde(default = "localhost")]
    pub target_host: String,
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub trust: TrustPolicy,
    #[serde(default)]
    pub acl_mode: AclMode,
    #[serde(default)]
    pub acl_rules: String,
    #[serde(default)]
    pub options: RouteOptions,
    #[serde(default)]
    pub target_policy: crate::access::TargetPolicy,
    #[serde(default)]
    pub health: Option<HealthCheck>,
    #[serde(default)]
    pub remark: String,
}
fn localhost() -> String {
    "127.0.0.1".into()
}

impl PortMapping {
    pub fn expand(&self) -> Result<Vec<TunnelConfig>, ConfigError> {
        self.expand_with_geo(&crate::geodata::GeoResolver::default())
    }
    pub fn expand_with_geo(
        &self,
        geo: &crate::geodata::GeoResolver,
    ) -> Result<Vec<TunnelConfig>, ConfigError> {
        if !matches!(self.mode, TunnelMode::Tcp | TunnelMode::Udp) {
            return Err(ConfigError::invalid(
                "port mappings support tcp and udp only",
            ));
        }
        let public = ports(&self.server_ports, true)?;
        let target = ports(&self.target_ports, false)?;
        if public.len() != target.len() {
            return Err(ConfigError::invalid(
                "server_ports and target_ports need the same number of ports",
            ));
        }
        let host = self.target_host.trim_matches(['[', ']']);
        if host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(ConfigError::invalid("target_host cannot contain a port"));
        }
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.into()
        };
        public
            .into_iter()
            .zip(target)
            .map(|(port, backend)| {
                let mut tunnel = TunnelConfig {
                    public: true,
                    enabled: true,
                    file: None,
                    id: None,
                    mode: self.mode,
                    port,
                    bind: self.bind.clone(),
                    backends: vec![Backend {
                        addr: format!("{host}:{backend}"),
                        weight: 1,
                        tls: self.tls,
                        trust: self.trust.clone(),
                    }],
                    balance: LoadBalance::default(),
                    health: self.health.clone(),
                    hosts: vec![],
                    acl_mode: self.acl_mode,
                    acl_rules: self.acl_rules.clone(),
                    remark: self.remark.clone(),
                    options: self.options.clone(),
                    target_policy: self.target_policy.clone(),
                };
                tunnel.validate(geo)?;
                Ok(tunnel)
            })
            .collect()
    }
}

fn ports(text: &str, unique: bool) -> Result<Vec<u16>, ConfigError> {
    let mut ports = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let parse = |text: &str| {
        text.trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| ConfigError::invalid("port lists require numbers 1..65535"))
    };
    for part in text.split(',') {
        let (start, end) = if let Some((start, end)) = part.split_once('-') {
            (parse(start)?, parse(end)?)
        } else {
            let port = parse(part)?;
            (port, port)
        };
        if start > end || ports.len() + usize::from(end - start) + 1 > MAX_PORTS {
            return Err(ConfigError::invalid(
                "port range is reversed or exceeds 4096 ports",
            ));
        }
        for port in start..=end {
            if unique && !seen.insert(port) {
                return Err(ConfigError::invalid("duplicate port in mapping"));
            }
            ports.push(port);
        }
    }
    Ok(ports)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_preserve_order_and_reject_ambiguous_or_unbounded_mappings() {
        assert_eq!(
            ports("9001-9003,10001,8000", true).unwrap(),
            [9001, 9002, 9003, 10001, 8000]
        );
        assert_eq!(ports("65535", true).unwrap(), [65535]);
        assert_eq!(ports("8080,8080", false).unwrap(), [8080, 8080]);
        for invalid in [
            "",
            "0",
            "1-65535",
            "9000-8999",
            "8000,8000",
            "1-2-3",
            "9000,",
        ] {
            assert!(ports(invalid, true).is_err(), "{invalid}");
        }
    }
    #[test]
    fn expansion_is_ordered_idempotent_and_checks_collisions_with_explicit_tunnels() {
        let text = r#"
server = "localhost:14028"
vkey = "secret"
[[mapping]]
mode = "tcp"
server_ports = "9001-9003,10000"
target_ports = "8001-8003,8080"
target_host = "::1"
"#;
        let mut agent: super::super::AgentConfig = toml::from_str(text).unwrap();
        agent.validate().unwrap();
        assert_eq!(agent.tunnels.len(), 4);
        assert_eq!(agent.tunnels[3].port, 10000);
        assert_eq!(agent.tunnels[3].backends[0].addr, "[::1]:8080");
        agent.validate().unwrap();
        assert_eq!(agent.tunnels.len(), 4);
        let mut mismatched: super::super::AgentConfig =
            toml::from_str(&text.replace("8001-8003,8080", "8080")).unwrap();
        assert!(
            mismatched
                .validate()
                .unwrap_err()
                .to_string()
                .contains("same number")
        );
        let mut duplicate: super::super::AgentConfig = toml::from_str(&format!(
            r#"{text}
[[tunnel]]
mode = "tcp"
port = 9001
backend = [{{addr = "127.0.0.1:1234"}}]
"#
        ))
        .unwrap();
        assert!(
            duplicate
                .validate()
                .unwrap_err()
                .to_string()
                .contains("two tunnels")
        );
    }
}
