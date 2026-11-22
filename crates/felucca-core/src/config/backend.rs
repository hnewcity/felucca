//! Backend pools and active health-check configuration.
use super::TrustPolicy;
use crate::ConfigError;
use felucca_i18n::{Message, Value};
use serde::{Deserialize, Serialize};
use std::time::Duration;
/// One place an agent can send a tunnel's traffic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backend {
    /// `host:port` on the agent's side.
    pub addr: String,

    /// Relative share of traffic. Equal weights spread evenly.
    #[serde(default = "default_weight")]
    pub weight: u32,

    /// Whether the agent should speak TLS to this backend, for `http` mode.
    #[serde(default)]
    pub tls: bool,
    /// Certificate verification for a TLS backend.
    #[serde(default)]
    pub trust: TrustPolicy,
}

fn default_weight() -> u32 {
    1
}

impl Backend {
    /// A backend with default weight and no TLS.
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            weight: default_weight(),
            tls: false,
            trust: TrustPolicy::default(),
        }
    }
}

/// How to spread traffic across backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalance {
    /// Each connection to the next healthy backend in turn.
    #[default]
    RoundRobin,
    /// Round robin, but proportional to weight.
    Weighted,
    /// The backend with the fewest live connections. Better when connections are
    /// long-lived and unevenly expensive, where round robin drifts.
    LeastConnections,
    /// Consistent by client address, so one client keeps landing on one backend.
    SourceHash,
    /// The first healthy backend, in configured order. The rest are standby.
    Failover,
}

/// What kind of probe decides a backend is alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthCheckKind {
    /// No probing; every backend is assumed up.
    #[default]
    None,
    /// A TCP connection that is opened and closed.
    Tcp,
    /// An HTTP request expecting an acceptable status.
    Http,
}

/// Active health checking for a tunnel's backends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    #[serde(default, rename = "type")]
    pub kind: HealthCheckKind,

    #[serde(default = "default_health_interval", with = "humantime_serde")]
    pub interval: Duration,

    #[serde(default = "default_health_timeout", with = "humantime_serde")]
    pub timeout: Duration,

    /// Consecutive failures before a backend is taken out of rotation.
    ///
    /// Above one so a single dropped packet does not evict a healthy backend.
    #[serde(default = "default_health_failures")]
    pub failures_to_evict: u32,

    /// Consecutive successes before a recovered backend is used again.
    #[serde(default = "default_health_successes")]
    pub successes_to_restore: u32,

    /// Path to request, for an HTTP check.
    #[serde(default = "default_health_path")]
    pub path: String,

    /// Status codes counted as healthy. Empty means any 2xx.
    #[serde(default)]
    pub expect_status: Vec<u16>,
}

fn default_health_interval() -> Duration {
    Duration::from_secs(10)
}

fn default_health_timeout() -> Duration {
    Duration::from_secs(5)
}

fn default_health_failures() -> u32 {
    3
}

fn default_health_successes() -> u32 {
    2
}

fn default_health_path() -> String {
    "/".to_owned()
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            kind: HealthCheckKind::default(),
            interval: default_health_interval(),
            timeout: default_health_timeout(),
            failures_to_evict: default_health_failures(),
            successes_to_restore: default_health_successes(),
            path: default_health_path(),
            expect_status: Vec::new(),
        }
    }
}

impl HealthCheck {
    pub(super) fn validate(&mut self) -> Result<(), ConfigError> {
        if self.kind == HealthCheckKind::None {
            return Ok(());
        }
        if self.interval.is_zero() {
            return Err(ConfigError::invalid(
                "a health check interval of zero is a busy loop",
            ));
        }
        if self.timeout.is_zero() || self.timeout >= self.interval {
            // Overlapping probes pile up: each one is still running when the next
            // starts, so a slow backend gets hammered instead of marked down.
            return Err(ConfigError::invalid(
                "a health check timeout must be shorter than its interval",
            ));
        }
        if self.failures_to_evict == 0 {
            return Err(ConfigError::invalid(
                "`failures_to_evict` must be at least 1",
            ));
        }
        if self.successes_to_restore == 0 {
            return Err(ConfigError::invalid(
                "`successes_to_restore` must be at least 1",
            ));
        }
        if self.kind == HealthCheckKind::Http
            && (!self.path.starts_with('/') || self.path.bytes().any(|c| c <= 32 || c == 127))
        {
            return Err(ConfigError::invalid(Message::with(
                "a health check path must start with '/': {path}",
                [Value::debug("path", &self.path)],
            )));
        }
        Ok(())
    }

    /// Whether `status` counts as healthy.
    pub fn accepts(&self, status: u16) -> bool {
        if self.expect_status.is_empty() {
            (200..300).contains(&status)
        } else {
            self.expect_status.contains(&status)
        }
    }
}
