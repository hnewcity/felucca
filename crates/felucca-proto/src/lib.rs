//! The felucca control protocol: framing, messages and the handshake.

pub mod codec;
pub mod handshake;
pub mod message;
pub mod session;
pub mod visitor;

pub use codec::{CodecError, MAX_FRAME_LEN, read_frame, write_frame};
pub use handshake::{
    Direction, HandshakeError, Transcript, build_auth, build_server_proof, compute_mac, verify_mac,
    verify_server_proof,
};
pub use message::{
    Auth, AuthReject, AuthResult, ConfigRejection, ConnectionRole, Control, Hello,
    MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, ServerHello, StreamReject, StreamRequest,
    StreamResponse, TunnelMode, TunnelSpec,
};
pub use session::{AcceptPolicy, Authenticated, HandshakeFailure, accept, connect};

pub mod datagram;
pub mod diagnostics;

pub mod peer;
