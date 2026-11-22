//! One interface over both kinds of tunnel.
//!
//! felucca reaches multiplexed streams two different ways:
//!
//! - over a stream transport (TCP, TLS, WebSocket), by running `felucca-mux`
//! - over QUIC, by using its native streams, since it multiplexes already
//!
//! Callers should not care which. [`Tunnel`] presents both as "open a stream,
//! accept a stream", and [`TunnelStream`] as "a duplex byte stream".
//!
//! The variants are an enum rather than boxed trait objects on purpose: this is
//! the data path, and every read and write would otherwise pay for a virtual
//! call.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use felucca_mux::{CloseReason, Config as MuxConfig, Role, Session, SessionHandle};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::conn::Conn;
use crate::error::TransportError;
use crate::quic::QuicStream;

pub fn mux_config(config: &felucca_core::config::MuxConfig) -> MuxConfig {
    MuxConfig {
        initial_stream_window: config.initial_stream_window,
        max_stream_window: config.max_stream_window,
        max_session_window: config.max_session_window,
        max_send_queue: config.max_send_queue,
        max_control_queue: config.max_control_queue,
        max_streams: config.max_streams,
        accept_backlog: config.accept_backlog,
        open_timeout: config.open_timeout,
        ..Default::default()
    }
}

/// A duplex byte stream on a tunnel.
#[derive(Debug)]
pub enum TunnelStream {
    Local(tokio::io::DuplexStream),
    Mux(felucca_mux::Stream),
    Quic(QuicStream),
}

impl AsyncRead for TunnelStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Mux(s) => Pin::new(s).poll_read(cx, buf),
            Self::Quic(s) => Pin::new(s).poll_read(cx, buf),
            Self::Local(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for TunnelStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Mux(s) => Pin::new(s).poll_write(cx, buf),
            Self::Quic(s) => Pin::new(s).poll_write(cx, buf),
            Self::Local(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Mux(s) => Pin::new(s).poll_flush(cx),
            Self::Quic(s) => Pin::new(s).poll_flush(cx),
            Self::Local(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Mux(s) => Pin::new(s).poll_shutdown(cx),
            Self::Quic(s) => Pin::new(s).poll_shutdown(cx),
            Self::Local(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// A tunnel carrying many streams between a server and an agent.
#[derive(Debug)]
pub enum Tunnel {
    Mux(Session),
    Quic(quinn::Connection),
}

impl Tunnel {
    /// Runs a mux session over a stream transport.
    pub fn over_stream<T: Conn + 'static>(io: T, role: Role, config: MuxConfig) -> Self {
        Self::Mux(Session::new(io, config, role))
    }

    /// Uses a QUIC connection's native streams.
    pub fn over_quic(conn: quinn::Connection) -> Self {
        Self::Quic(conn)
    }

    /// A handle for opening streams from any task.
    pub fn handle(&self) -> TunnelHandle {
        match self {
            Self::Mux(s) => TunnelHandle::Mux(s.handle()),
            Self::Quic(c) => TunnelHandle::Quic(c.clone()),
        }
    }

    /// Opens a stream.
    pub async fn open(&self) -> Result<TunnelStream, TransportError> {
        self.handle().open().await
    }

    /// Waits for the peer to open a stream.
    pub async fn accept(&mut self) -> Result<TunnelStream, TransportError> {
        match self {
            Self::Mux(s) => s.accept().await.map(TunnelStream::Mux).map_err(mux_error),
            Self::Quic(c) => {
                let (send, recv) = c.accept_bi().await.map_err(quic_error)?;
                Ok(TunnelStream::Quic(QuicStream::new(send, recv)))
            }
        }
    }

    /// Smoothed round-trip time, once measured.
    pub fn rtt(&self) -> Option<Duration> {
        match self {
            Self::Mux(s) => s.rtt(),
            // quinn measures RTT as part of congestion control, so there is
            // always an estimate rather than a wait for the first ping.
            Self::Quic(c) => Some(c.rtt()),
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            Self::Mux(s) => s.is_closed(),
            Self::Quic(c) => c.close_reason().is_some(),
        }
    }

    /// Tells the peer we are going away, then tears the tunnel down.
    pub fn close(&self, reason: CloseReason) {
        match self {
            Self::Mux(s) => s.close(reason),
            // QUIC carries an application-defined code; reuse the mux reason so
            // both transports report the same thing in a log.
            Self::Quic(c) => c.close((reason as u32).into(), b"felucca"),
        }
    }
}

/// A cloneable handle for opening streams on a live tunnel.
#[derive(Debug, Clone)]
pub enum TunnelHandle {
    Local(crate::local::LocalHandle),
    Mux(SessionHandle),
    Quic(quinn::Connection),
}

impl TunnelHandle {
    /// Opens a stream.
    pub async fn open(&self) -> Result<TunnelStream, TransportError> {
        self.open_context(crate::local::LocalContext {
            peer: ([127, 0, 0, 1], 0).into(),
            trace: Default::default(),
        })
        .await
    }
    pub async fn open_context(
        &self,
        context: crate::local::LocalContext,
    ) -> Result<TunnelStream, TransportError> {
        match self {
            Self::Local(handle) => handle
                .open(context)
                .await
                .map(TunnelStream::Local)
                .map_err(Into::into),
            // Opening a mux stream is synchronous: it is optimistic, so there is
            // nothing to wait for.
            Self::Mux(h) => h.open().map(TunnelStream::Mux).map_err(mux_error),
            Self::Quic(c) => {
                let (send, recv) = c.open_bi().await.map_err(quic_error)?;
                Ok(TunnelStream::Quic(QuicStream::new(send, recv)))
            }
        }
    }

    pub fn close(&self, reason: CloseReason) {
        match self {
            Self::Local(handle) => handle.close(),
            Self::Mux(h) => h.close(reason),
            Self::Quic(c) => c.close((reason as u32).into(), b"felucca"),
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            Self::Local(handle) => handle.is_closed(),
            Self::Mux(h) => h.is_closed(),
            Self::Quic(c) => c.close_reason().is_some(),
        }
    }
}

fn mux_error(e: felucca_mux::MuxError) -> TransportError {
    // Preserve the graceful/fault distinction: a caller deciding whether to
    // reconnect needs to know an orderly close from a broken one.
    if e.is_graceful() {
        TransportError::Io(io::ErrorKind::UnexpectedEof.into())
    } else {
        TransportError::Io(e.into())
    }
}

fn quic_error(e: quinn::ConnectionError) -> TransportError {
    use quinn::ConnectionError as E;
    match e {
        E::ApplicationClosed(_) | E::LocallyClosed => {
            TransportError::Io(io::ErrorKind::UnexpectedEof.into())
        }
        E::TimedOut => TransportError::Timeout,
        other => TransportError::Io(io::Error::other(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Echoes one stream, so both transports can be driven by the same test body.
    async fn echo_once(mut tunnel: Tunnel) {
        let Ok(mut s) = tunnel.accept().await else {
            return;
        };
        let mut buf = Vec::new();
        // Read until the peer half-closes, then send it all back.
        let _ = s.read_to_end(&mut buf).await;
        let _ = s.write_all(&buf).await;
        let _ = s.shutdown().await;
        // Hold the tunnel open until the peer has read the reply.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    /// A connected pair of mux tunnels over an in-memory pipe.
    fn mux_pair() -> (Tunnel, Tunnel) {
        let (a, b) = tokio::io::duplex(256 * 1024);
        (
            Tunnel::over_stream(a, Role::Client, MuxConfig::default()),
            Tunnel::over_stream(b, Role::Server, MuxConfig::default()),
        )
    }

    /// A connected pair of QUIC tunnels over loopback.
    async fn quic_pair() -> (Tunnel, Tunnel) {
        let id = crate::tls::Identity::self_signed(vec!["localhost".into()]).unwrap();
        let endpoint = crate::quic::server(&id, "127.0.0.1:0".parse().unwrap()).unwrap();
        let port = endpoint.local_addr().unwrap().port();

        let accepting = tokio::spawn(async move {
            let incoming = endpoint.accept().await.expect("an inbound connection");
            let conn = incoming.await.expect("handshake");
            // The endpoint must outlive the connection or the socket closes.
            (Tunnel::over_quic(conn), endpoint)
        });

        let policy = crate::tls::Verification::Pinned {
            fingerprint: id.fingerprint(),
        };
        let client = crate::quic::connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .unwrap();
        let (server, endpoint) = accepting.await.unwrap();
        // Leak the endpoint for the duration of the test: dropping it would close
        // the connection under the test's feet.
        std::mem::forget(endpoint);
        (Tunnel::over_quic(client), server)
    }

    #[tokio::test]
    async fn a_mux_tunnel_carries_a_stream() {
        let (client, server) = mux_pair();
        tokio::spawn(echo_once(server));

        let mut s = client.open().await.unwrap();
        s.write_all(b"over mux").await.unwrap();
        s.shutdown().await.unwrap();

        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut back))
            .await
            .expect("should not stall")
            .unwrap();
        assert_eq!(&back, b"over mux");
    }

    #[tokio::test]
    async fn a_quic_tunnel_carries_a_stream() {
        let (client, server) = quic_pair().await;
        tokio::spawn(echo_once(server));

        let mut s = client.open().await.unwrap();
        s.write_all(b"over quic").await.unwrap();
        s.shutdown().await.unwrap();

        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut back))
            .await
            .expect("should not stall")
            .unwrap();
        assert_eq!(&back, b"over quic");
    }

    #[tokio::test]
    async fn both_transports_report_an_rtt() {
        // A caller watching tunnel health should not have to special-case which
        // transport it got.
        let (quic_client, _quic_server) = quic_pair().await;
        assert!(quic_client.rtt().is_some(), "quinn always has an estimate");

        let (mux_client, _mux_server) = mux_pair();
        // Mux measures on its first ping, so this may be None immediately; what
        // matters is that the call works on both.
        let _ = mux_client.rtt();
    }

    #[tokio::test]
    async fn a_handle_opens_streams_from_another_task() {
        let (client, server) = mux_pair();
        tokio::spawn(echo_once(server));

        let handle = client.handle();
        let opened = tokio::spawn(async move {
            let mut s = handle.open().await.unwrap();
            s.write_all(b"from a task").await.unwrap();
            s.shutdown().await.unwrap();
            let mut back = Vec::new();
            s.read_to_end(&mut back).await.unwrap();
            back
        });

        let back = tokio::time::timeout(Duration::from_secs(10), opened)
            .await
            .expect("should not stall")
            .unwrap();
        assert_eq!(&back, b"from a task");
    }

    #[tokio::test]
    async fn closing_a_mux_tunnel_is_observable() {
        let (client, _server) = mux_pair();
        assert!(!client.is_closed());
        client.close(CloseReason::Shutdown);
        assert!(client.is_closed());
        assert!(
            client.open().await.is_err(),
            "a closed tunnel refuses new streams"
        );
    }

    #[tokio::test]
    async fn closing_a_quic_tunnel_is_observable() {
        let (client, _server) = quic_pair().await;
        assert!(!client.is_closed());
        client.close(CloseReason::Shutdown);
        assert!(client.is_closed());
        assert!(
            client.open().await.is_err(),
            "a closed tunnel refuses new streams"
        );
    }

    #[tokio::test]
    async fn a_stream_is_send_so_it_can_be_moved_into_a_task() {
        fn assert_send<T: Send>() {}
        assert_send::<TunnelStream>();
        assert_send::<Tunnel>();
        assert_send::<TunnelHandle>();
    }
}
