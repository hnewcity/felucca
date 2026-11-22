//! WebSocket transport.
//!
//! Exists for one reason: a WebSocket looks like ordinary HTTP traffic, so it
//! traverses corporate proxies and CDNs that would drop a bare TCP tunnel. Over
//! `wss` it is also indistinguishable from normal HTTPS.
//!
//! WebSocket is message-oriented and `felucca-mux` wants a byte stream, so
//! [`WsConn`] adapts between them: each write becomes one binary message, and
//! reads splice message payloads back into a continuous stream. Message
//! boundaries carry no meaning here — mux does its own framing — so a reader may
//! see bytes from one write split across several reads, or several writes
//! coalesced into one.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf, Bytes};
use futures_util::{Sink, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::error::TransportError;

/// Largest payload we put in one WebSocket message.
///
/// Matches the mux write loop's batch size, so a full batch is one message.
const MAX_PAYLOAD: usize = 64 * 1024;

/// Configuration for a tunnel WebSocket.
///
/// tungstenite defaults to a 64 MiB message limit. Every message here is at most
/// one mux batch, so accepting anything near that only lets a hostile peer name a
/// large allocation. 1 MiB leaves generous headroom over [`MAX_PAYLOAD`].
fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(1024 * 1024))
        .max_frame_size(Some(1024 * 1024))
}

/// A WebSocket presented as a byte stream.
#[derive(Debug)]
pub struct WsConn<S> {
    inner: WebSocketStream<S>,
    /// Payload from a message the caller has not fully read yet.
    pending: Bytes,
    /// Set once the peer has closed or the stream ended.
    finished: bool,
}

impl<S> WsConn<S> {
    fn new(inner: WebSocketStream<S>) -> Self {
        Self {
            inner,
            pending: Bytes::new(),
            finished: false,
        }
    }
}

impl<S> WsConn<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Continue after an HTTP server has already completed a 101 upgrade.
    pub async fn upgraded(io: S) -> Self {
        Self::new(
            WebSocketStream::from_raw_socket(
                io,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                Some(config()),
            )
            .await,
        )
    }
    /// Completes the server side of a WebSocket handshake.
    pub async fn accept(io: S) -> Result<Self, TransportError> {
        let ws = tokio_tungstenite::accept_async_with_config(io, Some(config()))
            .await
            .map_err(|e| TransportError::tls(format!("websocket handshake failed: {e}")))?;
        Ok(Self::new(ws))
    }

    /// Enforces the configured path before upgrading the bridge connection.
    #[allow(clippy::result_large_err)] // The callback signature belongs to tungstenite.
    pub async fn accept_at(io: S, path: &str) -> Result<Self, TransportError> {
        use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
        let ws = tokio_tungstenite::accept_hdr_async_with_config(
            io,
            |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
                if request.uri().path() == path {
                    Ok(response)
                } else {
                    Err(Response::builder()
                        .status(404)
                        .body(Some("unknown bridge path".into()))
                        .unwrap())
                }
            },
            Some(config()),
        )
        .await
        .map_err(|e| TransportError::tls(e.to_string()))?;
        Ok(Self::new(ws))
    }

    /// Completes the client side of a WebSocket handshake against `url`.
    pub async fn connect(url: &str, io: S) -> Result<Self, TransportError> {
        let (ws, _response) = tokio_tungstenite::client_async_with_config(url, io, Some(config()))
            .await
            .map_err(|e| TransportError::tls(format!("websocket handshake failed: {e}")))?;
        Ok(Self::new(ws))
    }
}

pub fn accept_key(key: &[u8]) -> String {
    tokio_tungstenite::tungstenite::handshake::derive_accept_key(key)
}

impl<S> AsyncRead for WsConn<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            // Anything left from the last message goes first, so a message larger
            // than the caller's buffer is drained across several reads.
            if !this.pending.is_empty() {
                let n = this.pending.len().min(buf.remaining());
                buf.put_slice(&this.pending[..n]);
                this.pending.advance(n);
                return Poll::Ready(Ok(()));
            }
            if this.finished {
                // A zero-length read is how AsyncRead spells end-of-stream.
                return Poll::Ready(Ok(()));
            }

            match this.inner.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    this.finished = true;
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Some(Ok(msg))) => match msg {
                    Message::Binary(data) => {
                        // An empty message is not end-of-stream; keep reading.
                        if !data.is_empty() {
                            this.pending = data;
                        }
                    }
                    Message::Close(_) => {
                        this.finished = true;
                        return Poll::Ready(Ok(()));
                    }
                    // Ping and Pong are answered by tungstenite itself, and a
                    // tunnel never sends Text. None of them are payload, so
                    // skipping them is right — returning here instead would
                    // report a spurious EOF.
                    Message::Ping(_) | Message::Pong(_) | Message::Text(_) => {}
                    Message::Frame(_) => {}
                },
                Poll::Ready(Some(Err(e))) => {
                    this.finished = true;
                    return Poll::Ready(Err(ws_error(e)));
                }
            }
        }
    }
}

impl<S> AsyncWrite for WsConn<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        // The sink has to accept an item before one is handed over; this is where
        // backpressure from a slow peer reaches the caller.
        match Pin::new(&mut this.inner).poll_ready(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(ws_error(e))),
            Poll::Ready(Ok(())) => {}
        }

        let n = buf.len().min(MAX_PAYLOAD);
        let msg = Message::Binary(Bytes::copy_from_slice(&buf[..n]));
        match Pin::new(&mut this.inner).start_send(msg) {
            Ok(()) => Poll::Ready(Ok(n)),
            Err(e) => Poll::Ready(Err(ws_error(e))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_flush(cx).map_err(ws_error)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // Sends a Close frame, so the peer sees an orderly finish rather than a
        // truncated connection.
        Pin::new(&mut this.inner).poll_close(cx).map_err(ws_error)
    }
}

/// Maps a WebSocket failure onto an `io::Error`.
///
/// A peer vanishing becomes `UnexpectedEof` rather than a custom error, so
/// generic copy loops treat it as end-of-stream instead of a fault.
fn ws_error(e: tokio_tungstenite::tungstenite::Error) -> io::Error {
    use tokio_tungstenite::tungstenite::Error as E;
    match e {
        E::Io(e) => e,
        E::ConnectionClosed | E::AlreadyClosed => io::ErrorKind::UnexpectedEof.into(),
        E::Protocol(_) => io::Error::new(io::ErrorKind::InvalidData, e.to_string()),
        other => io::Error::other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A connected pair of WebSocket byte streams over an in-memory pipe.
    async fn pair() -> (
        WsConn<tokio::io::DuplexStream>,
        WsConn<tokio::io::DuplexStream>,
    ) {
        let (a, b) = tokio::io::duplex(256 * 1024);
        let server = tokio::spawn(async move { WsConn::accept(b).await });
        let client = WsConn::connect("ws://localhost/tunnel", a)
            .await
            .expect("client handshake");
        let server = server.await.unwrap().expect("server handshake");
        (client, server)
    }

    #[tokio::test]
    async fn bytes_round_trip_through_a_websocket() {
        let (mut client, mut server) = pair().await;

        client.write_all(b"hello over ws").await.unwrap();
        client.flush().await.unwrap();

        let mut got = [0u8; 13];
        server.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hello over ws");
    }

    #[tokio::test]
    async fn data_flows_in_both_directions() {
        let (mut client, mut server) = pair().await;

        client.write_all(b"ping").await.unwrap();
        client.flush().await.unwrap();
        let mut got = [0u8; 4];
        server.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");

        server.write_all(b"pong").await.unwrap();
        server.flush().await.unwrap();
        let mut back = [0u8; 4];
        client.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"pong");
    }

    #[tokio::test]
    async fn a_message_larger_than_the_readers_buffer_drains_across_reads() {
        let (mut client, mut server) = pair().await;

        client.write_all(&[0xab; 4096]).await.unwrap();
        client.flush().await.unwrap();

        // Read it back a few bytes at a time.
        let mut total = 0usize;
        let mut small = [0u8; 100];
        while total < 4096 {
            let n = server.read(&mut small).await.unwrap();
            assert_ne!(n, 0, "premature EOF at {total}");
            assert!(
                small[..n].iter().all(|b| *b == 0xab),
                "corrupted at {total}"
            );
            total += n;
        }
        assert_eq!(total, 4096);
    }

    #[tokio::test]
    async fn message_boundaries_do_not_survive_and_need_not() {
        // Three writes, one read: mux does its own framing, so coalescing is fine
        // and the byte order is all that matters.
        let (mut client, mut server) = pair().await;
        for part in [b"aaa".as_slice(), b"bbb", b"ccc"] {
            client.write_all(part).await.unwrap();
        }
        client.flush().await.unwrap();

        let mut got = [0u8; 9];
        server.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"aaabbbccc");
    }

    #[tokio::test]
    async fn a_bulk_transfer_arrives_intact() {
        // Larger than one message, so this exercises segmentation and reassembly.
        const LEN: usize = 512 * 1024;
        let (mut client, mut server) = pair().await;

        let reader = tokio::spawn(async move {
            let mut got = Vec::with_capacity(LEN);
            let mut buf = vec![0u8; 16 * 1024];
            while got.len() < LEN {
                let n = server.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            got
        });

        let payload: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        client.write_all(&payload).await.unwrap();
        client.flush().await.unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(10), reader)
            .await
            .expect("transfer should not stall")
            .unwrap();
        assert_eq!(got.len(), LEN);
        assert_eq!(got, payload, "payload corrupted in transit");
    }

    #[tokio::test]
    async fn a_shutdown_reads_as_a_clean_eof() {
        let (mut client, mut server) = pair().await;
        client.write_all(b"last words").await.unwrap();
        client.shutdown().await.unwrap();

        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert_eq!(&got, b"last words", "buffered data must survive the close");
    }

    #[tokio::test]
    async fn a_dropped_peer_ends_the_read_rather_than_hanging() {
        let (client, mut server) = pair().await;
        drop(client);

        let mut got = Vec::new();
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            server.read_to_end(&mut got),
        )
        .await;
        assert!(r.is_ok(), "a vanished peer must not hang the reader");
    }

    #[tokio::test]
    async fn a_zero_length_write_is_a_no_op() {
        let (mut client, mut server) = pair().await;
        assert_eq!(client.write(b"").await.unwrap(), 0);
        client.write_all(b"real").await.unwrap();
        client.flush().await.unwrap();

        // The empty write must not have been sent as a message that reads as EOF.
        let mut got = [0u8; 4];
        server.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"real");
    }

    #[tokio::test]
    async fn a_mux_session_runs_over_a_websocket() {
        // The real proof: the adapter is good enough for what it exists to carry.
        use felucca_mux::{Config, Role, Session};

        let (client_io, server_io) = pair().await;
        let client = Session::new(client_io, Config::default(), Role::Client);
        let mut server = Session::new(server_io, Config::default(), Role::Server);

        let echo = tokio::spawn(async move {
            let mut s = server.accept().await.unwrap();
            let mut buf = vec![0u8; 11];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
            s.flush().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });

        let mut stream = client.open().unwrap();
        stream.write_all(b"mux over ws").await.unwrap();
        stream.flush().await.unwrap();

        let mut back = vec![0u8; 11];
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            stream.read_exact(&mut back),
        )
        .await
        .expect("mux over ws should not stall")
        .unwrap();
        assert_eq!(&back, b"mux over ws");
        echo.await.unwrap();
    }
}
