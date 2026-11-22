//! The connection shape every transport produces.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf, Bytes};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// What the multiplexer needs from a transport: a reliable, ordered, duplex
/// byte stream.
///
/// Every transport reduces to this, which is why `felucca-mux` can run over TCP,
/// TLS, a WebSocket or a QUIC stream without knowing the difference.
pub trait Conn: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> Conn for T {}

/// A transport connection with its concrete type erased.
///
/// The four transports are known at compile time, but a listener's caller should
/// not have to name which one produced a connection.
pub type BoxedConn = Box<dyn Conn>;

/// Wraps a connection whose first bytes have already been read, replaying them
/// before anything from the socket.
///
/// This is what makes single-port protocol detection possible: the sniffer
/// consumes a prefix to decide what is speaking, then hands the connection on
/// with that prefix intact, so the real handler sees an unbroken stream.
#[derive(Debug)]
pub struct PrefixedConn<T> {
    prefix: Bytes,
    inner: T,
}

impl<T> PrefixedConn<T> {
    pub fn new(prefix: Bytes, inner: T) -> Self {
        Self { prefix, inner }
    }

    /// Whether any of the replayed prefix is still pending.
    pub fn prefix_pending(&self) -> bool {
        !self.prefix.is_empty()
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for PrefixedConn<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.prefix.is_empty() {
            let n = self.prefix.len().min(buf.remaining());
            buf.put_slice(&self.prefix[..n]);
            self.prefix.advance(n);
            // Return what the prefix supplied rather than also touching the
            // socket: mixing the two here could block on a read the caller did
            // not ask for.
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for PrefixedConn<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn the_prefix_is_replayed_before_the_socket() {
        let (mut a, b) = tokio::io::duplex(1024);
        a.write_all(b"world").await.unwrap();

        let mut c = PrefixedConn::new(Bytes::from_static(b"hello "), b);
        let mut got = vec![0u8; 11];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hello world");
    }

    #[tokio::test]
    async fn a_reader_smaller_than_the_prefix_drains_it_across_reads() {
        let (_a, b) = tokio::io::duplex(1024);
        let mut c = PrefixedConn::new(Bytes::from_static(b"abcdef"), b);

        let mut two = [0u8; 2];
        for want in [b"ab", b"cd", b"ef"] {
            c.read_exact(&mut two).await.unwrap();
            assert_eq!(&two, want);
        }
        assert!(!c.prefix_pending());
    }

    #[tokio::test]
    async fn an_empty_prefix_is_transparent() {
        let (mut a, b) = tokio::io::duplex(1024);
        a.write_all(b"direct").await.unwrap();

        let mut c = PrefixedConn::new(Bytes::new(), b);
        assert!(!c.prefix_pending());
        let mut got = vec![0u8; 6];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"direct");
    }

    #[tokio::test]
    async fn writes_pass_straight_through() {
        let (mut a, b) = tokio::io::duplex(1024);
        let mut c = PrefixedConn::new(Bytes::from_static(b"unread"), b);
        c.write_all(b"outbound").await.unwrap();
        c.flush().await.unwrap();

        let mut got = vec![0u8; 8];
        a.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"outbound", "a pending prefix must not affect writes");
    }

    #[tokio::test]
    async fn eof_after_the_prefix_is_reported() {
        let (a, b) = tokio::io::duplex(1024);
        drop(a);

        let mut c = PrefixedConn::new(Bytes::from_static(b"tail"), b);
        let mut got = Vec::new();
        c.read_to_end(&mut got).await.unwrap();
        assert_eq!(&got, b"tail", "the prefix survives a closed socket");
    }
}
