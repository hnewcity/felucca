//! Protocol detection, so one listening port can serve several protocols.
//!
//! Running the bridge, the HTTP proxy and the HTTPS proxy on port 443 is a
//! common deployment need — often it is the only port a network lets out. The
//! detection itself is one reliable discriminator: a TLS record header versus a
//! plaintext HTTP request line.
//!
//! The Go original's classifier also does host matching, request-path routing
//! and `web_host` reservation here (`lib/pmux/classifier.go`). Those are routing
//! decisions that belong to the layer that owns the routing table; folding them
//! into detection is what makes that file hard to follow. This module answers
//! one question — what is this peer speaking — and hands the connection on with
//! its bytes intact.

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::conn::PrefixedConn;
use crate::error::TransportError;

/// How long to wait for a peer to say enough to classify it.
///
/// A connection that opens and then says nothing is usually a health check or a
/// port scan. Without a deadline each one would hold a task indefinitely.
pub const SNIFF_TIMEOUT: Duration = Duration::from_secs(10);

/// Bytes needed to classify with certainty: `OPTIONS ` is the longest method
/// token plus its separating space, and TLS needs only three.
const SNIFF_LEN: usize = 8;

/// What a peer appears to be speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// A TLS record — could be HTTPS, WSS, or a TLS-wrapped tunnel. Which one it
    /// is becomes clear from the ALPN or SNI after the handshake.
    Tls,
    /// A plaintext HTTP request. A WebSocket upgrade also starts this way, so
    /// telling those apart is the HTTP layer's job, not ours.
    Http,
}

/// HTTP methods worth recognising, longest first so a prefix check is
/// unambiguous.
const METHODS: &[&[u8]] = &[
    b"OPTIONS ",
    b"CONNECT ",
    b"DELETE ",
    b"PATCH ",
    b"TRACE ",
    b"POST ",
    b"HEAD ",
    b"GET ",
    b"PUT ",
];

/// Classifies a prefix already read from a peer.
///
/// `None` means undecided: the bytes so far match no known protocol *and* could
/// still become one with more data. Distinguishing that from a definite mismatch
/// is what lets the caller keep reading rather than dropping a slow client.
pub fn classify(buf: &[u8]) -> Option<Protocol> {
    if buf.is_empty() {
        return None;
    }

    // A TLS record: content type 22 (handshake) then a major version of 3. Every
    // TLS version in use, 1.0 through 1.3, opens this way.
    if buf[0] == 0x16 {
        return match buf.get(1) {
            None => None,
            Some(0x03) => Some(Protocol::Tls),
            Some(_) => None,
        };
    }

    // An HTTP request line starts with a method token and a space.
    for method in METHODS {
        let n = buf.len().min(method.len());
        if buf[..n] == method[..n] {
            return if buf.len() >= method.len() {
                Some(Protocol::Http)
            } else {
                None
            };
        }
    }
    None
}

/// Whether `buf` could still become a recognised protocol with more bytes.
///
/// Used to fail fast: a peer whose first byte matches nothing will not improve,
/// and holding the connection for the full timeout only wastes a task.
fn could_still_match(buf: &[u8]) -> bool {
    if buf.is_empty() {
        return true;
    }
    if buf[0] == 0x16 {
        // Only a major version of 3 continues a TLS record.
        return buf.len() < 2 || buf[1] == 0x03;
    }
    METHODS.iter().any(|m| {
        let n = buf.len().min(m.len());
        buf[..n] == m[..n]
    })
}

/// Reads just enough from `io` to classify it, then returns the protocol and the
/// connection with those bytes replayed.
pub async fn sniff<T>(mut io: T) -> Result<(Protocol, PrefixedConn<T>), TransportError>
where
    T: AsyncRead + Unpin,
{
    let mut buf = BytesMut::with_capacity(SNIFF_LEN);

    let outcome = tokio::time::timeout(SNIFF_TIMEOUT, async {
        loop {
            if let Some(proto) = classify(&buf) {
                return Ok(proto);
            }
            if !could_still_match(&buf) {
                return Err(TransportError::UnknownProtocol);
            }
            if buf.len() >= SNIFF_LEN {
                // Enough bytes to decide any known protocol, and none matched.
                return Err(TransportError::UnknownProtocol);
            }
            if io.read_buf(&mut buf).await? == 0 {
                // The peer hung up mid-classification.
                return Err(TransportError::UnknownProtocol);
            }
        }
    })
    .await;

    let proto = match outcome {
        Ok(r) => r?,
        Err(_) => return Err(TransportError::Timeout),
    };
    Ok((proto, PrefixedConn::new(Bytes::from(buf), io)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn a_tls_client_hello_is_recognised() {
        // Handshake record, TLS 1.0 through 1.3 all use a 0x0301+ record version.
        assert_eq!(
            classify(&[0x16, 0x03, 0x01, 0x02, 0x00]),
            Some(Protocol::Tls)
        );
        assert_eq!(classify(&[0x16, 0x03, 0x03]), Some(Protocol::Tls));
        assert_eq!(classify(&[0x16, 0x03, 0x04]), Some(Protocol::Tls));
    }

    #[test]
    fn every_http_method_is_recognised() {
        for line in [
            "GET / HTTP/1.1",
            "POST /x HTTP/1.1",
            "PUT /x HTTP/1.1",
            "HEAD / HTTP/1.1",
            "DELETE / HTTP/1.1",
            "OPTIONS * HTTP/1.1",
            "PATCH /x HTTP/1.1",
            "TRACE / HTTP/1.1",
            "CONNECT host:443 HTTP/1.1",
        ] {
            assert_eq!(classify(line.as_bytes()), Some(Protocol::Http), "{line}");
        }
    }

    #[test]
    fn a_websocket_upgrade_classifies_as_http() {
        // It is an HTTP request until the upgrade completes; separating ws from
        // plain http is the HTTP layer's decision, not the sniffer's.
        assert_eq!(
            classify(b"GET /ws HTTP/1.1\r\nUpgrade: websocket"),
            Some(Protocol::Http)
        );
    }

    #[test]
    fn a_partial_prefix_stays_undecided() {
        // Enough to guess but not to be sure: the caller must keep reading.
        assert_eq!(classify(b"GE"), None);
        assert_eq!(classify(b"OPTION"), None);
        assert_eq!(classify(&[0x16]), None);
        assert_eq!(classify(b""), None);
    }

    #[test]
    fn a_method_without_its_space_is_not_yet_a_match() {
        // "GETS" is not a method; only "GET " is.
        assert_eq!(classify(b"GET"), None);
        assert_eq!(classify(b"GETX /"), None);
    }

    #[test]
    fn junk_is_rejected_rather_than_guessed() {
        assert_eq!(classify(&[0x00, 0x01, 0x02, 0x03]), None);
        assert_eq!(classify(b"HELLO THERE"), None);
        // A 0x16 first byte with a wrong version is not TLS.
        assert_eq!(classify(&[0x16, 0x99, 0x01]), None);
    }

    #[test]
    fn a_hopeless_prefix_is_known_to_be_hopeless() {
        // Fail fast rather than holding the connection for the whole timeout.
        assert!(!could_still_match(b"HELLO"));
        assert!(!could_still_match(&[0x16, 0x99]));
        assert!(could_still_match(b"GE"));
        assert!(could_still_match(&[0x16]));
        assert!(could_still_match(b""));
    }

    #[tokio::test]
    async fn sniffing_leaves_the_stream_intact_for_the_handler() {
        let (mut client, server) = tokio::io::duplex(1024);
        client
            .write_all(b"GET /path HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();

        let (proto, mut conn) = sniff(server).await.unwrap();
        assert_eq!(proto, Protocol::Http);

        // The handler must see the request from its first byte.
        let mut got = vec![0u8; 18];
        conn.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"GET /path HTTP/1.1");
    }

    #[tokio::test]
    async fn a_tls_connection_keeps_its_client_hello() {
        let (mut client, server) = tokio::io::duplex(1024);
        let hello = [0x16, 0x03, 0x01, 0x00, 0x2f, 0x01, 0x00, 0x00, 0x2b];
        client.write_all(&hello).await.unwrap();

        let (proto, mut conn) = sniff(server).await.unwrap();
        assert_eq!(proto, Protocol::Tls);

        // The TLS acceptor needs the ClientHello it never saw us read.
        let mut got = [0u8; 9];
        conn.read_exact(&mut got).await.unwrap();
        assert_eq!(got, hello);
    }

    #[tokio::test]
    async fn a_byte_at_a_time_client_is_still_classified() {
        // A dribbling peer must not be mistaken for an unknown protocol.
        let (mut client, server) = tokio::io::duplex(1024);
        let writer = tokio::spawn(async move {
            for b in b"GET / HTTP/1.1" {
                client.write_all(&[*b]).await.unwrap();
                client.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            client
        });

        let (proto, _) = sniff(server).await.unwrap();
        assert_eq!(proto, Protocol::Http);
        let _ = writer.await;
    }

    #[tokio::test]
    async fn a_silent_peer_times_out_rather_than_hanging() {
        tokio::time::pause();
        let (_client, server) = tokio::io::duplex(1024);

        let task = tokio::spawn(async move { sniff(server).await });
        tokio::time::advance(SNIFF_TIMEOUT + Duration::from_secs(1)).await;

        let err = task.await.unwrap().expect_err("must not wait forever");
        assert!(matches!(err, TransportError::Timeout), "{err}");
    }

    #[tokio::test]
    async fn a_peer_that_hangs_up_early_is_rejected() {
        let (client, server) = tokio::io::duplex(1024);
        drop(client);

        let err = sniff(server).await.expect_err("nothing to classify");
        assert!(matches!(err, TransportError::UnknownProtocol), "{err}");
    }

    #[tokio::test]
    async fn junk_is_rejected_without_waiting_for_the_timeout() {
        let (mut client, server) = tokio::io::duplex(1024);
        client.write_all(b"\x00\x01\x02\x03").await.unwrap();

        // No time is advanced, so this only passes if detection fails fast.
        let err = sniff(server).await.expect_err("junk should be refused");
        assert!(matches!(err, TransportError::UnknownProtocol), "{err}");
    }
}
