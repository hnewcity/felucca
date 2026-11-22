//! Frame layout for the multiplexer.
//!
//! Every frame is a fixed 9-byte header optionally followed by a payload:
//!
//! ```text
//! +------+---------------------+---------------------+
//! | type |  stream id (u32 BE) |    value (u32 BE)   |
//! +------+---------------------+---------------------+
//!    1              4                     4
//! ```
//!
//! `value` is reused per frame type — a payload length, a credit grant, a ping
//! nonce or a close reason — which keeps the header one fixed size for every
//! frame and lets a reader decode it without branching first.
//!
//! The Go original packs a 13-byte header with a little-endian 16-bit length,
//! capping a segment at 4085 bytes so that a frame fits one 4 KiB pool buffer
//! (`lib/mux/pool.go`). felucca uses a 32-bit length field and a 16 KiB segment:
//! four times fewer frames for the same bulk transfer, which matters more on
//! today's links than fitting a 4 KiB allocation.

use crate::error::MuxError;

/// Size of a frame header, in bytes.
pub const HEADER_LEN: usize = 9;

/// Largest payload one [`FrameType::Data`] frame may carry, in bytes.
pub const MAX_SEGMENT: usize = 16 * 1024;
/// Absolute per-stream protocol ceiling. Session budgets may be smaller.
pub const MAX_STREAM_WINDOW: u32 = 64 * 1024 * 1024;

/// What a frame does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// Stream payload. `value` is the payload length.
    Data = 1,
    /// Grant of receive credit. `value` is the number of bytes added.
    WindowUpdate = 2,
    /// Open a stream; `value` advertises the sender's receive window.
    Open = 3,
    /// The peer refused an [`FrameType::Open`]. `value` is a [`CloseReason`].
    ///
    OpenNack = 4,
    /// Accepted open, advertising the receiver's initial window.
    /// The opener can send one segment optimistically before this arrives.
    OpenAck = 5,
    /// This side is done writing but will keep reading. `value` is unused.
    CloseWrite = 6,
    /// The stream is finished in both directions. `value` is a [`CloseReason`].
    Close = 7,
    /// Liveness probe. `value` is a nonce echoed in the [`FrameType::Pong`].
    Ping = 8,
    /// Answer to a [`FrameType::Ping`], echoing its nonce.
    Pong = 9,
    /// The whole session is closing. `value` is a [`CloseReason`].
    GoAway = 10,
    /// Additional receive window, separate from returning consumed credit.
    WindowGrow = 11,
}

impl FrameType {
    fn from_u8(raw: u8) -> Option<Self> {
        Some(match raw {
            1 => Self::Data,
            2 => Self::WindowUpdate,
            3 => Self::Open,
            4 => Self::OpenNack,
            5 => Self::OpenAck,
            6 => Self::CloseWrite,
            7 => Self::Close,
            8 => Self::Ping,
            9 => Self::Pong,
            10 => Self::GoAway,
            11 => Self::WindowGrow,
            _ => return None,
        })
    }

    /// Whether frames of this type carry a payload after the header.
    ///
    /// Only `Data` does. Everything else puts what it needs in `value`, so a
    /// reader knows from the type alone whether more bytes follow.
    pub fn has_payload(self) -> bool {
        matches!(self, Self::Data)
    }

    /// Whether this frame concerns one stream rather than the whole session.
    pub fn is_stream_scoped(self) -> bool {
        !matches!(self, Self::Ping | Self::Pong | Self::GoAway)
    }
}

/// Why a stream or session closed.
///
/// Carried as a code rather than a string so a close frame stays fixed-size;
/// the Go original logs a reason locally but cannot tell the peer which it was
/// (`lib/mux/reason.go`), which makes a remote-side diagnosis guesswork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CloseReason {
    /// Both sides finished; nothing went wrong.
    Normal = 0,
    /// The local endpoint dropped the stream without draining it.
    Cancelled = 1,
    /// The target refused the connection.
    ConnectionRefused = 2,
    /// The target did not respond in time.
    Timeout = 3,
    /// The peer sent something that violates the protocol.
    ProtocolError = 4,
    /// The session is over its stream limit.
    TooManyStreams = 5,
    /// The underlying transport failed.
    TransportFailure = 6,
    /// The endpoint is shutting down.
    Shutdown = 7,
    /// Anything not covered above.
    Internal = 8,
}

impl CloseReason {
    fn from_u32(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Normal,
            1 => Self::Cancelled,
            2 => Self::ConnectionRefused,
            3 => Self::Timeout,
            4 => Self::ProtocolError,
            5 => Self::TooManyStreams,
            6 => Self::TransportFailure,
            7 => Self::Shutdown,
            8 => Self::Internal,
            _ => return None,
        })
    }

    /// Decodes a reason, falling back to [`CloseReason::Internal`].
    ///
    /// An unrecognised reason must not fail the session: a newer peer may name a
    /// reason this build predates, and "closed for a reason I don't know" is
    /// still a close.
    pub fn from_u32_lossy(raw: u32) -> Self {
        Self::from_u32(raw).unwrap_or(Self::Internal)
    }

    /// Whether this represents an orderly finish rather than a fault.
    pub fn is_graceful(self) -> bool {
        matches!(self, Self::Normal | Self::Shutdown)
    }
}

/// A decoded frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub ty: FrameType,
    /// Which stream this concerns; zero for session-scoped frames.
    pub stream: u32,
    /// Payload length, credit, nonce or reason code, per [`FrameType`].
    pub value: u32,
}

impl FrameHeader {
    pub fn new(ty: FrameType, stream: u32, value: u32) -> Self {
        Self { ty, stream, value }
    }

    /// A `Data` header for a payload of `len` bytes.
    pub fn data(stream: u32, len: usize) -> Self {
        debug_assert!(len <= MAX_SEGMENT, "caller must segment before framing");
        Self::new(FrameType::Data, stream, len as u32)
    }

    /// A credit grant of `credit` bytes.
    pub fn window_update(stream: u32, credit: u32) -> Self {
        Self::new(FrameType::WindowUpdate, stream, credit)
    }

    pub fn close(stream: u32, reason: CloseReason) -> Self {
        Self::new(FrameType::Close, stream, reason as u32)
    }

    pub fn close_write(stream: u32) -> Self {
        Self::new(FrameType::CloseWrite, stream, 0)
    }

    pub fn open(stream: u32) -> Self {
        Self::open_with_window(stream, MAX_SEGMENT as u32)
    }
    pub fn open_with_window(stream: u32, window: u32) -> Self {
        Self::new(FrameType::Open, stream, window)
    }
    pub fn open_ack(stream: u32, window: u32) -> Self {
        Self::new(FrameType::OpenAck, stream, window)
    }
    pub fn window_grow(stream: u32, additional: u32) -> Self {
        Self::new(FrameType::WindowGrow, stream, additional)
    }

    pub fn open_nack(stream: u32, reason: CloseReason) -> Self {
        Self::new(FrameType::OpenNack, stream, reason as u32)
    }

    pub fn ping(nonce: u32) -> Self {
        Self::new(FrameType::Ping, 0, nonce)
    }

    pub fn pong(nonce: u32) -> Self {
        Self::new(FrameType::Pong, 0, nonce)
    }

    pub fn go_away(reason: CloseReason) -> Self {
        Self::new(FrameType::GoAway, 0, reason as u32)
    }

    /// How many payload bytes follow this header.
    pub fn payload_len(&self) -> usize {
        if self.ty.has_payload() {
            self.value as usize
        } else {
            0
        }
    }

    /// Reads `value` as a close reason.
    pub fn reason(&self) -> CloseReason {
        CloseReason::from_u32_lossy(self.value)
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0] = self.ty as u8;
        out[1..5].copy_from_slice(&self.stream.to_be_bytes());
        out[5..9].copy_from_slice(&self.value.to_be_bytes());
        out
    }

    /// Decodes a header and rejects anything a well-behaved peer cannot send.
    ///
    /// Validation happens here rather than in the session loop so a malformed
    /// frame can never reach the stream bookkeeping, where an absurd length or a
    /// stream-scoped frame with id zero would be far more awkward to unwind.
    pub fn decode(buf: &[u8; HEADER_LEN]) -> Result<Self, MuxError> {
        let ty = FrameType::from_u8(buf[0]).ok_or(MuxError::Protocol("unknown frame type"))?;
        let stream = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
        let value = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]);

        if ty.is_stream_scoped() && stream == 0 {
            return Err(MuxError::Protocol("stream-scoped frame with stream id 0"));
        }
        if !ty.is_stream_scoped() && stream != 0 {
            return Err(MuxError::Protocol("session-scoped frame with a stream id"));
        }
        if ty == FrameType::Data && value as usize > MAX_SEGMENT {
            return Err(MuxError::Protocol("data frame exceeds the segment limit"));
        }
        if matches!(ty, FrameType::WindowUpdate | FrameType::WindowGrow) && value == 0 {
            // A zero grant is either a bug or an attempt to spin the sender.
            return Err(MuxError::Protocol("window update grants no credit"));
        }
        if matches!(ty, FrameType::Open | FrameType::OpenAck)
            && !(MAX_SEGMENT as u32..=MAX_STREAM_WINDOW).contains(&value)
        {
            return Err(MuxError::Protocol("invalid initial stream window"));
        }
        Ok(Self { ty, stream, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(h: FrameHeader) -> FrameHeader {
        FrameHeader::decode(&h.encode()).expect("should decode")
    }

    #[test]
    fn every_frame_type_round_trips() {
        let cases = [
            FrameHeader::data(1, 1234),
            FrameHeader::window_update(2, 64 * 1024),
            FrameHeader::open(3),
            FrameHeader::open_nack(5, CloseReason::TooManyStreams),
            FrameHeader::close_write(6),
            FrameHeader::close(7, CloseReason::Normal),
            FrameHeader::ping(0xdead_beef),
            FrameHeader::pong(0xdead_beef),
            FrameHeader::go_away(CloseReason::Shutdown),
        ];
        for h in cases {
            assert_eq!(round_trip(h), h, "{h:?}");
        }
    }

    #[test]
    fn a_header_is_exactly_nine_bytes() {
        assert_eq!(FrameHeader::ping(1).encode().len(), HEADER_LEN);
    }

    #[test]
    fn only_data_frames_carry_a_payload() {
        assert_eq!(FrameHeader::data(1, 500).payload_len(), 500);
        // A window update's value is credit, not a length to read off the wire.
        assert_eq!(FrameHeader::window_update(1, 500).payload_len(), 0);
        assert_eq!(FrameHeader::ping(500).payload_len(), 0);
    }

    #[test]
    fn the_maximum_segment_is_accepted_and_one_more_is_not() {
        let ok = FrameHeader::new(FrameType::Data, 1, MAX_SEGMENT as u32);
        assert!(FrameHeader::decode(&ok.encode()).is_ok());

        let too_big = FrameHeader::new(FrameType::Data, 1, MAX_SEGMENT as u32 + 1);
        assert!(FrameHeader::decode(&too_big.encode()).is_err());
    }

    #[test]
    fn an_unknown_frame_type_is_rejected() {
        let mut raw = FrameHeader::ping(1).encode();
        raw[0] = 200;
        assert!(FrameHeader::decode(&raw).is_err());
    }

    #[test]
    fn stream_scope_is_enforced_in_both_directions() {
        // A data frame must name a stream.
        let mut raw = FrameHeader::data(1, 10).encode();
        raw[1..5].copy_from_slice(&0u32.to_be_bytes());
        assert!(FrameHeader::decode(&raw).is_err());

        // A ping must not.
        let mut raw = FrameHeader::ping(1).encode();
        raw[1..5].copy_from_slice(&7u32.to_be_bytes());
        assert!(FrameHeader::decode(&raw).is_err());
    }

    #[test]
    fn a_zero_credit_window_update_is_rejected() {
        let raw = FrameHeader::new(FrameType::WindowUpdate, 1, 0).encode();
        assert!(FrameHeader::decode(&raw).is_err());
    }

    #[test]
    fn an_empty_data_frame_is_allowed() {
        // Zero-length writes are legal and must not be mistaken for a close.
        let h = FrameHeader::data(1, 0);
        assert_eq!(round_trip(h).payload_len(), 0);
    }

    #[test]
    fn a_close_reason_survives_the_round_trip() {
        let h = FrameHeader::close(1, CloseReason::ConnectionRefused);
        assert_eq!(round_trip(h).reason(), CloseReason::ConnectionRefused);
    }

    #[test]
    fn an_unknown_close_reason_degrades_instead_of_failing() {
        // A newer peer may name a reason this build predates.
        let raw = FrameHeader::new(FrameType::Close, 1, 9999).encode();
        let h = FrameHeader::decode(&raw).expect("must still decode");
        assert_eq!(h.reason(), CloseReason::Internal);
    }

    #[test]
    fn graceful_reasons_are_distinguished_from_faults() {
        assert!(CloseReason::Normal.is_graceful());
        assert!(CloseReason::Shutdown.is_graceful());
        assert!(!CloseReason::ProtocolError.is_graceful());
        assert!(!CloseReason::TransportFailure.is_graceful());
    }

    #[test]
    fn the_largest_stream_id_is_usable() {
        let h = FrameHeader::data(u32::MAX, 1);
        assert_eq!(round_trip(h).stream, u32::MAX);
    }
}
