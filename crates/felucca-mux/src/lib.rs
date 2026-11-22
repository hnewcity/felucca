//! A credit-based stream multiplexer.
//!
//! Runs over anything reliable, ordered and duplex — TCP, TLS, a WebSocket, a
//! QUIC stream — and turns it into many independent streams, each with its own
//! flow control so one busy stream cannot starve the rest.

mod budget;
pub mod config;
pub mod credit;
pub mod error;
pub mod frame;
pub mod rtt;
pub mod session;
pub mod stream;

pub use config::Config;
pub use credit::{CreditViolation, RecvWindow, SendCredit};
pub use error::MuxError;
pub use frame::{CloseReason, FrameHeader, FrameType, HEADER_LEN, MAX_SEGMENT};
pub use rtt::RttTracker;
pub use session::{Role, Session, SessionHandle};
pub use stream::Stream;
