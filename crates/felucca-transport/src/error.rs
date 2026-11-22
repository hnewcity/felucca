//! Transport failures.

/// Something went wrong establishing or accepting a tunnel connection.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("cannot bind {addr}")]
    Bind {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot reach {addr}")]
    Connect {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    /// The peer spoke something this listener does not serve.
    #[error("unrecognised protocol on an accepted connection")]
    UnknownProtocol,

    /// TLS setup or handshake failed.
    #[error("tls error: {0}")]
    Tls(String),

    /// A certificate or key file could not be used.
    #[error("certificate problem: {0}")]
    Certificate(String),

    /// The peer took too long at a stage with a deadline.
    #[error("timed out")]
    Timeout,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl TransportError {
    /// Whether this is a routine disconnect rather than a fault worth alerting
    /// on. A server logging every half-open scan at warn level drowns its own
    /// signal.
    pub fn is_routine(&self) -> bool {
        match self {
            Self::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
            ),
            Self::UnknownProtocol | Self::Timeout => true,
            _ => false,
        }
    }

    pub fn tls(msg: impl std::fmt::Display) -> Self {
        Self::Tls(msg.to_string())
    }

    pub fn certificate(msg: impl std::fmt::Display) -> Self {
        Self::Certificate(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routine_disconnects_are_distinguished_from_faults() {
        assert!(TransportError::Io(std::io::ErrorKind::ConnectionReset.into()).is_routine());
        assert!(TransportError::UnknownProtocol.is_routine());
        assert!(!TransportError::tls("handshake failed").is_routine());
        assert!(
            !TransportError::Bind {
                addr: "0.0.0.0:80".into(),
                source: std::io::ErrorKind::AddrInUse.into()
            }
            .is_routine(),
            "a bind failure needs an operator's attention"
        );
    }
}
