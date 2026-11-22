use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn reliable_stream_recovers_loss_duplication_and_reordering() {
    let config = KcpConfig::default();
    let mut listener = KcpListener::bind(&config, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let server = listener.local_addr();
    let relay = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let proxy = relay.local_addr().unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    let duplicates = Arc::new(AtomicUsize::new(0));
    let reordered = Arc::new(AtomicUsize::new(0));
    let parity = Arc::new(AtomicUsize::new(0));
    let counts = (
        dropped.clone(),
        duplicates.clone(),
        reordered.clone(),
        parity.clone(),
    );
    let forwarding = tokio::spawn(async move {
        let mut client = None;
        let mut buffer = [0; 2048];
        let mut sequence = 0usize;
        let mut held: Option<(SocketAddr, Vec<u8>, Instant)> = None;
        loop {
            // Reordering delays a packet for at most 10ms. Holding it until a
            // later packet *regardless of time* can trap the last ACK of a
            // congestion window and turn a reorder into an unbounded outage.
            let received = tokio::select! {
                received = relay.recv_from(&mut buffer) => Some(received.unwrap()),
                () = async {
                    tokio::time::sleep_until(held.as_ref().expect("guarded").2).await;
                }, if held.is_some() => {
                    let (target, packet, _) = held.take().expect("guarded");
                    relay.send_to(&packet, target).await.unwrap();
                    None
                }
            };
            let Some((size, source)) = received else {
                continue;
            };
            if size > fec::HEADER && &buffer[..4] == b"FKP5" && buffer[14] >= buffer[12] {
                counts.3.fetch_add(1, Ordering::SeqCst);
            }
            let target = if source == server {
                client.unwrap()
            } else {
                client = Some(source);
                server
            };
            sequence += 1;
            if sequence.is_multiple_of(17) {
                counts.0.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            if sequence.is_multiple_of(19) && held.is_none() {
                held = Some((
                    target,
                    buffer[..size].to_vec(),
                    Instant::now() + Duration::from_millis(10),
                ));
                continue;
            }
            relay.send_to(&buffer[..size], target).await.unwrap();
            if sequence.is_multiple_of(29) {
                counts.1.fetch_add(1, Ordering::SeqCst);
                relay.send_to(&buffer[..size], target).await.unwrap();
            }
            if let Some((target, packet, _)) = held.take() {
                counts.2.fetch_add(1, Ordering::SeqCst);
                relay.send_to(&packet, target).await.unwrap();
            }
        }
    });
    let mut client = connect(&config, proxy, Some("127.0.0.1".parse().unwrap()))
        .await
        .unwrap();
    let payload: Vec<u8> = (0..256 * 1024).map(|n| (n % 251) as u8).collect();
    let echo = tokio::spawn(async move {
        let (server, _) = listener.accept().await.unwrap();
        let (mut read, mut write) = tokio::io::split(server);
        let mut bytes = vec![0; 256 * 1024];
        read.read_exact(&mut bytes).await.unwrap();
        write.write_all(&bytes).await.unwrap();
        // Keep the KCP socket alive until the client has received the response.
        let mut finished = [0; 1];
        read.read_exact(&mut finished).await.unwrap();
        assert_eq!(finished, [42]);
        listener
    });
    tokio::time::timeout(Duration::from_secs(20), async {
        client.write_all(&payload).await.unwrap();
        let mut received = vec![0; payload.len()];
        client.read_exact(&mut received).await.unwrap();
        assert_eq!(received, payload);
        client.write_all(&[42]).await.unwrap();
        let _listener = echo.await.unwrap();
    })
    .await
    .unwrap();
    forwarding.abort();
    assert!(dropped.load(Ordering::SeqCst) > 0);
    assert!(duplicates.load(Ordering::SeqCst) > 0);
    assert!(reordered.load(Ordering::SeqCst) > 0);
    assert!(
        parity.load(Ordering::SeqCst) > 0,
        "the wire contains FEC parity packets"
    );
}

fn packet(conv: u32, data: &[u8]) -> Vec<u8> {
    let mut packet = vec![0; 24];
    packet[..4].copy_from_slice(&conv.to_le_bytes());
    packet[4] = 81;
    packet[6..8].copy_from_slice(&256u16.to_le_bytes());
    packet[20..24].copy_from_slice(&(data.len() as u32).to_le_bytes());
    packet.extend_from_slice(data);
    packet
}

#[test]
fn malformed_or_fragmented_datagrams_never_create_kcp_messages() {
    let good = packet(42, b"body");
    assert_eq!(valid_packet(&good, 1200), Some(42));
    assert_eq!(
        valid_packet(&[good.clone(), good.clone()].concat(), 1200),
        Some(42)
    );
    for (at, value) in [(4, 80), (5, 1), (20, 255)] {
        let mut bad = good.clone();
        bad[at] = value;
        assert_eq!(valid_packet(&bad, 1200), None);
    }
    assert_eq!(
        valid_packet(&[good.clone(), packet(43, b"body")].concat(), 1200),
        None
    );
    assert_eq!(valid_packet(&good[..23], 1200), None);
    assert_eq!(valid_packet(&packet(0, b"body"), 1200), None);
    assert_eq!(valid_packet(&packet(42, &vec![0; 1200]), 1200), None);
}

#[tokio::test]
async fn connected_udp_filters_forged_sources_even_with_the_correct_conversation() {
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let attacker = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let raw = KcpConfig {
        data_shards: 0,
        parity_shards: 0,
        ..Default::default()
    };
    let mut client = connect(&raw, peer.local_addr().unwrap(), None)
        .await
        .unwrap();
    client.write_all(b"hello").await.unwrap();
    let mut wire = [0; 1500];
    let (_, source) = peer.recv_from(&mut wire).await.unwrap();
    assert_eq!(client.local_addr().port(), source.port());
    let conv = u32::from_le_bytes(wire[..4].try_into().unwrap());
    let fake = packet(conv, b"forged");
    attacker.send_to(&fake, source).await.unwrap();
    let mut data = [0; 6];
    assert!(
        tokio::time::timeout(Duration::from_millis(60), client.read_exact(&mut data))
            .await
            .is_err()
    );
    peer.send_to(&fake, source).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), client.read_exact(&mut data))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&data, b"forged");
}

#[tokio::test]
async fn session_cap_and_listener_drop_reclaim_resources_for_reconnect() {
    let config = KcpConfig {
        max_sessions: 1,
        session_timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let mut listener = KcpListener::bind(&config, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let address = listener.local_addr();
    let mut first = connect(&config, address, None).await.unwrap();
    first.write_all(b"a").await.unwrap();
    let (mut accepted, _) = listener.accept().await.unwrap();
    assert_eq!(accepted.read_u8().await.unwrap(), b'a');
    let mut second = connect(&config, address, None).await.unwrap();
    second.write_all(b"b").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    drop(first);
    drop(accepted);
    let (mut accepted, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.read_u8().await.unwrap(), b'b');
    drop(listener);
    // Accepted stream objects do not keep the listener's UDP socket alive.
    let rebound = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(socket) = UdpSocket::bind(address).await {
                break socket;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(rebound.local_addr().unwrap(), address);
    assert_eq!(accepted.read(&mut [0]).await.unwrap(), 0);
}

#[tokio::test]
async fn idle_session_expiry_wakes_waiters_and_bad_source_family_is_rejected() {
    let config = KcpConfig {
        session_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let mut listener = KcpListener::bind(&config, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut client = connect(&config, listener.local_addr(), None).await.unwrap();
    client.write_all(&[1]).await.unwrap();
    let (mut server, _) = listener.accept().await.unwrap();
    assert_eq!(server.read_u8().await.unwrap(), 1);
    tokio::time::timeout(Duration::from_secs(3), async {
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        assert_eq!(client.read(&mut [0]).await.unwrap(), 0);
    })
    .await
    .unwrap();
    assert!(
        connect(&config, listener.local_addr(), Some("::1".parse().unwrap()))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn write_then_drop_delivers_the_accepted_bytes_before_reclaiming_the_socket() {
    let mut listener = KcpListener::bind(&Default::default(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut client = connect(&Default::default(), listener.local_addr(), None)
        .await
        .unwrap();
    client.write_all(b"request").await.unwrap();
    let (delivered, received) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut server, _) = listener.accept().await.unwrap();
        let mut request = [0; 7];
        server.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"request");
        server.write_all(b"last reply").await.unwrap();
        drop(server);
        received.await.unwrap();
        listener
    });
    let mut reply = [0; 10];
    tokio::time::timeout(Duration::from_secs(1), client.read_exact(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&reply, b"last reply");
    delivered.send(()).unwrap();
    let _listener = task.await.unwrap();
}
