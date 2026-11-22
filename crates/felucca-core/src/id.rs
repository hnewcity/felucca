//! Opaque identifiers for the persisted objects.
//!
//! The Go original threads bare `int` ids through every layer, which makes a
//! client id and a tunnel id interchangeable at the type level. These newtypes
//! make that mix-up a compile error instead.

use std::fmt;

macro_rules! define_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl $name {
            pub const fn new(raw: u32) -> Self {
                Self(raw)
            }

            pub const fn get(self) -> u32 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<u32> for $name {
            fn from(raw: u32) -> Self {
                Self(raw)
            }
        }
    };
}

define_id!(
    /// A registered agent, keyed by its verify key at handshake time.
    ClientId
);
define_id!(
    /// A port-forwarding or proxy tunnel owned by a client.
    TunnelId
);
define_id!(
    /// A virtual host entry on the HTTP/HTTPS reverse proxy.
    HostId
);
define_id!(
    /// A dashboard user owning clients, tunnels and hosts.
    UserId
);

/// Identifies one multiplexed stream on one tunnel connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamId(pub u32);

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
