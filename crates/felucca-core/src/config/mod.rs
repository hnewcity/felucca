//! Configuration model.
//!
//! felucca reads TOML rather than the Go original's INI-with-sections. The nps
//! agent config expresses one tunnel per `[section]` with the mode encoded in
//! sibling keys (`conf/npc.conf`), which means every field is a string that each
//! consumer re-parses. TOML's array-of-tables says the same thing with types
//! intact, so a wrong port or a misspelled mode fails at load rather than at the
//! moment a connection arrives.

pub mod admin;
pub mod agent;
pub mod backend;
pub mod file;
pub mod kcp;
pub mod launch;
pub mod local;
pub mod managed;
pub mod mapping;
pub mod mux;
pub mod network;
pub mod ntp;
pub mod payload;
pub mod proxy_protocol;
pub mod route;
pub mod server;
pub mod tunnel;
pub mod visitor;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use felucca_i18n::{Message, Value};

pub use admin::{AdminAccount, AdminConfig, AdminRole, split_origin};
pub use agent::AgentConfig;
pub use backend::{Backend, HealthCheck, HealthCheckKind, LoadBalance};
pub use file::FileConfig;
pub use kcp::KcpConfig;
pub use local::LocalConfig;
pub use managed::{ManagedConfig, RemotePolicy};
pub use mapping::PortMapping;
pub use mux::MuxConfig;
pub use ntp::NtpConfig;
pub use proxy_protocol::{ProxyAccept, ProxyMode, ProxyVersion};
pub use route::{HttpRouteOptions, HttpScheme, RouteOptions};
pub use server::{
    AcmeChallenge, AcmeConfig, BridgeConfig, ClientEntry, ExternalAccountBinding, HttpProxyConfig,
    LogConfig, PortRange, ServerConfig, StateConfig, TlsConfig, UserEntry,
};
pub use tunnel::{TunnelConfig, TunnelMode};
pub use visitor::{ShareConfig, VisitorConfig, VisitorMode};

/// How an agent reaches its server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    /// Plain TCP. Fine on a trusted network; the tunnel handshake still
    /// authenticates both ends, but traffic is readable in transit.
    Tcp,
    /// TLS over TCP. The default: confidential without extra moving parts.
    #[default]
    Tls,
    /// A WebSocket, for networks where only HTTP gets out.
    Ws,
    /// A WebSocket over TLS.
    Wss,
    /// QUIC, which multiplexes natively and survives packet loss better.
    Quic,
    /// KCP reliable UDP carrying felucca-mux.
    Kcp,
}

impl TransportKind {
    /// Whether this transport encrypts on its own.
    pub fn is_encrypted(self) -> bool {
        matches!(self, Self::Tls | Self::Wss | Self::Quic)
    }

    /// Whether streams ride a `felucca-mux` session, as opposed to native
    /// transport streams.
    pub fn needs_mux(self) -> bool {
        !matches!(self, Self::Quic)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Tls => "tls",
            Self::Ws => "ws",
            Self::Wss => "wss",
            Self::Quic => "quic",
            Self::Kcp => "kcp",
        }
    }
}

/// How an agent decides to trust the server it reached.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum TrustPolicy {
    /// Verify against the bundled Mozilla CA roots.
    #[default]
    Webpki,
    /// Accept exactly one certificate, by SHA-256 fingerprint. The answer for a
    /// self-signed bridge.
    Pinned { fingerprint: String },
    /// Accept any certificate. An attacker in the network path can then read the
    /// tunnel, so this needs to be a deliberate choice rather than a default.
    Insecure,
}

impl<'de> Deserialize<'de> for TrustPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Serde's internally tagged unit variants ignore extra fields even with
        // deny_unknown_fields. Empty struct variants enforce that rule.
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
        enum StrictPolicy {
            Webpki {},
            Pinned { fingerprint: String },
            Insecure {},
        }
        Ok(match StrictPolicy::deserialize(deserializer)? {
            StrictPolicy::Webpki {} => Self::Webpki,
            StrictPolicy::Pinned { fingerprint } => Self::Pinned { fingerprint },
            StrictPolicy::Insecure {} => Self::Insecure,
        })
    }
}

impl TrustPolicy {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if let Self::Pinned { fingerprint } = self {
            let hex: String = fingerprint
                .chars()
                .filter(|c| !matches!(c, ':' | ' ' | '-'))
                .collect();
            if hex.len() != 64 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(ConfigError::invalid(
                    "a pinned fingerprint must contain 64 SHA-256 hex digits",
                ));
            }
        }
        Ok(())
    }
}

/// Validate a dial target without DNS lookup or an async runtime.
pub fn validate_endpoint(address: &str) -> Result<(), ConfigError> {
    let valid = address.rsplit_once(':').is_some_and(|(host, port)| {
        !host.is_empty()
            && port.parse::<u16>().is_ok_and(|p| p != 0)
            && !host
                .bytes()
                .any(|c| c <= 32 || c == 127 || b"/\\@?#".contains(&c))
            && if host.starts_with('[') {
                host.strip_prefix('[')
                    .and_then(|h| h.strip_suffix(']'))
                    .is_some_and(|h| h.parse::<std::net::Ipv6Addr>().is_ok())
            } else {
                !host.contains([':', '[', ']'])
            }
    });
    if valid {
        Ok(())
    } else {
        Err(ConfigError::invalid(Message::with(
            "invalid host:port endpoint: {address}",
            [Value::debug("address", address)],
        )))
    }
}

/// Reads and parses a TOML config file.
pub fn load<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Parses a socket address, defaulting the host to every interface.
///
/// Accepts `14028`, `0.0.0.0:14028` and `[::]:14028`, because an operator writing a
/// port on its own means "listen on this port" rather than nothing at all.
pub fn parse_bind(spec: &str) -> Result<std::net::SocketAddr, ConfigError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(ConfigError::invalid("an empty bind address"));
    }
    if let Ok(port) = spec.parse::<u16>() {
        return Ok(std::net::SocketAddr::from(([0, 0, 0, 0], port)));
    }
    spec.parse().map_err(|_| {
        ConfigError::invalid(Message::with(
            "{spec} is not an address or a port",
            [Value::debug("spec", spec)],
        ))
    })
}

/// Combines a bind host with a separately configured port.
///
/// A tunnel names its host and its port in different fields, so the host here is
/// a bare address — `127.0.0.1`, `::1`, `[::1]` — or empty for every interface.
/// It is deliberately not a `host:port`: carrying a second port would leave two
/// sources of truth, and silently preferring one of them is how a tunnel ends up
/// listening somewhere nobody expected.
pub fn parse_bind_host(host: Option<&str>, port: u16) -> Result<std::net::SocketAddr, ConfigError> {
    use std::net::{IpAddr, Ipv6Addr, SocketAddr};

    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        // Every interface, v4 and v6: an unspecified v6 socket accepts v4 too on
        // every platform felucca targets.
        return Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port));
    };

    // Reject a host that carries its own port rather than guessing which wins.
    // An IPv6 literal is full of colons, so only look outside the brackets.
    let bracketed = host.starts_with('[');
    let stripped = host.trim_matches(['[', ']']);
    if !bracketed && host.matches(':').count() == 1 {
        return Err(ConfigError::invalid(Message::with(
            "a bind host must not include a port: {host} (the port belongs in `port`)",
            [Value::debug("host", host)],
        )));
    }
    if bracketed
        && host
            .rsplit_once(']')
            .is_some_and(|(_, rest)| rest.starts_with(':'))
    {
        return Err(ConfigError::invalid(Message::with(
            "a bind host must not include a port: {host} (the port belongs in `port`)",
            [Value::debug("host", host)],
        )));
    }

    let ip: IpAddr = stripped.parse().map_err(|_| {
        ConfigError::invalid(Message::with(
            "a bind host must be an IP address, not {host} (a name would resolve to several)",
            [Value::debug("host", host)],
        ))
    })?;
    Ok(SocketAddr::new(ip, port))
}

/// DNS host patterns used by routes and SNI certificates. Wildcards match one label.
pub fn normalize_host_pattern(value: &str) -> Result<String, ConfigError> {
    let host = value.trim_end_matches('.').to_ascii_lowercase();
    let dns = host.strip_prefix("*.").unwrap_or(&host);
    if dns.is_empty()
        || dns.len() > 253
        || !dns.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(ConfigError::invalid(Message::with(
            "invalid DNS host pattern: {value}",
            [Value::debug("value", value)],
        )));
    }
    Ok(host)
}

pub fn host_matches(pattern: &str, host: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        host.split_once('.')
            .is_some_and(|(label, tail)| !label.is_empty() && tail == suffix)
    } else {
        pattern == host
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_validation_requires_unambiguous_host_and_port() {
        for good in ["localhost:80", "127.0.0.1:3000", "[::1]:443"] {
            assert!(validate_endpoint(good).is_ok());
        }
        for bad in [
            "host:0",
            "host:65536",
            ":80",
            "::1:80",
            "host:80/path",
            "host\r\nInjected:80",
        ] {
            assert!(validate_endpoint(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn trust_policy_rejects_typos_and_malformed_pins() {
        assert!(toml::from_str::<TrustPolicy>("mode='webpki'\nverfy=false").is_err());
        assert!(
            TrustPolicy::Pinned {
                fingerprint: "invalid".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            TrustPolicy::Pinned {
                fingerprint: "AB:".repeat(31) + "CD"
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn transport_kinds_round_trip_through_toml() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Holder {
            transport: TransportKind,
        }
        for kind in [
            TransportKind::Tcp,
            TransportKind::Tls,
            TransportKind::Ws,
            TransportKind::Wss,
            TransportKind::Quic,
            TransportKind::Kcp,
        ] {
            let text = toml::to_string(&Holder { transport: kind }).unwrap();
            assert!(text.contains(kind.as_str()), "{text}");
            let back: Holder = toml::from_str(&text).unwrap();
            assert_eq!(back.transport, kind);
        }
    }

    #[test]
    fn a_misspelled_transport_is_rejected_at_load() {
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Holder {
            transport: TransportKind,
        }
        let err = toml::from_str::<Holder>("transport = \"tls1\"").unwrap_err();
        assert!(err.to_string().contains("tls1"), "{err}");
    }

    #[test]
    fn encryption_and_mux_needs_are_stated_per_transport() {
        assert!(!TransportKind::Tcp.is_encrypted());
        assert!(!TransportKind::Ws.is_encrypted());
        assert!(!TransportKind::Kcp.is_encrypted());
        assert!(TransportKind::Tls.is_encrypted());
        assert!(TransportKind::Wss.is_encrypted());
        assert!(TransportKind::Quic.is_encrypted());

        // QUIC multiplexes natively; everything else needs mux on top.
        assert!(!TransportKind::Quic.needs_mux());
        for k in [
            TransportKind::Kcp,
            TransportKind::Tcp,
            TransportKind::Tls,
            TransportKind::Ws,
            TransportKind::Wss,
        ] {
            assert!(k.needs_mux(), "{k:?}");
        }
    }

    #[test]
    fn the_default_transport_is_encrypted() {
        assert!(
            TransportKind::default().is_encrypted(),
            "an insecure default is a trap"
        );
    }

    #[test]
    fn trust_policies_round_trip() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Holder {
            trust: TrustPolicy,
        }
        let pinned = Holder {
            trust: TrustPolicy::Pinned {
                fingerprint: "ab".repeat(32),
            },
        };
        let text = toml::to_string(&pinned).unwrap();
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), pinned);

        let parsed: Holder = toml::from_str("[trust]\nmode = \"insecure\"\n").unwrap();
        assert_eq!(parsed.trust, TrustPolicy::Insecure);
    }

    #[test]
    fn the_default_trust_policy_verifies() {
        assert_eq!(TrustPolicy::default(), TrustPolicy::Webpki);
    }

    #[test]
    fn a_bare_port_binds_every_interface() {
        let addr = parse_bind("14028").unwrap();
        assert_eq!(addr.port(), 14028);
        assert!(
            addr.ip().is_unspecified(),
            "a bare port should mean all interfaces"
        );
    }

    #[test]
    fn a_full_address_is_taken_as_written() {
        assert_eq!(
            parse_bind("127.0.0.1:9000").unwrap().to_string(),
            "127.0.0.1:9000"
        );
        assert_eq!(parse_bind("[::1]:9000").unwrap().to_string(), "[::1]:9000");
    }

    #[test]
    fn a_meaningless_bind_is_refused() {
        for bad in ["", "  ", "not-an-address", "1.2.3.4", "99999"] {
            assert!(parse_bind(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn an_absent_bind_host_means_every_interface() {
        let addr = parse_bind_host(None, 8080).unwrap();
        assert!(addr.ip().is_unspecified());
        assert_eq!(addr.port(), 8080);

        // A blank string is the same thing written differently.
        assert!(
            parse_bind_host(Some("  "), 80)
                .unwrap()
                .ip()
                .is_unspecified()
        );
    }

    #[test]
    fn a_bare_host_is_combined_with_the_port() {
        // The case the server registration path needs: `bind` is a host, the port
        // comes from the tunnel's own field.
        assert_eq!(
            parse_bind_host(Some("127.0.0.1"), 9000)
                .unwrap()
                .to_string(),
            "127.0.0.1:9000"
        );
        assert_eq!(
            parse_bind_host(Some("0.0.0.0"), 80).unwrap().to_string(),
            "0.0.0.0:80"
        );
    }

    #[test]
    fn an_ipv6_host_works_bracketed_or_not() {
        assert_eq!(
            parse_bind_host(Some("::1"), 9000).unwrap().to_string(),
            "[::1]:9000"
        );
        assert_eq!(
            parse_bind_host(Some("[::1]"), 9000).unwrap().to_string(),
            "[::1]:9000"
        );
    }

    #[test]
    fn a_host_carrying_its_own_port_is_refused() {
        // Two sources of truth for the port; silently picking one is how a tunnel
        // ends up listening somewhere nobody expected.
        for bad in ["127.0.0.1:9999", "[::1]:9999"] {
            let err = parse_bind_host(Some(bad), 8080).unwrap_err();
            assert!(err.to_string().contains("port"), "{bad}: {err}");
        }
    }

    #[test]
    fn a_hostname_is_refused_as_a_bind_host() {
        // A name can resolve to several addresses; which one to bind is ambiguous.
        let err = parse_bind_host(Some("localhost"), 80).unwrap_err();
        assert!(err.to_string().contains("IP address"), "{err}");
    }

    #[test]
    fn loading_a_missing_file_names_it() {
        let err = load::<toml::Value>(Path::new("/nonexistent/felucca.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }), "{err}");
        assert!(err.to_string().contains("felucca.toml"), "{err}");
    }
}

pub mod peer;

pub mod node;

pub mod event;
