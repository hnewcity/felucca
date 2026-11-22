//! Credentials and destination policies. The caller must connect to the checked
//! SocketAddr, never resolve a checked name a second time.
use crate::geodata::{GeoIpSet, GeoResolver, GeoSiteSet};
use crate::{ConfigError, Secret};
use felucca_i18n::{Message, Value};
use ipnet::IpNet;
use regex_automata::meta::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, net::IpAddr, sync::Arc};
use subtle::{Choice, ConstantTimeEq};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authentication {
    #[serde(default)]
    pub users: BTreeMap<String, Secret>,
}

impl Authentication {
    pub fn required(&self) -> bool {
        !self.users.is_empty()
    }
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.users.len() > 1024 {
            return Err(ConfigError::invalid(
                "at most 1024 proxy accounts are allowed",
            ));
        }
        for (name, password) in &self.users {
            if name.is_empty()
                || name.len() > 255
                || name.bytes().any(|b| b <= 32 || b == b':' || b == 127)
                || password.is_empty()
                || password.len() > 255
            {
                return Err(ConfigError::invalid(
                    "account names and passwords must contain 1..255 bytes; names cannot contain whitespace or ':'",
                ));
            }
        }
        Ok(())
    }
    /// Digests hide password length and the first mismatching byte during
    /// comparison. These are ephemeral comparisons, not password storage hashes.
    pub fn verify(&self, name: &[u8], password: &[u8]) -> bool {
        let name = Sha256::digest(name);
        let password = Sha256::digest(password);
        let mut accepted = Choice::from(0);
        for (expected_name, expected_password) in &self.users {
            accepted |= name.ct_eq(&Sha256::digest(expected_name.as_bytes()))
                & password.ct_eq(&Sha256::digest(expected_password.as_bytes()));
        }
        bool::from(accepted)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetPolicy {
    /// Empty allows any destination not denied below.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Deny wins, including a permitted name resolving to a denied IP.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Empty means any nonzero port.
    #[serde(default)]
    pub ports: Vec<u16>,
}

#[derive(Debug, Clone)]
enum Rule {
    Any,
    Ip(IpNet),
    Host(String),
    Suffix(String),
    Regex(Regex),
    GeoIp(Arc<GeoIpSet>),
    GeoSite(Arc<GeoSiteSet>),
}
impl Rule {
    fn parse(text: &str, geo: &GeoResolver) -> Result<Self, ConfigError> {
        let bad = || {
            ConfigError::invalid(Message::with(
                "invalid destination rule: {rule}",
                [Value::debug("rule", text)],
            ))
        };
        if let Some(selector) = text.strip_prefix("geoip:") {
            return geo.ip(selector).map(Self::GeoIp);
        }
        if let Some(selector) = text.strip_prefix("geosite:") {
            return geo.site(selector).map(Self::GeoSite);
        }
        if text == "*" {
            return Ok(Self::Any);
        }
        if let Some(pattern) = text.strip_prefix("regex:") {
            if pattern.is_empty() || pattern.len() > 4096 {
                return Err(bad());
            }
            return Regex::new(pattern).map(Self::Regex).map_err(|_| bad());
        }
        let address = text.strip_prefix("ip:").unwrap_or(text);
        if let Ok(ip) = address.parse::<IpAddr>() {
            return Ok(Self::Ip(ip.to_canonical().into()));
        }
        if let Ok(net) = address.parse::<IpNet>() {
            if net.addr() != net.network() {
                return Err(bad());
            }
            if let IpNet::V6(net) = net
                && net.prefix_len() >= 96
                && let Some(ip) = net.addr().to_ipv4_mapped()
            {
                return Ok(Self::Ip(
                    ipnet::Ipv4Net::new(ip, net.prefix_len() - 96)
                        .map_err(|_| bad())?
                        .into(),
                ));
            }
            return Ok(Self::Ip(net));
        }
        let (suffix, host) = if let Some(host) = text.strip_prefix("suffix:") {
            (true, host)
        } else if let Some(host) = text.strip_prefix("*.") {
            (true, host)
        } else {
            (false, text.strip_prefix("domain:").unwrap_or(text))
        };
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty()
            || host.len() > 253
            || host.split('.').any(|s| {
                s.is_empty()
                    || s.len() > 63
                    || s.starts_with('-')
                    || s.ends_with('-')
                    || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(bad());
        }
        Ok(if suffix {
            Self::Suffix(host)
        } else {
            Self::Host(host)
        })
    }
    fn matches(&self, host: &str, ip: IpAddr) -> bool {
        match self {
            Self::Any => true,
            Self::Ip(net) => net.contains(&ip.to_canonical()),
            Self::Host(name) => host == name,
            Self::Suffix(name) => {
                host == name || host.strip_suffix(name).is_some_and(|s| s.ends_with('.'))
            }
            Self::Regex(regex) => regex.is_match(host),
            Self::GeoIp(set) => set.contains(ip),
            Self::GeoSite(set) => set.matches(host),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TargetAcl {
    intersections: Vec<Arc<TargetAcl>>,
    allow: Vec<Rule>,
    deny: Vec<Rule>,
    ports: Vec<u16>,
}
impl TargetAcl {
    pub fn deny_all() -> Self {
        Self {
            deny: vec![Rule::Any],
            ..Self::default()
        }
    }
    pub fn compile(policy: &TargetPolicy) -> Result<Self, ConfigError> {
        Self::compile_with_geo(policy, &GeoResolver::default())
    }
    pub fn compile_with_geo(policy: &TargetPolicy, geo: &GeoResolver) -> Result<Self, ConfigError> {
        if policy.allow.len() + policy.deny.len() > 4096 || policy.ports.contains(&0) {
            return Err(ConfigError::invalid(
                "destination policy allows at most 4096 rules and cannot name port 0",
            ));
        }
        Ok(Self {
            allow: policy
                .allow
                .iter()
                .map(|r| Rule::parse(r, geo))
                .collect::<Result<_, _>>()?,
            deny: policy
                .deny
                .iter()
                .map(|r| Rule::parse(r, geo))
                .collect::<Result<_, _>>()?,
            ports: policy.ports.clone(),
            intersections: Vec::new(),
        })
    }
    pub fn intersect(mut self, boundary: Arc<TargetAcl>) -> Self {
        self.intersections.push(boundary);
        self
    }
    pub fn permits(&self, host: &str, address: std::net::SocketAddr) -> bool {
        if !self
            .intersections
            .iter()
            .all(|acl| acl.permits(host, address))
        {
            return false;
        }
        let host = host
            .trim_matches(['[', ']'])
            .trim_end_matches('.')
            .to_ascii_lowercase();
        address.port() != 0
            && (self.ports.is_empty() || self.ports.contains(&address.port()))
            && !self
                .deny
                .iter()
                .any(|rule| rule.matches(&host, address.ip()))
            && (self.allow.is_empty()
                || self
                    .allow
                    .iter()
                    .any(|rule| rule.matches(&host, address.ip())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_domain_allow_cannot_override_a_denied_resolved_ip() {
        let policy = TargetAcl::compile(&TargetPolicy {
            allow: vec!["suffix:example.com".into()],
            deny: vec!["127.0.0.0/8".into(), "::1".into()],
            ports: vec![443],
        })
        .unwrap();
        assert!(policy.permits("a.b.Example.COM.", "1.2.3.4:443".parse().unwrap()));
        assert!(!policy.permits("example.com", "127.0.0.1:443".parse().unwrap()));
        assert!(!policy.permits("example.com", "[::ffff:127.0.0.1]:443".parse().unwrap()));
        assert!(!policy.permits("example.com", "[::1]:443".parse().unwrap()));
        assert!(!policy.permits("evilexample.com", "1.2.3.4:443".parse().unwrap()));
        assert!(!policy.permits("example.com", "1.2.3.4:80".parse().unwrap()));
    }
    #[test]
    fn exact_names_regex_and_ip_rules() {
        let policy = TargetAcl::compile(&TargetPolicy {
            allow: vec![
                "domain:api.example.com".into(),
                "regex:^db[0-9]+\\.internal$".into(),
                "10.0.0.0/8".into(),
            ],
            ..Default::default()
        })
        .unwrap();
        assert!(policy.permits("db12.internal", "1.2.3.4:80".parse().unwrap()));
        assert!(policy.permits("something", "10.1.2.3:80".parse().unwrap()));
        assert!(!policy.permits("bad.api.example.com", "1.2.3.4:80".parse().unwrap()));
        assert!(!policy.permits("db.internal", "1.2.3.4:80".parse().unwrap()));
        for rule in [
            "regex:[",
            "10.0.0.1/8",
            "domain:",
            "geoip:missing",
            "bad/name",
        ] {
            assert!(
                TargetAcl::compile(&TargetPolicy {
                    allow: vec![rule.into()],
                    ..Default::default()
                })
                .is_err()
            );
        }
    }
    #[test]
    fn multiple_accounts_require_both_fields_and_hide_secrets() {
        let auth = Authentication {
            users: [
                ("alice".into(), "alice-password".into()),
                ("bob".into(), "bob-password".into()),
            ]
            .into(),
        };
        auth.validate().unwrap();
        assert!(auth.verify(b"alice", b"alice-password"));
        assert!(auth.verify(b"bob", b"bob-password"));
        assert!(!auth.verify(b"alice", b"bob-password"));
        assert!(!auth.verify(b"nobody", b"alice-password"));
        assert!(!format!("{auth:?}").contains("alice-password"));
    }
}
