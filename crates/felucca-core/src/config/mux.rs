use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MuxConfig {
    pub initial_stream_window: u32,
    pub max_stream_window: u32,
    pub max_session_window: u64,
    pub max_send_queue: usize,
    pub max_control_queue: usize,
    pub max_streams: usize,
    pub accept_backlog: usize,
    #[serde(with = "humantime_serde")]
    pub open_timeout: Duration,
}
impl Default for MuxConfig {
    fn default() -> Self {
        Self {
            initial_stream_window: 256 * 1024,
            max_stream_window: 8 * 1024 * 1024,
            max_session_window: 256 * 1024 * 1024,
            max_send_queue: 8 * 1024 * 1024,
            max_control_queue: 8192,
            max_streams: 1024,
            accept_backlog: 256,
            open_timeout: Duration::from_secs(30),
        }
    }
}
impl MuxConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(16384..=64 * 1024 * 1024).contains(&self.initial_stream_window)
            || !(self.initial_stream_window..=64 * 1024 * 1024).contains(&self.max_stream_window)
            || self.max_session_window < u64::from(self.max_stream_window)
            || !(16384 + 9..=256 * 1024 * 1024).contains(&self.max_send_queue)
            || !(16..=65536).contains(&self.max_control_queue)
            || self.max_streams == 0
            || self.accept_backlog == 0
            || self.open_timeout.is_zero()
        {
            return Err(ConfigError::invalid(
                "inconsistent mux windows, queue limits, stream limits or open_timeout",
            ));
        }
        Ok(())
    }
}
