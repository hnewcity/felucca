//! Management listener, identities and authorization.
use crate::{ConfigError, Secret};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    /// Permit administrators to collect a bounded process CPU profile.
    #[serde(default)]
    pub profiling: bool,
    pub bind: String,
    pub state_path: PathBuf,
    #[serde(default)]
    pub tls: Option<super::TlsConfig>,
    /// Browser origin for a non-loopback listener: `https://` behind a TLS
    /// terminator, or `http://` when the operator accepts plaintext management.
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default = "session_ttl", with = "humantime_serde")]
    pub session_ttl: Duration,
    #[serde(default = "max_sessions")]
    pub max_sessions: usize,
    #[serde(default, rename = "account")]
    pub accounts: BTreeMap<String, AdminAccount>,
}
fn session_ttl() -> Duration {
    Duration::from_secs(12 * 3600)
}
fn max_sessions() -> usize {
    4096
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminRole {
    Admin,
    Operator,
    Viewer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAccount {
    pub role: AdminRole,
    #[serde(default)]
    pub user: Option<String>,
    /// pbkdf2-sha256$iterations$salt-hex$derived-key-hex
    pub password_hash: Secret,
    /// SHA-256 of an optional bearer token; no plaintext token is stored.
    #[serde(default)]
    pub token_hash: Option<Secret>,
    #[serde(default)]
    pub disabled: bool,
}

/// Splits a configured origin into (is_plaintext, authority).
pub fn split_origin(origin: &str) -> Option<(bool, &str)> {
    if let Some(rest) = origin.strip_prefix("https://") {
        Some((false, rest))
    } else {
        origin.strip_prefix("http://").map(|rest| (true, rest))
    }
}

impl AdminConfig {
    /// Whether the browser reaches management over TLS.
    pub fn secure(&self) -> bool {
        match self.origin.as_deref().and_then(split_origin) {
            Some((plain, _)) => !plain,
            None => self.tls.is_some(),
        }
    }

    pub fn validate(
        &self,
        users: &std::collections::HashMap<String, super::UserEntry>,
    ) -> Result<(), ConfigError> {
        if self.profiling && !cfg!(any(target_os = "linux", target_os = "macos")) {
            return Err(ConfigError::invalid(
                "CPU profiling is supported on Linux and macOS",
            ));
        }
        let bind = super::parse_bind(&self.bind)?;
        if self.state_path.as_os_str().is_empty() || bind.port() == 0 {
            return Err(ConfigError::invalid(
                "admin requires a nonempty state_path and a fixed listen port",
            ));
        }
        if !bind.ip().is_loopback() && self.origin.is_none() {
            return Err(ConfigError::invalid(
                "a public admin listener requires an explicit browser origin",
            ));
        }
        if let Some(origin) = &self.origin {
            let (plain, url) = split_origin(origin).ok_or_else(|| {
                ConfigError::invalid("admin.origin requires an http or https origin")
            })?;
            if plain && self.tls.is_some() {
                return Err(ConfigError::invalid(
                    "admin.origin must be https when admin.tls is set",
                ));
            }
            if url.is_empty()
                || (!url.ends_with(']')
                    && url
                        .rsplit_once(':')
                        .is_some_and(|(_, port)| !port.parse::<u16>().is_ok_and(|port| port != 0)))
                || url.parse::<http::uri::Authority>().is_err()
                || url
                    .parse::<http::uri::Authority>()
                    .is_ok_and(|a| a.port().is_some_and(|p| p.as_u16() == 0))
                || url
                    .bytes()
                    .any(|b| b <= 32 || b == 127 || b"/\\@?#".contains(&b))
            {
                return Err(ConfigError::invalid(
                    "admin.origin must contain only scheme and authority",
                ));
            }
        }
        if self.session_ttl < Duration::from_secs(60)
            || self.session_ttl > Duration::from_secs(7 * 86400)
            || !(1..=16384).contains(&self.max_sessions)
        {
            return Err(ConfigError::invalid(
                "admin session_ttl must be 1m..7d and max_sessions 1..16384",
            ));
        }
        if self.accounts.len() > 512
            || !self
                .accounts
                .values()
                .any(|a| !a.disabled && a.role == AdminRole::Admin)
        {
            return Err(ConfigError::invalid(
                "admin requires at least one enabled administrator and at most 512 accounts",
            ));
        }
        for (name, account) in &self.accounts {
            if !super::local::valid_name(name) {
                return Err(ConfigError::invalid("invalid admin account name"));
            }
            account.validate(users)?;
        }
        Ok(())
    }
}
impl AdminAccount {
    fn validate(
        &self,
        users: &std::collections::HashMap<String, super::UserEntry>,
    ) -> Result<(), ConfigError> {
        match self.role {
            AdminRole::Admin if self.user.is_some() => {
                return Err(ConfigError::invalid(
                    "administrators cannot have a tenant restriction",
                ));
            }
            AdminRole::Operator | AdminRole::Viewer
                if self
                    .user
                    .as_ref()
                    .is_none_or(|user| !users.contains_key(user)) =>
            {
                return Err(ConfigError::invalid(
                    "operator/viewer accounts require an existing user",
                ));
            }
            _ => {}
        }
        let parts: Vec<_> = self.password_hash.expose().split('$').collect();
        if parts.len() != 4
            || parts[0] != "pbkdf2-sha256"
            || !parts[1].parse::<u32>().is_ok_and(|n| n == 600_000)
            || !hex(parts[2], 32)
            || !hex(parts[3], 64)
        {
            return Err(ConfigError::invalid(
                "invalid admin password hash; use felucca-server --hash-password",
            ));
        }
        if self
            .token_hash
            .as_ref()
            .is_some_and(|hash| !hex(hash.expose(), 64))
        {
            return Err(ConfigError::invalid(
                "admin bearer token hashes must contain 64 SHA-256 hex digits",
            ));
        }
        Ok(())
    }
}
fn hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}
