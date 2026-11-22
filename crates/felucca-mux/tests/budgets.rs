use felucca_mux::{CloseReason, Config, FrameHeader, FrameType, MAX_SEGMENT, Role, Session};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn eventually(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition did not become true");
}

#[tokio::test]
async fn asymmetric_windows_negotiate_and_bulk_transfer_stays_inside_budget() {
    let (a, b) = tokio::io::duplex(65536);
    let client = Session::new(
        a,
        Config {
            initial_stream_window: 512 * 1024,
            max_stream_window: 512 * 1024,
            ..Default::default()
        },
        Role::Client,
    );
    let mut server = Session::new(
        b,
        Config {
            initial_stream_window: MAX_SEGMENT as u32,
            max_stream_window: 128 * 1024,
            max_session_window: 128 * 1024,
            ..Default::default()
        },
        Role::Server,
    );
    let mut outgoing = client.open().unwrap();
    let mut incoming = server.accept().await.unwrap();
    let write = tokio::spawn(async move {
        outgoing
            .write_all(&vec![42; 2 * 1024 * 1024])
            .await
            .unwrap();
        outgoing.shutdown().await.unwrap();
    });
    let mut output = vec![];
    tokio::time::timeout(Duration::from_secs(5), incoming.read_to_end(&mut output))
        .await
        .unwrap()
        .unwrap();
    write.await.unwrap();
    assert_eq!(output, vec![42; 2 * 1024 * 1024]);
    assert!(
        incoming.receive_window() > MAX_SEGMENT as u32,
        "an actively drained stream should grow"
    );
    assert!(incoming.receive_window() <= 128 * 1024);
    assert!(server.reserved_receive_bytes() <= 128 * 1024);
    drop(incoming);
    eventually(|| server.reserved_receive_bytes() == 0).await;
}

#[tokio::test]
async fn fabricated_credit_and_oversized_window_growth_close_the_session() {
    for fabricated in [
        FrameHeader::window_update(1, 1),
        FrameHeader::new(
            FrameType::WindowGrow,
            1,
            felucca_mux::frame::MAX_STREAM_WINDOW,
        ),
    ] {
        let (a, mut raw) = tokio::io::duplex(65536);
        let client = Session::new(a, Config::default(), Role::Client);
        let mut stream = client.open().unwrap();
        raw.write_all(&FrameHeader::open_ack(stream.id(), MAX_SEGMENT as u32).encode())
            .await
            .unwrap();
        raw.write_all(&fabricated.encode()).await.unwrap();
        eventually(|| client.is_closed()).await;
        assert!(stream.write_all(b"must fail").await.is_err());
    }
}

#[tokio::test]
async fn an_unacknowledged_open_expires_without_waiting_for_keepalive() {
    let (a, _raw) = tokio::io::duplex(65536);
    let client = Session::new(
        a,
        Config {
            open_timeout: Duration::from_millis(40),
            ..Default::default()
        },
        Role::Client,
    );
    let mut stream = client.open().unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), stream.read_u8())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("Timeout"), "{error}");
    assert!(
        !client.is_closed(),
        "the stream timeout must not kill unrelated streams"
    );
    assert_eq!(client.stream_count(), 0);
}

#[tokio::test]
async fn a_stalled_writer_is_bounded_and_cancellation_wakes_a_queued_write() {
    let limit = MAX_SEGMENT + felucca_mux::HEADER_LEN;
    let (a, mut raw) = tokio::io::duplex(32);
    let client = Session::new(
        a,
        Config {
            max_send_queue: limit,
            ..Default::default()
        },
        Role::Client,
    );
    let mut stream = client.open().unwrap();
    raw.write_all(&FrameHeader::open_ack(stream.id(), 1024 * 1024).encode())
        .await
        .unwrap();
    let writer = tokio::spawn(async move { stream.write_all(&vec![7; 1024 * 1024]).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !writer.is_finished(),
        "a non-reading peer must impose backpressure"
    );
    assert!(client.queued_data_bytes() <= limit);
    client.close(CloseReason::Shutdown);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), writer)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn excessive_control_traffic_cannot_queue_unbounded_pongs() {
    let (a, mut raw) = tokio::io::duplex(32);
    let client = Session::new(
        a,
        Config {
            max_control_queue: 16,
            ..Default::default()
        },
        Role::Client,
    );
    let mut pings = Vec::new();
    for nonce in 0..1000 {
        pings.extend(FrameHeader::ping(nonce).encode());
    }
    // The raw peer keeps writing but deliberately never drains responses.
    let _ = tokio::time::timeout(Duration::from_secs(1), raw.write_all(&pings)).await;
    eventually(|| client.is_closed()).await;
}

#[tokio::test]
async fn a_closed_peer_stream_id_cannot_be_reused() {
    let (a, mut raw) = tokio::io::duplex(65536);
    let client = Session::new(a, Config::default(), Role::Client);
    for header in [
        FrameHeader::open(2),
        FrameHeader::close(2, CloseReason::Normal),
        FrameHeader::open(2),
    ] {
        raw.write_all(&header.encode()).await.unwrap();
    }
    eventually(|| client.is_closed()).await;
}
