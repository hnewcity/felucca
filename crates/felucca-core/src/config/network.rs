//! Agent egress settings. Proxy credentials stay separate from printable addresses.
use crate::{ConfigError, Secret};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    #[serde(default)]
    pub local_ip: Option<IpAddr>,
    /// Also bind direct backend sockets, not just the bridge/proxy connection.
    #[serde(default)]
    pub forward_local_ip: bool,
    /// Literal recursive DNS endpoints. Empty uses the system resolver.
    #[serde(default)]
    pub dns: Vec<SocketAddr>,
    /// Applies to TCP/TLS/WS/WSS bridge connections; backend sockets remain direct.
    #[serde(default)]
    pub proxy: Option<UpstreamProxy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamProxy {
    pub kind: ProxyKind,
    pub server: String,
    #[serde(default)]
    pub auth: Option<ProxyCredentials>,
    #[serde(default = "remote_dns")]
    pub remote_dns: bool,
}
fn remote_dns() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyKind {
    Http,
    Socks5,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyCredentials {
    pub username: String,
    pub password: Secret,
}

impl NetworkConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.forward_local_ip && self.local_ip.is_none() {
            return Err(ConfigError::invalid("forward_local_ip requires local_ip"));
        }
        if self.local_ip.is_some_and(|ip| ip.is_multicast()) {
            return Err(ConfigError::invalid("local_ip cannot be multicast"));
        }
        if self.dns.len() > 8
            || self.dns.iter().any(|address| {
                address.port() == 0
                    || address.ip().is_multicast()
                    || address.ip().is_unspecified()
                    || self
                        .local_ip
                        .is_some_and(|local| local.is_ipv4() != address.is_ipv4())
            })
        {
            return Err(ConfigError::invalid(
                "dns needs at most eight nonzero endpoints matching the local_ip address family",
            ));
        }
        if let Some(proxy) = &self.proxy {
            super::validate_endpoint(&proxy.server)?;
            if proxy.server.len() > 1024 {
                return Err(ConfigError::invalid("proxy endpoint is too long"));
            }
            if let Some(auth) = &proxy.auth {
                let max = if proxy.kind == ProxyKind::Socks5 {
                    255
                } else {
                    1024
                };
                if auth.username.is_empty()
                    || auth.username.len() > max
                    || auth.password.len() > max
                    || proxy.kind == ProxyKind::Socks5 && auth.password.is_empty()
                    || auth.username.contains(':')
                    || auth.username.chars().any(char::is_control)
                {
                    return Err(ConfigError::invalid("invalid upstream proxy credentials"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn egress_validation_rejects_inert_or_ambiguous_settings_and_redacts_passwords() {
        for text in [
            "forward_local_ip=true",
            "dns=['127.0.0.1:0']",
            "local_ip='::1'\ndns=['127.0.0.1:53']",
            "dns=['0.0.0.0:53']",
            "local_ip='224.0.0.1'",
            "[proxy]\nkind='socks5'\nserver='localhost:1080'\n[proxy.auth]\nusername='u'\npassword=''",
        ] {
            let config: NetworkConfig = toml::from_str(text).unwrap();
            assert!(config.validate().is_err(), "{text}");
        }
        let config: NetworkConfig = toml::from_str(
            "local_ip='127.0.0.1'\ndns=['127.0.0.1:5353']\n[proxy]\nkind='http'\nserver='localhost:8080'\n[proxy.auth]\nusername='u'\npassword='proxy-secret'"
        ).unwrap();
        config.validate().unwrap();
        assert!(!format!("{config:?}").contains("proxy-secret"));
        assert!(toml::from_str::<NetworkConfig>("dns_server=[]").is_err());
    }
}
