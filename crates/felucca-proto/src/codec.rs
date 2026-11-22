//! Length-delimited framing for control messages.
//!
//! Control messages are small and infrequent, so each is written as a 4-byte
//! big-endian length followed by its postcard encoding. Bulk proxied traffic
//! never passes through here — it rides a mux stream as raw bytes.
//!
//! Every read is bounded by [`MAX_FRAME_LEN`]. The length prefix arrives from
//! the network before we have authenticated anything, so an unbounded read here
//! would let a peer name a 4 GiB allocation.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest control message we will send or accept, in bytes.
///
/// A config push from an agent carrying many tunnel definitions is the biggest
/// legitimate message; 1 MiB leaves generous room for that while keeping a
/// hostile length prefix cheap to reject.
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

/// Failure while reading or writing a control frame.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("connection closed while reading a frame")]
    Eof,

    #[error("frame of {len} bytes exceeds the {MAX_FRAME_LEN} byte limit")]
    FrameTooLarge { len: usize },

    #[error("malformed message body")]
    Decode(#[source] postcard::Error),

    #[error("cannot encode message")]
    Encode(#[source] postcard::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl CodecError {
    /// Whether this is an orderly shutdown rather than a fault.
    ///
    /// A peer closing between frames is normal; the caller logs it at debug and
    /// moves on rather than reporting a protocol error.
    pub fn is_clean_eof(&self) -> bool {
        match self {
            Self::Eof => true,
            Self::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
            ),
            _ => false,
        }
    }
}

/// Reads one length-delimited message.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, CodecError>
where
    R: AsyncRead + Unpin + ?Sized,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(CodecError::Eof),
        Err(e) => return Err(e.into()),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge { len });
    }

    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    postcard::from_bytes(&body).map_err(CodecError::Decode)
}

/// Writes one length-delimited message and flushes it.
///
/// Control messages are latency-sensitive and rare, so each one is flushed
/// rather than left in a buffer waiting for traffic that may never come.
pub async fn write_frame<W, T>(writer: &mut W, msg: &T) -> Result<(), CodecError>
where
    W: AsyncWrite + Unpin + ?Sized,
    T: Serialize + ?Sized,
{
    let body = postcard::to_allocvec(msg).map_err(CodecError::Encode)?;
    if body.len() > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge { len: body.len() });
    }

    // One write for the header and body together: two writes on a TCP socket
    // with nodelay set would put the length prefix in its own packet.
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    writer.write_all(&out).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Sample {
        n: u32,
        s: String,
    }

    #[tokio::test]
    async fn round_trips_a_message() {
        let msg = Sample {
            n: 7,
            s: "hello".into(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).await.unwrap();

        let got: Sample = read_frame(&mut buf.as_slice()).await.unwrap();
        assert_eq!(got, msg);
    }

    #[tokio::test]
    async fn round_trips_several_messages_in_order() {
        let mut buf = Vec::new();
        for n in 0..5u32 {
            write_frame(
                &mut buf,
                &Sample {
                    n,
                    s: n.to_string(),
                },
            )
            .await
            .unwrap();
        }

        let mut cursor = buf.as_slice();
        for n in 0..5u32 {
            let got: Sample = read_frame(&mut cursor).await.unwrap();
            assert_eq!(got.n, n);
        }
        // ...and then a clean end of stream.
        let err = read_frame::<_, Sample>(&mut cursor).await.unwrap_err();
        assert!(err.is_clean_eof(), "{err}");
    }

    #[tokio::test]
    async fn an_empty_stream_reads_as_clean_eof() {
        let err = read_frame::<_, Sample>(&mut [].as_slice())
            .await
            .unwrap_err();
        assert!(matches!(err, CodecError::Eof));
        assert!(err.is_clean_eof());
    }

    #[tokio::test]
    async fn an_oversized_length_prefix_is_rejected_without_allocating() {
        // A 4 GiB claim, with no body behind it.
        let mut framed = u32::MAX.to_be_bytes().to_vec();
        framed.extend_from_slice(b"nope");

        let err = read_frame::<_, Sample>(&mut framed.as_slice())
            .await
            .unwrap_err();
        assert!(matches!(err, CodecError::FrameTooLarge { .. }), "{err}");
    }

    #[tokio::test]
    async fn a_truncated_body_is_an_error_not_a_hang() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &Sample {
                n: 1,
                s: "abcdef".into(),
            },
        )
        .await
        .unwrap();
        buf.truncate(buf.len() - 3);

        let err = read_frame::<_, Sample>(&mut buf.as_slice())
            .await
            .unwrap_err();
        assert!(!matches!(err, CodecError::FrameTooLarge { .. }), "{err}");
    }

    #[tokio::test]
    async fn a_garbage_body_is_a_decode_error() {
        // Length says 4, body is not a valid Sample encoding.
        let mut framed = 4u32.to_be_bytes().to_vec();
        framed.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);

        let err = read_frame::<_, Sample>(&mut framed.as_slice())
            .await
            .unwrap_err();
        assert!(matches!(err, CodecError::Decode(_)), "{err}");
    }

    #[tokio::test]
    async fn a_frame_is_written_as_a_single_syscall() {
        // Guards the coalescing above: a length prefix in its own packet costs a
        // round trip on a nodelay socket.
        struct CountingWriter {
            writes: usize,
            buf: Vec<u8>,
        }
        impl AsyncWrite for CountingWriter {
            fn poll_write(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                self.writes += 1;
                self.buf.extend_from_slice(buf);
                std::task::Poll::Ready(Ok(buf.len()))
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }

        let mut w = CountingWriter {
            writes: 0,
            buf: Vec::new(),
        };
        write_frame(
            &mut w,
            &Sample {
                n: 1,
                s: "x".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(w.writes, 1, "header and body should go out together");
    }
}
