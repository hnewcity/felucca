//! Tunnel transports.
//!
//! Everything here produces the same thing: a reliable, ordered, duplex byte
//! stream for `felucca-mux` to run over. What differs is how it gets there — a
//! bare TCP socket, a TLS session, a WebSocket, or a QUIC stream.

pub mod clock;
pub mod conn;
pub mod error;
pub mod kcp;
pub mod local;
pub mod network;
pub mod payload;
pub mod proxy_protocol;
pub mod quic;
pub mod sniff;
pub mod tcp;
pub mod tls;
pub mod tunnel;
pub mod udp;
pub mod ws;

pub use conn::{BoxedConn, Conn, PrefixedConn};
pub use error::TransportError;
pub use quic::QuicStream;
pub use sniff::{Protocol, sniff};
pub use tls::{Identity, Verification};
pub use tunnel::{Tunnel, TunnelHandle, TunnelStream};
pub use ws::WsConn;

/// Distinguishes bridge TLS from website TLS when sharing a listener.
pub const BRIDGE_ALPN: &[u8] = b"felucca/10";

#[cfg(test)]
#[path = "../tests/support/dns.rs"]
pub(crate) mod test_dns;

pub mod transparent;

pub mod peer;
pub mod rendezvous;
