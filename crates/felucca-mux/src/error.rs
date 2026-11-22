//! Failures from the multiplexer.

use crate::frame::CloseReason;

/// Something went wrong on a session or one of its streams.
#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    /// The peer violated the framing rules. The session cannot continue: once a
    /// frame boundary is in doubt, every later frame is too.
    #[error("protocol error: {0}")]
    Protocol(&'static str),

    /// This stream is finished. `reason` says why, including whether it was
    /// orderly.
    #[error("stream closed: {reason:?}")]
    StreamClosed { reason: CloseReason },

    /// The whole session is finished; no further streams can be opened.
    #[error("session closed: {reason:?}")]
    SessionClosed { reason: CloseReason },

    /// The peer refused an open request.
    #[error("peer refused the stream: {reason:?}")]
    OpenRejected { reason: CloseReason },

    /// An operation did not complete within its deadline.
    #[error("timed out")]
    Timeout,

    /// The underlying transport failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl MuxError {
    /// Whether this is an orderly shutdown rather than a fault worth logging
    /// loudly. A peer finishing a stream is the common case, not an incident.
    pub fn is_graceful(&self) -> bool {
        match self {
            Self::StreamClosed { reason } | Self::SessionClosed { reason } => reason.is_graceful(),
            Self::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
            ),
            _ => false,
        }
    }

    /// The reason to put in a close frame when this error ends a stream.
    pub fn close_reason(&self) -> CloseReason {
        match self {
            Self::Protocol(_) => CloseReason::ProtocolError,
            Self::StreamClosed { reason }
            | Self::SessionClosed { reason }
            | Self::OpenRejected { reason } => *reason,
            Self::Timeout => CloseReason::Timeout,
            Self::Io(_) => CloseReason::TransportFailure,
        }
    }
}

impl From<MuxError> for std::io::Error {
    /// Streams implement [`tokio::io::AsyncRead`] and [`tokio::io::AsyncWrite`],
    /// whose contracts are in terms of `io::Error`, so mux failures have to map
    /// onto kinds a generic caller already handles. An orderly close becomes
    /// `UnexpectedEof` rather than a custom error, so `copy_bidirectional` and
    /// friends treat it as end-of-stream instead of a fault.
    fn from(e: MuxError) -> Self {
        use std::io::ErrorKind;
        match e {
            MuxError::Io(e) => e,
            MuxError::Timeout => ErrorKind::TimedOut.into(),
            MuxError::Protocol(msg) => Self::new(ErrorKind::InvalidData, msg),
            ref other if other.is_graceful() => ErrorKind::UnexpectedEof.into(),
            other => Self::new(ErrorKind::ConnectionAborted, other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graceful_closes_are_distinguished_from_faults() {
        assert!(
            MuxError::StreamClosed {
                reason: CloseReason::Normal
            }
            .is_graceful()
        );
        assert!(
            MuxError::SessionClosed {
                reason: CloseReason::Shutdown
            }
            .is_graceful()
        );
        assert!(!MuxError::Protocol("bad").is_graceful());
        assert!(
            !MuxError::StreamClosed {
                reason: CloseReason::ProtocolError
            }
            .is_graceful()
        );
    }

    #[test]
    fn a_reset_transport_reads_as_graceful() {
        let e = MuxError::Io(std::io::ErrorKind::ConnectionReset.into());
        assert!(e.is_graceful(), "a peer vanishing mid-copy is routine");
    }

    #[test]
    fn an_orderly_close_maps_to_eof_for_generic_callers() {
        // copy_bidirectional must see end-of-stream, not an error.
        let io: std::io::Error = MuxError::StreamClosed {
            reason: CloseReason::Normal,
        }
        .into();
        assert_eq!(io.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_protocol_error_maps_to_invalid_data() {
        let io: std::io::Error = MuxError::Protocol("bad frame").into();
        assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
        assert!(io.to_string().contains("bad frame"));
    }

    #[test]
    fn a_fault_close_does_not_look_like_eof() {
        let io: std::io::Error = MuxError::StreamClosed {
            reason: CloseReason::ConnectionRefused,
        }
        .into();
        assert_eq!(io.kind(), std::io::ErrorKind::ConnectionAborted);
    }

    #[test]
    fn an_io_error_passes_through_unchanged() {
        let io: std::io::Error = MuxError::Io(std::io::ErrorKind::PermissionDenied.into()).into();
        assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn errors_carry_the_reason_to_send_in_a_close_frame() {
        assert_eq!(
            MuxError::Protocol("x").close_reason(),
            CloseReason::ProtocolError
        );
        assert_eq!(MuxError::Timeout.close_reason(), CloseReason::Timeout);
        assert_eq!(
            MuxError::Io(std::io::ErrorKind::BrokenPipe.into()).close_reason(),
            CloseReason::TransportFailure
        );
        assert_eq!(
            MuxError::OpenRejected {
                reason: CloseReason::TooManyStreams
            }
            .close_reason(),
            CloseReason::TooManyStreams
        );
    }
}
