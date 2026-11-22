//! Optional, bounded read-only diagnostics on an already authenticated bridge.
use serde::{Deserialize, Serialize};

pub const MAX_BACKENDS: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendHealth {
    pub addr: String,
    pub healthy: bool,
    pub connections: u64,
    pub last_probe_ms: u64,
    pub last_success: Option<bool>,
    pub failures: u32,
    pub successes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendReport {
    pub total: usize,
    pub probing: bool,
    pub backends: Vec<BackendHealth>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostics_does_not_renumber_existing_stream_requests() {
        use crate::StreamRequest;
        assert_eq!(
            postcard::to_allocvec(&StreamRequest::Tunnel { tunnel_id: 17 }).unwrap(),
            [0, 17]
        );
        assert_eq!(postcard::to_allocvec(&StreamRequest::Control).unwrap(), [5]);
        assert_eq!(
            postcard::to_allocvec(&StreamRequest::Diagnostics { tunnel_id: 17 }).unwrap(),
            [8, 17]
        );
    }
}
