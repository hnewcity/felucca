//! Source-address access control.
//!
//! Mirrors the Go original's `AclOff` / `AclWhitelist` / `AclBlacklist` modes
//! (`lib/file/obj.go`), but rules are parsed once at load time instead of being
//! re-split from a comma-separated string on every connection.

use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;

use ipnet::IpNet;

use crate::error::RuleParseError;
use crate::geodata::{GeoIpSet, GeoResolver};

/// How an [`Acl`] treats an address that matches one of its rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclMode {
    /// No filtering; every address is allowed.
    #[default]
    Off,
    /// Only addresses matching a rule are allowed.
    Allow,
    /// Addresses matching a rule are rejected; everything else is allowed.
    Deny,
}

/// One access-control rule: a bare address or a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AclRule {
    Addr(IpAddr),
    Net(IpNet),
    GeoIp(Arc<GeoIpSet>),
}

impl AclRule {
    pub fn matches(&self, addr: IpAddr) -> bool {
        match self {
            Self::GeoIp(set) => set.contains(addr),
            Self::Addr(want) => want.to_canonical() == addr.to_canonical(),
            // Dual-stack listeners may report IPv4 users as ::ffff:a.b.c.d.
            Self::Net(IpNet::V4(net)) => match addr.to_canonical() {
                IpAddr::V4(addr) => net.contains(&addr),
                IpAddr::V6(_) => false,
            },
            Self::Net(IpNet::V6(net)) => match addr {
                IpAddr::V6(addr) => net.contains(&addr),
                IpAddr::V4(_) => false,
            },
        }
    }
}

impl FromStr for AclRule {
    type Err = RuleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let err = |reason: &str| RuleParseError {
            kind: "acl",
            value: s.to_owned(),
            reason: reason.to_owned(),
        };

        if s.is_empty() {
            return Err(err("empty rule"));
        }
        if s.contains('/') {
            // A prefix is only meaningful if the host bits are actually zero;
            // `10.0.0.5/8` is a config bug we surface rather than silently widen.
            let net: IpNet = s.parse().map_err(|_| err("not an IP network"))?;
            if net.network() != net.addr() {
                return Err(err("host bits set; write the network address"));
            }
            return Ok(Self::Net(net));
        }
        s.parse::<IpAddr>()
            .map(Self::Addr)
            .map_err(|_| err("not an IP address"))
    }
}

/// A parsed access-control list.
#[derive(Debug, Clone, Default)]
pub struct Acl {
    mode: AclMode,
    rules: Vec<AclRule>,
}

impl Acl {
    /// An ACL that permits everything.
    pub fn off() -> Self {
        Self::default()
    }

    pub fn deny_all() -> Self {
        Self {
            mode: AclMode::Allow,
            rules: Vec::new(),
        }
    }

    /// Parses `rules`, splitting on commas, semicolons and newlines.
    ///
    /// An `Allow` list with no rules would deny every connection, which is
    /// almost never what someone means when they leave the field blank, so it is
    /// rejected rather than silently locking the tunnel out.
    pub fn parse(mode: AclMode, rules: &str) -> Result<Self, RuleParseError> {
        Self::parse_with_geo(mode, rules, &GeoResolver::default())
    }

    pub fn parse_with_geo(
        mode: AclMode,
        rules: &str,
        geo: &GeoResolver,
    ) -> Result<Self, RuleParseError> {
        if rules.len() > 256 * 1024 {
            return Err(RuleParseError {
                kind: "acl",
                value: String::new(),
                reason: "ACL exceeds 256 KiB".into(),
            });
        }
        let rules: Vec<AclRule> = rules
            .split([',', ';', '\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|rule| match rule.strip_prefix("geoip:") {
                Some(selector) => {
                    geo.ip(selector)
                        .map(AclRule::GeoIp)
                        .map_err(|e| RuleParseError {
                            kind: "acl",
                            value: rule.into(),
                            reason: e.to_string(),
                        })
                }
                None => AclRule::from_str(rule),
            })
            .collect::<Result<_, _>>()?;

        if rules.len() > 4096 {
            return Err(RuleParseError {
                kind: "acl",
                value: String::new(),
                reason: "ACL exceeds 4096 rules".into(),
            });
        }
        if mode == AclMode::Allow && rules.is_empty() {
            return Err(RuleParseError {
                kind: "acl",
                value: String::new(),
                reason: "allow-list mode with no rules would reject everything".to_owned(),
            });
        }
        Ok(Self { mode, rules })
    }

    pub fn mode(&self) -> AclMode {
        self.mode
    }

    pub fn rules(&self) -> &[AclRule] {
        &self.rules
    }

    /// Whether `addr` may connect.
    pub fn permits(&self, addr: IpAddr) -> bool {
        let hit = || self.rules.iter().any(|r| r.matches(addr));
        match self.mode {
            AclMode::Off => true,
            AclMode::Allow => hit(),
            AclMode::Deny => !hit(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn off_permits_everything() {
        let acl = Acl::off();
        assert!(acl.permits(ip("8.8.8.8")));
        assert!(acl.permits(ip("::1")));
    }

    #[test]
    fn allow_list_admits_only_listed() {
        let acl = Acl::parse(AclMode::Allow, "10.0.0.0/8, 127.0.0.1").unwrap();
        assert!(acl.permits(ip("10.1.2.3")));
        assert!(acl.permits(ip("127.0.0.1")));
        assert!(!acl.permits(ip("8.8.8.8")));
    }

    #[test]
    fn deny_list_rejects_only_listed() {
        let acl = Acl::parse(AclMode::Deny, "8.8.8.8").unwrap();
        assert!(!acl.permits(ip("8.8.8.8")));
        assert!(acl.permits(ip("10.1.2.3")));
    }

    #[test]
    fn ipv4_mapped_sources_cannot_bypass_ipv4_rules() {
        let denied = Acl::parse(AclMode::Deny, "127.0.0.0/8").unwrap();
        assert!(!denied.permits(ip("::ffff:127.0.0.1")));
        let allowed = Acl::parse(AclMode::Allow, "192.0.2.1").unwrap();
        assert!(allowed.permits(ip("::ffff:192.0.2.1")));
        assert!(!allowed.permits(ip("::ffff:192.0.2.2")));
    }

    #[test]
    fn separators_are_interchangeable() {
        let acl = Acl::parse(AclMode::Deny, "1.1.1.1\n2.2.2.2; 3.3.3.3,\n").unwrap();
        assert_eq!(acl.rules().len(), 3);
    }

    #[test]
    fn ipv6_networks_work() {
        let acl = Acl::parse(AclMode::Allow, "2001:db8::/32").unwrap();
        assert!(acl.permits(ip("2001:db8::1")));
        assert!(!acl.permits(ip("2001:dead::1")));
    }

    #[test]
    fn empty_allow_list_is_rejected() {
        assert!(Acl::parse(AclMode::Allow, "   ").is_err());
        // ...but an empty deny list is harmless: it denies nothing.
        assert!(
            Acl::parse(AclMode::Deny, "")
                .unwrap()
                .permits(ip("8.8.8.8"))
        );
    }

    #[test]
    fn host_bits_in_a_prefix_are_rejected() {
        let err = Acl::parse(AclMode::Deny, "10.0.0.5/8").unwrap_err();
        assert!(err.reason.contains("host bits"), "{err}");
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(Acl::parse(AclMode::Deny, "not-an-ip").is_err());
        assert!(Acl::parse(AclMode::Deny, "10.0.0.0/99").is_err());
    }
}
