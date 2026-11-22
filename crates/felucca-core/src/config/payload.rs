use serde::{Deserialize, Serialize};

/// Extra processing of a route's payload, independent of bridge TLS/QUIC.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadOptions {
    #[serde(default)]
    pub compress: bool,
    #[serde(default)]
    pub encrypt: bool,
}
impl PayloadOptions {
    pub fn enabled(self) -> bool {
        self.compress || self.encrypt
    }
}
