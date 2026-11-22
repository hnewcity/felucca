//! Server-owned routes use the same backends as an agent, without a bridge.
use super::{TunnelConfig, network::NetworkConfig};
use crate::{AclMode, ConfigError, traffic::ResourceLimits};
use felucca_i18n::{Message, Value};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub acl_mode: AclMode,
    #[serde(default)]
    pub acl_rules: String,
    #[serde(default)]
    pub limits: ResourceLimits,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default = "timeout", with = "humantime_serde")]
    pub dial_timeout: Duration,
    #[serde(default, rename = "mapping")]
    pub mappings: Vec<super::PortMapping>,
    #[serde(default, rename = "tunnel")]
    pub tunnels: Vec<TunnelConfig>,
}

fn timeout() -> Duration {
    Duration::from_secs(10)
}

impl LocalConfig {
    pub(super) fn validate(
        &mut self,
        geo: &crate::geodata::GeoResolver,
    ) -> Result<(), ConfigError> {
        self.network.validate()?;
        if self.network.proxy.is_some() {
            return Err(ConfigError::invalid(
                "local routes have no bridge for an upstream proxy",
            ));
        }
        if self.dial_timeout.is_zero() || self.dial_timeout > Duration::from_secs(300) {
            return Err(ConfigError::invalid(
                "local dial_timeout must be positive and at most 5m",
            ));
        }
        crate::Acl::parse_with_geo(self.acl_mode, &self.acl_rules, geo)
            .map_err(|e| ConfigError::invalid(e.to_string()))?;
        super::tunnel::normalize_tunnels(&mut self.tunnels, &mut self.mappings, geo, true)?;
        if self.tunnels.is_empty() {
            return Err(ConfigError::invalid(
                "a local group needs at least one tunnel",
            ));
        }
        Ok(())
    }
}

/// Only an operator-owned local backend may select another server service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualTarget {
    Tunnel { client: String, id: u32 },
    File { client: String, id: u32 },
    Bridge(BridgeTarget),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTarget {
    Tcp,
    Tls,
    Ws,
    Wss,
    Web,
    Http,
}

impl VirtualTarget {
    pub fn parse(value: &str) -> Result<Option<Self>, ConfigError> {
        let Some((scheme, rest)) = value.split_once("://") else {
            return Ok(None);
        };
        let invalid = || {
            ConfigError::invalid(Message::with(
                "invalid virtual target: {target}",
                [Value::debug("target", value)],
            ))
        };
        match scheme {
            "tunnel" | "file" => {
                let (client, id) = rest.split_once('/').ok_or_else(invalid)?;
                if !valid_name(client) {
                    return Err(invalid());
                }
                let id = id
                    .parse::<u32>()
                    .ok()
                    .filter(|id| *id != 0)
                    .ok_or_else(invalid)?;
                Ok(Some(if scheme == "file" {
                    Self::File {
                        client: client.into(),
                        id,
                    }
                } else {
                    Self::Tunnel {
                        client: client.into(),
                        id,
                    }
                }))
            }
            "bridge" => Ok(Some(Self::Bridge(match rest {
                "tcp" => BridgeTarget::Tcp,
                "tls" => BridgeTarget::Tls,
                "ws" => BridgeTarget::Ws,
                "wss" => BridgeTarget::Wss,
                "web" => BridgeTarget::Web,
                "http" => BridgeTarget::Http,
                _ => return Err(invalid()),
            }))),
            _ => Err(invalid()),
        }
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_groups_validate_and_reserve_their_registry_namespace() {
        let good = "[local.office]\n[[local.office.tunnel]]\nid=7\nmode='tcp'\nport=9000\nbackend=[{addr='127.0.0.1:9001'}]";
        let mut config: super::super::ServerConfig = toml::from_str(good).unwrap();
        config.validate().unwrap();
        config.validate().unwrap();
        assert_eq!(config.locals["office"].tunnels.len(), 1);
        assert_eq!(config.locals["office"].tunnels[0].id, Some(7));
        for extra in [
            "\n[client.'local.office']\nvkey='k'",
            "\n[local.empty]",
            "\n[local.bad]\nuser='missing'",
        ] {
            let mut config: super::super::ServerConfig =
                toml::from_str(&format!("{good}{extra}")).unwrap();
            assert!(config.validate().is_err(), "{extra}");
        }
        let mut agent: super::super::AgentConfig = toml::from_str(
            "server='127.0.0.1:14028'\nvkey='k'\n[[tunnel]]\nmode='tcp'\nport=9000\nbackend=[{addr='tunnel://local.office/7'}]"
        ).unwrap();
        assert!(
            agent.validate().is_err(),
            "agents cannot select server virtual services"
        );
    }

    #[test]
    fn virtual_targets_reject_ambiguous_or_unknown_schemes() {
        for value in [
            "bridge://",
            "bridge://quic",
            "http://localhost",
            "tunnel://office",
            "tunnel://office/0",
            "tunnel://office/1/x",
            "file://../x",
            "file://office/1?x",
        ] {
            assert!(VirtualTarget::parse(value).is_err(), "{value}");
        }
        assert_eq!(VirtualTarget::parse("127.0.0.1:80").unwrap(), None);
        assert_eq!(
            VirtualTarget::parse("file://local.office/7").unwrap(),
            Some(VirtualTarget::File {
                client: "local.office".into(),
                id: 7
            })
        );
        assert_eq!(
            VirtualTarget::parse("bridge://wss").unwrap(),
            Some(VirtualTarget::Bridge(BridgeTarget::Wss))
        );
    }
}
