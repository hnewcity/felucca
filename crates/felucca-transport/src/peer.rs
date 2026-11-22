//! One-ticket QUIC endpoints over the socket already used for rendezvous.
use crate::tls::{Identity, Verification};
use std::{io, net::SocketAddr, sync::Arc, time::Duration};

const ALPN: &[u8] = b"felucca-peer/1";
fn transport() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.max_concurrent_bidi_streams(1u32.into());
    config.max_concurrent_uni_streams(0u32.into());
    config.stream_receive_window((256u32 * 1024).into());
    config.receive_window((512u32 * 1024).into());
    config.send_window(512 * 1024);
    config.datagram_receive_buffer_size(None);
    config.keep_alive_interval(Some(Duration::from_secs(2)));
    config.max_idle_timeout(Some(
        Duration::from_secs(10).try_into().expect("bounded timeout"),
    ));
    Arc::new(config)
}
pub fn endpoint(
    socket: std::net::UdpSocket,
    identity: Option<&Identity>,
) -> io::Result<quinn::Endpoint> {
    socket.set_nonblocking(true)?;
    let server = identity
        .map(|identity| {
            let provider = rustls::crypto::CryptoProvider::get_default()
                .cloned()
                .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));
            let mut tls = rustls::ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(io::Error::other)?
                .with_no_client_auth()
                .with_single_cert(identity.chain.clone(), identity.key.clone_key())
                .map_err(io::Error::other)?;
            tls.alpn_protocols = vec![ALPN.to_vec()];
            let crypto =
                quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(io::Error::other)?;
            let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
            server.transport_config(transport());
            Ok::<_, io::Error>(server)
        })
        .transpose()?;
    quinn::Endpoint::new(
        Default::default(),
        server,
        socket,
        Arc::new(quinn::TokioRuntime),
    )
}
pub async fn connect(
    endpoint: &quinn::Endpoint,
    peer: SocketAddr,
    fingerprint: &str,
) -> io::Result<quinn::Connection> {
    let mut tls = Verification::Pinned {
        fingerprint: fingerprint.into(),
    }
    .rustls_client_config_tls13()
    .map_err(io::Error::other)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto =
        quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(io::Error::other)?;
    let mut client = quinn::ClientConfig::new(Arc::new(crypto));
    client.transport_config(transport());
    endpoint
        .connect_with(client, peer, "peer.felucca")
        .map_err(io::Error::other)?
        .await
        .map_err(io::Error::other)
}
/// Sends only; QUIC is the sole receiver after taking ownership of its clone.
/// Fixed-bit-zero packets are discarded by QUIC without parsing a connection.
pub async fn punch(
    socket: std::net::UdpSocket,
    peer: SocketAddr,
    ticket: &[u8; 16],
    secret: &felucca_core::Secret,
) -> io::Result<()> {
    socket.set_nonblocking(true)?;
    let mut bytes = [0; 64];
    bytes[..4].copy_from_slice(b"\0FP2");
    bytes[4..20].copy_from_slice(ticket);
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let tag = ring::hmac::sign(&key, &bytes[..20]);
    bytes[20..52].copy_from_slice(tag.as_ref());
    let mut timer = tokio::time::interval(Duration::from_millis(50));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    for _ in 0..40 {
        timer.tick().await;
        match socket.send_to(&bytes, peer) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn observed_socket_keeps_its_port_for_pinned_quic_bulk_and_half_close() {
        let broker = crate::rendezvous::Broker::bind("127.0.0.1:0".parse().unwrap(), 1)
            .await
            .unwrap();
        let keys: [felucca_core::Secret; 2] =
            ["provider-key-1234".into(), "visitor-key-12345".into()];
        let ticket = [42; 16];
        let registration = broker
            .register(ticket, [&keys[0], &keys[1]], Duration::from_secs(10))
            .unwrap();
        let stop = tokio_util::sync::CancellationToken::new();
        let broker_task = tokio::spawn(broker.clone().run(stop.clone()));
        let (server_socket, server_address) = crate::rendezvous::observe(
            broker.local_addr().unwrap(),
            "127.0.0.1".parse().unwrap(),
            ticket,
            0,
            &keys[0],
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        let (client_socket, client_address) = crate::rendezvous::observe(
            broker.local_addr().unwrap(),
            "127.0.0.1".parse().unwrap(),
            ticket,
            1,
            &keys[1],
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(registration.observed(0).await.unwrap(), server_address);
        assert_eq!(registration.observed(1).await.unwrap(), client_address);
        let id = Identity::self_signed(vec!["peer.felucca".into()]).unwrap();
        let punch_socket = server_socket.try_clone().unwrap();
        let endpoint = endpoint(server_socket, Some(&id)).unwrap();
        assert_eq!(endpoint.local_addr().unwrap(), server_address);
        let key = keys[0].clone();
        let punching =
            tokio::spawn(async move { punch(punch_socket, client_address, &ticket, &key).await });
        let (done, received) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            let (send, recv) = connection.accept_bi().await.unwrap();
            let mut stream = crate::quic::QuicStream::new(send, recv);
            let mut data = Vec::new();
            stream.read_to_end(&mut data).await.unwrap();
            assert_eq!(data.len(), 2 * 1024 * 1024);
            stream.write_all(&data).await.unwrap();
            stream.shutdown().await.unwrap();
            received.await.unwrap();
            connection.close(0u32.into(), b"done");
        });
        let client = super::endpoint(client_socket, None).unwrap();
        assert_eq!(client.local_addr().unwrap(), client_address);
        let connection = connect(&client, server_address, &id.fingerprint())
            .await
            .unwrap();
        let (send, recv) = connection.open_bi().await.unwrap();
        let mut stream = crate::quic::QuicStream::new(send, recv);
        let data: Vec<_> = (0..2 * 1024 * 1024).map(|n| (n % 251) as u8).collect();
        tokio::time::timeout(Duration::from_secs(10), async {
            stream.write_all(&data).await.unwrap();
            stream.shutdown().await.unwrap();
            let mut received = Vec::new();
            stream.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, data);
        })
        .await
        .unwrap();
        done.send(()).unwrap();
        serving.await.unwrap();
        punching.abort();
        stop.cancel();
        broker_task.await.unwrap().unwrap();
    }
}
