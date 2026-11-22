use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KcpConfig {
    pub data_shards: u8,
    pub parity_shards: u8,
    pub mtu: u16,
    pub send_window: u16,
    pub receive_window: u16,
    pub interval_ms: u16,
    pub fast_resend: u8,
    pub nodelay: bool,
    pub congestion_control: bool,
    #[serde(with = "humantime_serde")]
    pub session_timeout: Duration,
    /// Includes connections still completing bridge authentication.
    pub max_sessions: usize,
}
impl Default for KcpConfig {
    fn default() -> Self {
        Self {
            data_shards: 10,
            parity_shards: 3,
            mtu: 1200,
            send_window: 256,
            receive_window: 256,
            interval_ms: 20,
            fast_resend: 2,
            nodelay: true,
            congestion_control: true,
            session_timeout: Duration::from_secs(90),
            max_sessions: 128,
        }
    }
}
impl KcpConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if (self.data_shards != 0 || self.parity_shards != 0)
            && (!(1..=32).contains(&self.data_shards) || !(1..=16).contains(&self.parity_shards))
        {
            return Err(ConfigError::invalid(
                "KCP FEC needs data_shards 1..32 and parity_shards 1..16, or both zero to disable",
            ));
        }
        if !(576..=1472).contains(&self.mtu)
            || !(16..=4096).contains(&self.send_window)
            || !(128..=4096).contains(&self.receive_window)
            || !(10..=1000).contains(&self.interval_ms)
            || self.fast_resend > 3
            || !(Duration::from_secs(1)..=Duration::from_secs(86400))
                .contains(&self.session_timeout)
            || !(1..=4096).contains(&self.max_sessions)
        {
            return Err(ConfigError::invalid(
                "KCP requires mtu 576..1472, send_window 16..4096, receive_window 128..4096, interval_ms 10..1000, fast_resend 0..3, session_timeout 1s..24h, max_sessions 1..4096",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_kcp_windows_packets_timers_and_session_counts() {
        assert!(KcpConfig::default().validate().is_ok());
        for text in [
            "mtu=1500",
            "send_window=0",
            "receive_window=16",
            "interval_ms=0",
            "fast_resend=4",
            "session_timeout='0s'",
            "max_sessions=0",
            "data_shards=0",
            "parity_shards=0",
            "data_shards=33",
            "parity_shards=17",
        ] {
            let config: KcpConfig = toml::from_str(text).unwrap();
            assert!(config.validate().is_err(), "{text}");
        }
        assert!(toml::from_str::<KcpConfig>("interval=10").is_err());
        assert!(
            toml::from_str::<KcpConfig>("data_shards=0\nparity_shards=0")
                .unwrap()
                .validate()
                .is_ok()
        );
    }
}
