//! External management platforms and durable event delivery.
use crate::{ConfigError, Secret, TrustPolicy};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::IpAddr, path::PathBuf, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    pub id: String,
    pub state_path: PathBuf,
    #[serde(default = "history")]
    pub changes_window: usize,
    #[serde(default, rename = "platform")]
    pub platforms: BTreeMap<String, PlatformConfig>,
    #[serde(default, rename = "webhook")]
    pub webhooks: BTreeMap<String, WebhookConfig>,
    #[serde(default, with = "humantime_serde::option")]
    pub usage_interval: Option<Duration>,
    #[serde(default, with = "humantime_serde::option")]
    pub traffic_report_interval: Option<Duration>,
    #[serde(default)]
    pub traffic_report_step_bytes: u64,
}
fn history() -> usize {
    1024
}
fn enabled() -> bool {
    true
}
fn heartbeat() -> Duration {
    Duration::from_secs(15)
}
fn timeout() -> Duration {
    Duration::from_secs(5)
}
fn retry() -> u32 {
    2
}
fn backoff() -> Duration {
    Duration::from_secs(2)
}
fn queue() -> usize {
    100
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectMode {
    #[default]
    Direct,
    Reverse,
    Dual,
}
impl ConnectMode {
    pub fn direct(self) -> bool {
        self != Self::Reverse
    }
    pub fn reverse(self) -> bool {
        self != Self::Direct
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformScope {
    Full,
    #[default]
    Account,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformConfig {
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub token: Secret,
    #[serde(default)]
    pub scope: PlatformScope,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub mode: ConnectMode,
    #[serde(default)]
    pub reverse_url: Option<String>,
    #[serde(default)]
    pub trust: TrustPolicy,
    #[serde(default = "heartbeat", with = "humantime_serde")]
    pub heartbeat: Duration,
    #[serde(default)]
    pub callback: Option<CallbackConfig>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// Absent is a full-node webhook; present limits it to one tenant.
    #[serde(default)]
    pub user: Option<String>,
    pub delivery: CallbackConfig,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackConfig {
    #[serde(default)]
    pub selector: super::event::EventSelector,
    #[serde(default)]
    pub content: super::event::EventContent,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub url: String,
    pub signing_key: Secret,
    #[serde(default = "timeout", with = "humantime_serde")]
    pub timeout: Duration,
    #[serde(default = "retry")]
    pub retry_max: u32,
    #[serde(default = "backoff", with = "humantime_serde")]
    pub retry_backoff: Duration,
    #[serde(default = "queue")]
    pub queue_max: usize,
}

/// Plaintext is confined to literal loopback fixtures; public control is TLS.
pub fn endpoint(value: &str, websocket: bool) -> Result<http::Uri, ConfigError> {
    let uri: http::Uri = value
        .parse()
        .map_err(|_| ConfigError::invalid("invalid node endpoint URL"))?;
    let secure = if websocket { "wss" } else { "https" };
    let plain = if websocket { "ws" } else { "http" };
    let authority = uri
        .authority()
        .ok_or_else(|| ConfigError::invalid("node endpoint needs an authority"))?;
    let host = authority.host().trim_matches(['[', ']']);
    let loopback = host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if value.len() > 4096
        || value.contains('#')
        || authority.as_str().contains('@')
        || !(uri.scheme_str() == Some(secure) || uri.scheme_str() == Some(plain) && loopback)
        || authority.port().is_some_and(|p| p.as_u16() == 0)
    {
        return Err(ConfigError::invalid(
            "node endpoints require HTTPS/WSS, no userinfo or fragment; plaintext is only allowed on literal loopback",
        ));
    }
    Ok(uri)
}
fn valid_secret(secret: &Secret) -> bool {
    (16..=4096).contains(&secret.len())
}
impl CallbackConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.selector.validate()?;
        self.content.validate()?;
        endpoint(&self.url, false)?;
        if !valid_secret(&self.signing_key)
            || self.timeout < Duration::from_millis(100)
            || self.timeout > Duration::from_secs(60)
            || self.retry_max > 8
            || self.retry_backoff < Duration::from_millis(100)
            || self.retry_backoff > Duration::from_secs(300)
            || self.queue_max > 4096
        {
            return Err(ConfigError::invalid(
                "callback requires a 16..4096-byte signing key, timeout 100ms..60s, retry 0..8, backoff 100ms..5min and queue 0..4096",
            ));
        }
        Ok(())
    }
}
impl NodeConfig {
    pub fn validate(
        &self,
        users: &std::collections::HashMap<String, super::UserEntry>,
    ) -> Result<(), ConfigError> {
        if !super::local::valid_name(&self.id)
            || self.state_path.as_os_str().is_empty()
            || !(100..=4096).contains(&self.changes_window)
            || self.platforms.len() > 16
            || self.webhooks.len() > 64
            || self
                .usage_interval
                .is_some_and(|d| d < Duration::from_secs(1) || d > Duration::from_secs(3600))
            || self
                .traffic_report_interval
                .is_some_and(|d| d < Duration::from_millis(100) || d > Duration::from_secs(3600))
        {
            return Err(ConfigError::invalid(
                "node requires id/state_path, changes_window 100..4096, <=16 platforms, <=64 webhooks and usage interval 1s..1h",
            ));
        }
        let mut tokens = std::collections::HashSet::new();
        for (id, platform) in &self.platforms {
            if !tokens.insert(platform.token.expose()) {
                return Err(ConfigError::invalid("platform tokens must be unique"));
            }
            if !super::local::valid_name(id)
                || id.len() > 64
                || !valid_secret(&platform.token)
                || !platform
                    .token
                    .expose()
                    .bytes()
                    .all(|b| b.is_ascii_graphic())
                || platform.heartbeat < Duration::from_secs(1)
                || platform.heartbeat > Duration::from_secs(300)
            {
                return Err(ConfigError::invalid(
                    "invalid platform name, token or heartbeat",
                ));
            }
            match (platform.scope, platform.user.as_ref()) {
                (PlatformScope::Full, None) => {}
                (PlatformScope::Account, Some(user)) if users.contains_key(user) => {}
                _ => {
                    return Err(ConfigError::invalid(
                        "full platforms have no user; account platforms require an existing user",
                    ));
                }
            }
            if platform.mode.reverse() && platform.reverse_url.is_none() {
                return Err(ConfigError::invalid(
                    "reverse/dual platform requires reverse_url",
                ));
            }
            if let Some(url) = &platform.reverse_url {
                endpoint(url, true)?;
            }
            platform.trust.validate()?;
            if platform.trust == TrustPolicy::Insecure {
                return Err(ConfigError::invalid("platform TLS must verify its peer"));
            }
            if let Some(callback) = &platform.callback {
                callback.validate()?;
            }
        }
        for (id, webhook) in &self.webhooks {
            if !super::local::valid_name(id)
                || id.len() > 64
                || webhook
                    .user
                    .as_ref()
                    .is_some_and(|user| !users.contains_key(user))
            {
                return Err(ConfigError::invalid("invalid webhook id or tenant"));
            }
            webhook.delivery.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outbound_control_urls_reject_plain_public_userinfo_and_fragments() {
        for value in [
            "https://platform.example/hooks",
            "http://127.0.0.1:1234/hook",
            "http://[::1]:1234/",
        ] {
            endpoint(value, false).unwrap();
        }
        for value in [
            "http://platform.example/",
            "https://user:pass@platform.example/",
            "https://platform.example/#token",
            "file:///tmp/hook",
            "https://platform.example:0/",
            "localhost:443",
        ] {
            assert!(endpoint(value, false).is_err(), "{value}");
        }
        endpoint("wss://platform.example/reverse", true).unwrap();
        endpoint("ws://127.0.0.1:1234/reverse", true).unwrap();
        assert!(endpoint("ws://platform.example/reverse", true).is_err());
        assert!(endpoint("https://platform.example/reverse", true).is_err());
    }
    fn node() -> NodeConfig {
        toml::from_str("id='edge-a'\nstate_path='node.json'\n[platform.panel]\ntoken='platform-token-long-enough'\nscope='account'\nuser='team'").unwrap()
    }
    #[test]
    fn platforms_require_explicit_tenant_authority_and_unique_credentials() {
        let users = [("team".into(), super::super::UserEntry::default())]
            .into_iter()
            .collect();
        let mut config = node();
        config.validate(&users).unwrap();
        config.platforms.get_mut("panel").unwrap().user = None;
        assert!(config.validate(&users).is_err());
        config.platforms.get_mut("panel").unwrap().scope = PlatformScope::Full;
        config.validate(&users).unwrap();
        config
            .platforms
            .insert("duplicate".into(), config.platforms["panel"].clone());
        assert!(config.validate(&users).is_err());
    }
    #[test]
    fn reverse_channels_and_callback_budgets_validate_before_startup() {
        let users = [("team".into(), super::super::UserEntry::default())]
            .into_iter()
            .collect();
        let mut config = node();
        let platform = config.platforms.get_mut("panel").unwrap();
        platform.mode = ConnectMode::Reverse;
        assert!(config.validate(&users).is_err());
        config.platforms.get_mut("panel").unwrap().reverse_url =
            Some("wss://platform.example/control".into());
        config.validate(&users).unwrap();
        config.platforms.get_mut("panel").unwrap().trust = TrustPolicy::Insecure;
        assert!(config.validate(&users).is_err());
        let good = "url='https://platform.example/hook'\nsigning_key='a-long-callback-signing-key'";
        let callback: CallbackConfig = toml::from_str(good).unwrap();
        callback.validate().unwrap();
        for extra in [
            "\nretry_max=9",
            "\nqueue_max=4097",
            "\ntimeout='0s'",
            "\nretry_backoff='1ms'",
        ] {
            let config: CallbackConfig = toml::from_str(&format!("{good}{extra}")).unwrap();
            assert!(config.validate().is_err());
        }
        assert!(toml::from_str::<CallbackConfig>(&format!("{good}\nrety_max=1")).is_err());
    }
}
