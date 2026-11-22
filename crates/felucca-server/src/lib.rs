//! Server runtime, management and forwarding for Felucca.
pub mod accounting;
pub mod admin;
mod certificates;
pub mod control;
mod http_support;
mod platform;

pub mod daemon;
pub mod error;
mod peer;
pub mod proxy;
pub mod registry;
pub mod runtime;
mod visitor;

pub use copy::{Direction, splice};
pub use daemon::Daemon;
pub use error::ServerError;
pub use forward::{Tunnel, start_tcp};
pub use proxy::{copy, forward};
pub use registry::{Client, ConnectionGuard, Registry};
pub use runtime::Server;
// Existing embedding paths remain available; implementation ownership is explicit above.
pub use accounting as resources;
use certificates::{acme, zerossl};
use proxy::{
    http, http_options, http_pool, http3, ingress, nested, relay, reverse, socks, tls_ingress, udp,
    websocket,
};
