//! QUIC transport.
//!
//! QUIC already multiplexes streams, with per-stream flow control and no
//! head-of-line blocking between them — exactly what `felucca-mux` provides for
//! stream transports. So felucca does **not** run mux over QUIC. Doing that would
//! put two credit systems in series, and worse, funnel every logical stream
//! through one QUIC stream, which reintroduces the head-of-line blocking QUIC
//! exists to avoid: one lost packet would stall every tunnelled connection.
//!
//! Instead a QUIC connection is a tunnel in its own right, and its native bidi
//! streams are the tunnel's streams. [`crate::tunnel::Tunnel`] hides the
//! difference from callers.
//!
//! The Go original reaches the same conclusion (`bridge/runtime_tunnel_server.go`
//! uses the QUIC session directly rather than wrapping it in mux), but only for
//! protocol version 5 and above, with the mux path still there for older peers.
//! With no compatibility to keep, felucca has one rule.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use quinn::{ClientConfig, Endpoint, RecvStream, SendStream, ServerConfig};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::TransportError;
use crate::tls::{Identity, Verification};

/// ALPN identifier for the felucca tunnel protocol.
///
/// Set on both sides so a QUIC endpoint shared with an HTTP/3 service can tell
/// tunnel traffic from web traffic.
pub const ALPN: &[u8] = b"felucca/1";

/// A QUIC bidirectional stream as one duplex byte stream.
///
/// quinn splits the directions into separate handles; a caller that just wants to
/// copy bytes needs them joined.
#[derive(Debug)]
pub struct QuicStream {
    send: SendStream,
    recv: RecvStream,
}

impl QuicStream {
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }

    /// The stream's id on its connection, for correlating logs.
    pub fn id(&self) -> u64 {
        self.send.id().index()
    }
}

// quinn's stream types carry both inherent `poll_read`/`poll_write` methods, which
// return quinn's own error types, and the tokio trait impls. Inherent methods win
// method resolution, so these forwards name the trait explicitly — writing
// `Pin::new(&mut self.send).poll_write(..)` compiles against the wrong one.
impl AsyncRead for QuicStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.recv), cx, buf)
    }
}

impl AsyncWrite for QuicStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.send), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.send), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Finishes the send direction while leaving the receive direction open,
        // which is the half-close a proxied protocol needs.
        AsyncWrite::poll_shutdown(Pin::new(&mut self.send), cx)
    }
}

/// Builds a QUIC server endpoint serving `identity` on `addr`.
pub fn server(identity: &Identity, addr: SocketAddr) -> Result<Endpoint, TransportError> {
    Endpoint::server(server_config(identity)?, addr).map_err(|source| TransportError::Bind {
        addr: addr.to_string(),
        source,
    })
}

/// Build a replacement config for both a new endpoint and live certificate reload.
pub fn server_config(identity: &Identity) -> Result<ServerConfig, TransportError> {
    // QUIC mandates TLS 1.3; a config that also offers 1.2 is rejected outright,
    // so the version list is pinned rather than left to the default.
    let mut tls = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(identity.chain.clone(), identity.key.clone_key())
        .map_err(|e| TransportError::tls(format!("cannot build a QUIC server config: {e}")))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];

    let quic_tls = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| TransportError::tls(format!("config is not usable for QUIC: {e}")))?;

    let mut config = ServerConfig::with_crypto(Arc::new(quic_tls));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// Connects to a QUIC server, returning the established connection.
pub async fn connect(
    addr: &str,
    policy: &Verification,
) -> Result<quinn::Connection, TransportError> {
    let target: SocketAddr = tokio::net::lookup_host(addr)
        .await
        .map_err(|source| TransportError::Connect {
            addr: addr.to_owned(),
            source,
        })?
        .next()
        .ok_or_else(|| TransportError::Connect {
            addr: addr.to_owned(),
            source: io::Error::new(io::ErrorKind::NotFound, "no address resolved"),
        })?;

    let host = addr
        .rsplit_once(':')
        .map_or(addr, |(h, _)| h)
        .trim_matches(['[', ']']);
    connect_to(target, host, policy, None).await
}

/// Connect using an already resolved address and an optional bound source IP.
pub async fn connect_to(
    target: SocketAddr,
    host: &str,
    policy: &Verification,
    local_ip: Option<std::net::IpAddr>,
) -> Result<quinn::Connection, TransportError> {
    if local_ip.is_some_and(|ip| ip.is_ipv4() != target.is_ipv4()) {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "QUIC destination and local_ip have different families",
        )
        .into());
    }
    let mut tls = policy.rustls_client_config_tls13()?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| TransportError::tls(format!("config is not usable for QUIC: {e}")))?;

    // Bind whichever family the target needs; a v4 socket cannot reach a v6 peer.
    let local: SocketAddr = if target.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    }
    .parse()
    .expect("a literal wildcard address parses");
    let local = local_ip.map_or(local, |ip| SocketAddr::new(ip, 0));
    let mut endpoint = Endpoint::client(local).map_err(|source| TransportError::Bind {
        addr: local.to_string(),
        source,
    })?;
    let mut config = ClientConfig::new(Arc::new(quic_tls));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
    config.transport_config(Arc::new(transport));
    endpoint.set_default_client_config(config);

    let server_name = policy.server_name_for(host);
    // Validate the name before dialling, so a bad one is a config error rather
    // than a connection failure.
    ServerName::try_from(server_name.clone())
        .map_err(|_| TransportError::tls(format!("{server_name:?} is not a valid server name")))?;

    endpoint
        .connect(target, &server_name)
        .map_err(|e| TransportError::tls(format!("cannot start a QUIC connection: {e}")))?
        .await
        .map_err(|e| TransportError::tls(format!("QUIC handshake with {target} failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn identity() -> Identity {
        Identity::self_signed(vec!["localhost".to_owned()]).unwrap()
    }

    /// Starts a QUIC server that echoes each stream, returning its port.
    async fn serve(identity: &Identity) -> u16 {
        let endpoint = server(identity, "127.0.0.1:0".parse().unwrap()).unwrap();
        let port = endpoint.local_addr().unwrap().port();

        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let Ok(conn) = incoming.await else { continue };
                tokio::spawn(async move {
                    while let Ok((send, recv)) = conn.accept_bi().await {
                        let mut s = QuicStream::new(send, recv);
                        tokio::spawn(async move {
                            let mut buf = Vec::new();
                            // Echo whatever arrives until the peer half-closes.
                            let _ = s.read_to_end(&mut buf).await;
                            let _ = s.write_all(&buf).await;
                            let _ = s.shutdown().await;
                        });
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn a_pinned_client_completes_a_quic_handshake() {
        let id = identity();
        let port = serve(&id).await;

        let policy = Verification::Pinned {
            fingerprint: id.fingerprint(),
        };
        let conn = connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .expect("should connect");

        let (send, recv) = conn.open_bi().await.unwrap();
        let mut s = QuicStream::new(send, recv);
        s.write_all(b"quic hello").await.unwrap();
        s.shutdown().await.unwrap();

        let mut back = Vec::new();
        s.read_to_end(&mut back).await.unwrap();
        assert_eq!(&back, b"quic hello");
    }

    #[tokio::test]
    async fn a_mismatched_pin_refuses_the_connection() {
        let id = identity();
        let port = serve(&id).await;

        let policy = Verification::Pinned {
            fingerprint: identity().fingerprint(),
        };
        let err = connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .expect_err("a wrong pin must not connect");
        assert!(matches!(err, TransportError::Tls(_)), "{err}");
    }

    #[tokio::test]
    async fn native_streams_are_independent() {
        // The property that makes running mux over QUIC wrong: streams do not
        // block each other, so a stalled one leaves the rest working.
        let id = identity();
        let port = serve(&id).await;
        let policy = Verification::Pinned {
            fingerprint: id.fingerprint(),
        };
        let conn = connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .unwrap();

        // Open one stream and leave it hanging without finishing it.
        let (stalled_send, stalled_recv) = conn.open_bi().await.unwrap();
        let mut stalled = QuicStream::new(stalled_send, stalled_recv);
        stalled.write_all(b"never finished").await.unwrap();
        stalled.flush().await.unwrap();

        // A second stream must still complete end to end.
        let (send, recv) = conn.open_bi().await.unwrap();
        let mut healthy = QuicStream::new(send, recv);
        healthy.write_all(b"unblocked").await.unwrap();
        healthy.shutdown().await.unwrap();

        let mut back = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            healthy.read_to_end(&mut back),
        )
        .await
        .expect("a stalled stream must not block another")
        .unwrap();
        assert_eq!(&back, b"unblocked");
    }

    #[tokio::test]
    async fn many_streams_run_concurrently() {
        let id = identity();
        let port = serve(&id).await;
        let policy = Verification::Pinned {
            fingerprint: id.fingerprint(),
        };
        let conn = connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .unwrap();

        let mut tasks = Vec::new();
        for i in 0..32u32 {
            let conn = conn.clone();
            tasks.push(tokio::spawn(async move {
                let (send, recv) = conn.open_bi().await.unwrap();
                let mut s = QuicStream::new(send, recv);
                s.write_all(&i.to_be_bytes()).await.unwrap();
                s.shutdown().await.unwrap();
                let mut back = Vec::new();
                s.read_to_end(&mut back).await.unwrap();
                assert_eq!(back, i.to_be_bytes(), "reply landed on the wrong stream");
            }));
        }
        for t in tasks {
            tokio::time::timeout(std::time::Duration::from_secs(15), t)
                .await
                .expect("stream timed out")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn a_bulk_transfer_arrives_intact() {
        const LEN: usize = 2 * 1024 * 1024;
        let id = identity();
        let port = serve(&id).await;
        let policy = Verification::Pinned {
            fingerprint: id.fingerprint(),
        };
        let conn = connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .unwrap();

        let (send, recv) = conn.open_bi().await.unwrap();
        let mut s = QuicStream::new(send, recv);
        let payload: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();

        let expected = payload.clone();
        let writer = tokio::spawn(async move {
            s.write_all(&payload).await.unwrap();
            s.shutdown().await.unwrap();
            s
        });
        let mut s = writer.await.unwrap();

        let mut got = Vec::with_capacity(LEN);
        tokio::time::timeout(std::time::Duration::from_secs(30), s.read_to_end(&mut got))
            .await
            .expect("transfer should not stall")
            .unwrap();
        assert_eq!(got.len(), LEN);
        assert_eq!(got, expected, "payload corrupted in transit");
    }

    #[tokio::test]
    async fn an_unreachable_server_reports_a_connect_error() {
        // Nothing is listening on this UDP port.
        let err = connect("127.0.0.1:1", &Verification::Insecure)
            .await
            .expect_err("must not succeed");
        // QUIC has no RST equivalent, so this surfaces as a handshake timeout.
        assert!(
            matches!(err, TransportError::Tls(_) | TransportError::Connect { .. }),
            "{err}"
        );
    }
}
