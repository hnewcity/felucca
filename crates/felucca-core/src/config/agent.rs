//! Agent configuration.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::tunnel::normalize_tunnels;
use super::{TransportKind, TrustPolicy, TunnelConfig};
use crate::error::ConfigError;
use crate::secret::Secret;
use felucca_i18n::{Message, Value};

/// A whole agent configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    #[serde(default)]
    pub p2p: super::peer::P2pAgentConfig,
    #[serde(default, rename = "visitor")]
    pub visitors: Vec<super::VisitorConfig>,
    #[serde(default)]
    pub remote: super::RemotePolicy,
    #[serde(default)]
    pub kcp: super::KcpConfig,
    #[serde(default)]
    pub ntp: Option<super::NtpConfig>,
    #[serde(default)]
    pub network: super::network::NetworkConfig,
    #[serde(default)]
    pub geo: crate::geodata::GeoFiles,
    #[serde(default, rename = "mapping")]
    pub mappings: Vec<super::PortMapping>,
    #[serde(default)]
    pub mux: super::MuxConfig,
    /// The server to dial, as `host:port`.
    pub server: String,

    /// The shared verify key proving this agent's identity.
    pub vkey: Secret,

    /// Identifier the server knows this agent by. Defaults to the hostname, so a
    /// single-agent install needs no value here.
    #[serde(default)]
    pub key_id: Option<String>,

    #[serde(default)]
    pub transport: TransportKind,

    #[serde(default)]
    pub trust: TrustPolicy,

    /// Path for a WebSocket transport. Ignored otherwise.
    #[serde(default = "default_ws_path")]
    pub ws_path: String,

    /// How long to wait before the first reconnect attempt.
    #[serde(default = "default_reconnect_min", with = "humantime_serde")]
    pub reconnect_min: Duration,

    /// Ceiling for the reconnect backoff.
    #[serde(default = "default_reconnect_max", with = "humantime_serde")]
    pub reconnect_max: Duration,

    /// How long to wait for a local target to accept a connection.
    #[serde(default = "default_dial_timeout", with = "humantime_serde")]
    pub dial_timeout: Duration,

    /// Cap on simultaneous proxied connections. `None` means unlimited.
    #[serde(default)]
    pub max_connections: Option<usize>,

    /// Tunnels this agent asks the server to expose.
    #[serde(default, rename = "tunnel")]
    pub tunnels: Vec<TunnelConfig>,
}

fn default_ws_path() -> String {
    "/ws".to_owned()
}

fn default_reconnect_min() -> Duration {
    Duration::from_secs(1)
}

fn default_reconnect_max() -> Duration {
    Duration::from_secs(60)
}

fn default_dial_timeout() -> Duration {
    Duration::from_secs(10)
}

impl AgentConfig {
    /// Checks the parts serde cannot, and fills in what was left out.
    pub fn validate(&mut self) -> Result<(), ConfigError> {
        self.p2p.validate()?;
        self.kcp.validate()?;
        if let Some(ntp) = &self.ntp {
            ntp.validate()?;
        }
        self.network.validate()?;
        if matches!(self.transport, TransportKind::Quic | TransportKind::Kcp)
            && self.network.proxy.is_some()
        {
            return Err(ConfigError::invalid(
                "an upstream TCP proxy cannot carry a QUIC/KCP bridge; choose tcp, tls, ws or wss",
            ));
        }
        let geo = crate::geodata::GeoResolver::new(&self.geo)?;
        self.remote.validate(&geo)?;
        if self.visitors.len() > 128 {
            return Err(ConfigError::invalid(
                "at most 128 visitor listeners are supported",
            ));
        }
        let mut names = std::collections::HashSet::new();
        let mut binds = std::collections::HashSet::new();
        for visitor in &self.visitors {
            visitor.validate(&geo)?;
            if !names.insert(visitor.name.clone())
                || !binds.insert((
                    visitor.mode == super::VisitorMode::Udp,
                    super::parse_bind(&visitor.bind)?,
                ))
            {
                return Err(ConfigError::invalid(
                    "visitor names and listening endpoints must be unique",
                ));
            }
        }
        if (self.remote.enabled || self.p2p.enabled || !self.visitors.is_empty())
            && (!self.transport.is_encrypted() || self.trust == TrustPolicy::Insecure)
        {
            return Err(ConfigError::invalid(
                "remote configuration, P2P and visitors require verified TLS, WSS or QUIC",
            ));
        }
        self.mux.validate()?;
        if self.server.trim().is_empty() {
            return Err(ConfigError::invalid("`server` is required"));
        }
        if !self.server.contains(':') {
            return Err(ConfigError::invalid(Message::with(
                "`server` needs a port: {server}",
                [Value::debug("server", &self.server)],
            )));
        }
        super::validate_endpoint(&self.server)?;
        self.trust.validate()?;
        if self.vkey.is_empty() {
            // An empty key would authenticate against an empty key, which is to
            // say against anyone who guesses that.
            return Err(ConfigError::invalid("`vkey` must not be empty"));
        }

        if self.key_id.is_none() {
            // A hostname is a better default than a random id: it survives
            // restarts, so the dashboard keeps showing the same agent.
            self.key_id = Some(
                std::env::var("HOSTNAME")
                    .ok()
                    .filter(|h| !h.trim().is_empty())
                    .unwrap_or_else(|| "felucca-agent".to_owned()),
            );
        }

        if self.reconnect_max < self.reconnect_min {
            return Err(ConfigError::invalid(
                "`reconnect_max` is below `reconnect_min`",
            ));
        }
        if self.reconnect_min.is_zero() {
            // Reconnecting with no delay against a server that is down is a busy
            // loop that shows up as a CPU spike on both machines.
            return Err(ConfigError::invalid(
                "`reconnect_min` must be greater than zero",
            ));
        }

        if self.dial_timeout.is_zero() || self.max_connections == Some(0) {
            return Err(ConfigError::invalid(
                "dial_timeout and max_connections must be positive",
            ));
        }
        normalize_tunnels(&mut self.tunnels, &mut self.mappings, &geo, false)?;
        Ok(())
    }

    /// Loads and validates an agent config.
    pub fn from_file(path: &std::path::Path) -> Result<Self, ConfigError> {
        let mut config: Self = super::load(path)?;
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{Backend, HealthCheck, HealthCheckKind, LoadBalance, TunnelMode};
    #[test]
    fn transparent_routes_require_kernel_entrypoints_and_no_proxy_auth() {
        let base =
            "server='localhost:14028'\nvkey='key'\n[[tunnel]]\nmode='transparent'\nport=15080\n";
        let mut config: AgentConfig = toml::from_str(base).unwrap();
        config.validate().unwrap();
        assert_eq!(config.tunnels[0].mode, TunnelMode::Transparent);
        for extra in [
            "public=false",
            "backend=[{addr='127.0.0.1:8080'}]",
            "[tunnel.options.auth.users]\nalice='password'",
            "[tunnel.options.proxy_accept]\nmode='required'",
        ] {
            let mut config: AgentConfig = toml::from_str(&format!("{base}{extra}")).unwrap();
            assert!(config.validate().is_err(), "{extra}");
        }
        let mut config: AgentConfig =
            toml::from_str(&format!("{base}[tunnel.options]\ntransparent_tproxy=true")).unwrap();
        config.validate().unwrap();
        config.tunnels[0].mode = TunnelMode::Socks5;
        assert!(config.validate().is_err());
    }

    use super::*;

    #[test]
    fn explicit_tunnel_ids_survive_reordering_and_automatic_ids_avoid_collisions() {
        let text = r#"
server = "localhost:14028"
vkey = "secret"
[[tunnel]]
id = 2
mode = "tcp"
port = 9000
backend = [{addr = "127.0.0.1:10000"}]
[[tunnel]]
mode = "tcp"
port = 9001
backend = [{addr = "127.0.0.1:10001"}]
"#;
        let mut config: AgentConfig = toml::from_str(text).unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.tunnels.iter().map(|t| t.id).collect::<Vec<_>>(),
            [Some(2), Some(1)]
        );
        config.tunnels.swap(0, 1);
        config.validate().unwrap();
        assert_eq!(config.tunnels[0].id, Some(1));
        config.tunnels[0].id = Some(2);
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("unique")
        );
        config.tunnels[0].id = Some(0);
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("nonzero")
        );
    }

    /// The smallest agent config that should load.
    const MINIMAL: &str = r#"
server = "bridge.example.com:14028"
vkey = "a-shared-secret"
"#;

    fn parse(text: &str) -> Result<AgentConfig, ConfigError> {
        let mut c: AgentConfig =
            toml::from_str(text).map_err(|e| ConfigError::invalid(e.to_string()))?;
        c.validate()?;
        Ok(c)
    }

    #[test]
    fn a_minimal_config_loads_with_usable_defaults() {
        let c = parse(MINIMAL).expect("should load");
        assert_eq!(c.server, "bridge.example.com:14028");
        assert!(
            c.transport.is_encrypted(),
            "the default transport must be encrypted"
        );
        assert_eq!(c.trust, TrustPolicy::Webpki);
        assert!(c.key_id.is_some(), "a key id should be filled in");
        assert!(c.tunnels.is_empty());
    }

    #[test]
    fn a_vkey_never_appears_in_debug_output() {
        // The leak that matters: someone logs the whole config at startup.
        let c = parse(MINIMAL).unwrap();
        let shown = format!("{c:?}");
        assert!(
            !shown.contains("a-shared-secret"),
            "leaked the vkey: {shown}"
        );
    }

    #[test]
    fn the_essentials_are_required() {
        assert!(parse("vkey = \"k\"").is_err(), "server is required");
        assert!(parse("server = \"h:1\"").is_err(), "vkey is required");
        assert!(
            parse("server = \"\"\nvkey = \"k\"").is_err(),
            "an empty server is not a server"
        );
        assert!(
            parse("server = \"h:1\"\nvkey = \"\"").is_err(),
            "an empty vkey authenticates anyone who guesses it"
        );
    }

    #[test]
    fn a_server_without_a_port_is_refused() {
        let err = parse("server = \"bridge.example.com\"\nvkey = \"k\"").unwrap_err();
        assert!(err.to_string().contains("port"), "{err}");
    }

    #[test]
    fn a_zero_reconnect_delay_is_refused() {
        // It would busy-loop against a server that is down.
        let err = parse(&format!("{MINIMAL}\nreconnect_min = \"0s\"")).unwrap_err();
        assert!(err.to_string().contains("reconnect_min"), "{err}");
    }

    #[test]
    fn an_inverted_backoff_range_is_refused() {
        let err = parse(&format!(
            "{MINIMAL}\nreconnect_min = \"30s\"\nreconnect_max = \"5s\""
        ))
        .unwrap_err();
        assert!(err.to_string().contains("reconnect_max"), "{err}");
    }

    #[test]
    fn a_tcp_tunnel_needs_a_backend() {
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "tcp"
port = 8080
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("backend"), "{err}");
    }

    #[test]
    fn a_tcp_tunnel_with_a_backend_loads() {
        let c = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "tcp"
port = 8080
remark = "ssh"

[[tunnel.backend]]
addr = "127.0.0.1:22"
"#
        ))
        .expect("should load");

        assert_eq!(c.tunnels.len(), 1);
        let t = &c.tunnels[0];
        assert_eq!(t.mode, TunnelMode::Tcp);
        assert_eq!(t.port, 8080);
        assert_eq!(t.backends, vec![Backend::new("127.0.0.1:22")]);
        assert_eq!(t.balance, LoadBalance::RoundRobin);
    }

    #[test]
    fn several_weighted_backends_load() {
        let c = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "tcp"
port = 8080
balance = "weighted"

[[tunnel.backend]]
addr = "10.0.0.1:80"
weight = 3

[[tunnel.backend]]
addr = "10.0.0.2:80"
weight = 1
"#
        ))
        .expect("should load");

        let t = &c.tunnels[0];
        assert_eq!(t.balance, LoadBalance::Weighted);
        assert_eq!(t.backends.len(), 2);
        assert_eq!(t.backends[0].weight, 3);
        assert_eq!(t.backends[1].weight, 1);
    }

    #[test]
    fn a_backend_without_a_port_is_refused() {
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "tcp"
port = 8080

[[tunnel.backend]]
addr = "127.0.0.1"
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("port"), "{err}");
    }

    #[test]
    fn a_zero_weight_backend_is_refused() {
        // It would never be selected, which reads as a typo rather than intent.
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "tcp"
port = 8080

[[tunnel.backend]]
addr = "127.0.0.1:22"
weight = 0
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("weight"), "{err}");
    }

    #[test]
    fn a_socks5_tunnel_needs_no_backend_and_rejects_one() {
        assert!(
            parse(&format!(
                "{MINIMAL}\n[[tunnel]]\nmode = \"socks5\"\nport = 1080\n"
            ))
            .is_ok(),
            "socks5 picks its own target"
        );

        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "socks5"
port = 1080

[[tunnel.backend]]
addr = "127.0.0.1:22"
"#
        ))
        .unwrap_err();
        assert!(
            err.to_string().contains("ignored"),
            "a silently ignored backend is worse: {err}"
        );
    }

    #[test]
    fn an_http_tunnel_needs_a_host() {
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "http"
port = 8080

[[tunnel.backend]]
addr = "127.0.0.1:3000"
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("host"), "{err}");
    }

    #[test]
    fn hosts_on_a_non_http_tunnel_are_refused() {
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "tcp"
port = 8080
hosts = ["example.com"]

[[tunnel.backend]]
addr = "127.0.0.1:22"
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("hosts"), "{err}");
    }

    #[test]
    fn two_tunnels_on_one_port_are_refused() {
        // Which one won would depend on start order.
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "socks5"
port = 1080

[[tunnel]]
mode = "httpproxy"
port = 1080
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("1080"), "{err}");
    }

    #[test]
    fn port_zero_is_refused() {
        let err = parse(&format!(
            "{MINIMAL}\n[[tunnel]]\nmode = \"socks5\"\nport = 0\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("port 0"), "{err}");
    }

    #[test]
    fn a_bind_host_carrying_a_port_fails_at_load() {
        let err = parse(&format!(
            "{MINIMAL}\n[[tunnel]]\nmode = \"socks5\"\nport = 1080\nbind = \"127.0.0.1:9999\"\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("port"), "{err}");
    }

    #[test]
    fn a_bare_bind_host_loads() {
        assert!(
            parse(&format!(
                "{MINIMAL}\n[[tunnel]]\nmode = \"socks5\"\nport = 1080\nbind = \"127.0.0.1\"\n"
            ))
            .is_ok()
        );
    }

    #[test]
    fn a_bad_acl_rule_fails_at_load_not_on_first_connection() {
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "socks5"
port = 1080
acl_mode = "allow"
acl_rules = "not-an-ip"
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("not-an-ip"), "{err}");
    }

    #[test]
    fn an_allow_acl_with_no_rules_is_refused() {
        // It would reject every connection, which is never what a blank field means.
        assert!(
            parse(&format!(
                "{MINIMAL}\n[[tunnel]]\nmode = \"socks5\"\nport = 1080\nacl_mode = \"allow\"\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn a_health_check_loads_with_defaults() {
        let c = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "http"
port = 8080
hosts = ["example.com"]

[[tunnel.backend]]
addr = "127.0.0.1:3000"

[tunnel.health]
type = "http"
"#
        ))
        .expect("should load");

        let h = c.tunnels[0].health.as_ref().unwrap();
        assert_eq!(h.kind, HealthCheckKind::Http);
        assert_eq!(h.path, "/");
        assert!(
            h.failures_to_evict > 1,
            "one dropped packet must not evict a backend"
        );
        assert!(h.timeout < h.interval);
    }

    #[test]
    fn a_health_timeout_at_or_above_its_interval_is_refused() {
        // Overlapping probes pile onto a slow backend instead of marking it down.
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "tcp"
port = 8080

[[tunnel.backend]]
addr = "127.0.0.1:22"

[tunnel.health]
type = "tcp"
interval = "5s"
timeout = "5s"
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("timeout"), "{err}");
    }

    #[test]
    fn health_checking_a_udp_tunnel_is_refused() {
        // A TCP or HTTP probe says nothing about a UDP backend.
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "udp"
port = 5353

[[tunnel.backend]]
addr = "1.1.1.1:53"

[tunnel.health]
type = "tcp"
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("udp"), "{err}");
    }

    #[test]
    fn a_relative_health_path_is_refused() {
        let err = parse(&format!(
            r#"{MINIMAL}
[[tunnel]]
mode = "tcp"
port = 8080

[[tunnel.backend]]
addr = "127.0.0.1:22"

[tunnel.health]
type = "http"
path = "healthz"
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("healthz"), "{err}");
    }

    #[test]
    fn status_acceptance_defaults_to_2xx() {
        let h = HealthCheck {
            kind: HealthCheckKind::Http,
            ..Default::default()
        };
        assert!(h.accepts(200));
        assert!(h.accepts(204));
        assert!(!h.accepts(301));
        assert!(!h.accepts(500));
    }

    #[test]
    fn an_explicit_status_list_replaces_the_default() {
        let h = HealthCheck {
            kind: HealthCheckKind::Http,
            expect_status: vec![204, 301],
            ..Default::default()
        };
        assert!(h.accepts(204));
        assert!(h.accepts(301));
        assert!(!h.accepts(200), "an explicit list is exhaustive");
    }

    #[test]
    fn a_pinned_trust_policy_loads() {
        let c = parse(&format!(
            "{MINIMAL}\ntransport = \"tls\"\n[trust]\nmode = \"pinned\"\nfingerprint = \"{}\"\n",
            "ab".repeat(32)
        ))
        .expect("should load");
        assert!(matches!(c.trust, TrustPolicy::Pinned { .. }));
    }

    #[test]
    fn durations_are_written_the_way_people_write_them() {
        let c = parse(&format!(
            "{MINIMAL}\ndial_timeout = \"2s\"\nreconnect_max = \"5m\"\n"
        ))
        .expect("should load");
        assert_eq!(c.dial_timeout, Duration::from_secs(2));
        assert_eq!(c.reconnect_max, Duration::from_secs(300));
    }

    #[test]
    fn a_misspelled_key_is_an_error_rather_than_a_silent_default() {
        // Tolerating this would mean `reconect_max = "5m"` leaves the ceiling at
        // its 60s default while the operator believes they raised it.
        let err = toml::from_str::<AgentConfig>(&format!("{MINIMAL}\nreconect_max = \"5m\"\n"))
            .expect_err("a typo must not look like it worked");
        assert!(err.to_string().contains("reconect_max"), "{err}");
    }

    #[test]
    fn a_misspelled_key_inside_a_tunnel_is_also_caught() {
        let err = toml::from_str::<AgentConfig>(&format!(
            "{MINIMAL}\n[[tunnel]]\nmode = \"socks5\"\nprot = 1080\n"
        ))
        .expect_err("a nested typo must be caught too");
        assert!(err.to_string().contains("prot"), "{err}");
    }
}
