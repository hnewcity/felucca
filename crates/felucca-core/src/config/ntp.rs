//! Optional application clock calibration; never changes the OS clock.
use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtpConfig {
    /// An explicit trusted time source, with UDP port (normally 123).
    pub server: String,
    #[serde(default = "interval", with = "humantime_serde")]
    pub interval: Duration,
    #[serde(default = "timeout", with = "humantime_serde")]
    pub timeout: Duration,
    #[serde(default = "max_offset", with = "humantime_serde")]
    pub max_offset: Duration,
}
fn interval() -> Duration {
    Duration::from_secs(300)
}
fn timeout() -> Duration {
    Duration::from_secs(5)
}
fn max_offset() -> Duration {
    Duration::from_secs(86400)
}
impl NtpConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        super::validate_endpoint(&self.server)?;
        if !(Duration::from_secs(1)..=Duration::from_secs(86400)).contains(&self.interval)
            || self.timeout.is_zero()
            || self.timeout > Duration::from_secs(30)
            || self.timeout >= self.interval
            || self.max_offset.is_zero()
            || self.max_offset > Duration::from_secs(7 * 86400)
        {
            return Err(ConfigError::invalid(
                "ntp requires interval 1s..24h, timeout >0 and <=30s below interval, and max_offset >0 and <=7days",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_timing_and_endpoint_limits() {
        let config: NtpConfig = toml::from_str("server='127.0.0.1:123'").unwrap();
        assert!(config.validate().is_ok());
        for suffix in [
            "interval='0s'",
            "timeout='0s'",
            "timeout='31s'",
            "timeout='5m'",
            "max_offset='0s'",
            "max_offset='8days'",
        ] {
            let invalid: NtpConfig =
                toml::from_str(&format!("server='127.0.0.1:123'\n{suffix}")).unwrap();
            assert!(invalid.validate().is_err(), "{suffix}");
        }
        assert!(
            toml::from_str::<NtpConfig>("server='localhost:0'")
                .unwrap()
                .validate()
                .is_err()
        );
        assert!(toml::from_str::<NtpConfig>("server='localhost:123'\ntimout='1s'").is_err());
    }
}
