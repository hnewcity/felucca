//! Server failures.

use felucca_core::ConfigError;
use felucca_transport::TransportError;

/// Something went wrong running the server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Transport(#[from] TransportError),

    /// A tunnel asked for a port the server will not serve.
    #[error("port {port} is not in the allowed range")]
    PortNotAllowed { port: u16 },

    /// Two tunnels want the same port.
    #[error("port {port} is already served by another tunnel")]
    PortInUse { port: u16 },

    /// The named client is not connected.
    #[error("client {client} is not connected")]
    ClientOffline { client: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
