//! End-to-end tests over an in-memory duplex pipe.
//!
//! These drive two real sessions against each other, so they cover the parts
//! unit tests cannot: the read and write loops, stream registry bookkeeping, and
//! whether flow control actually stops a sender.

use std::sync::Arc;
use std::time::Duration;

use felucca_mux::frame::MAX_SEGMENT;
use felucca_mux::{CloseReason, Config, Role, Session};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A connected pair of sessions over a duplex pipe.
fn pair(config: Config) -> (Session, Session) {
    // 64 KiB each way: enough that the pipe is not the bottleneck under test,
    // small enough that a runaway writer still blocks rather than buffering
    // without bound.
    let (a, b) = tokio::io::duplex(64 * 1024);
    (
        Session::new(a, config.clone(), Role::Client),
        Session::new(b, config, Role::Server),
    )
}

fn timeout<F: std::future::Future>(f: F) -> tokio::time::Timeout<F> {
    tokio::time::timeout(Duration::from_secs(10), f)
}

#[tokio::test]
async fn a_stream_carries_data_to_the_peer() {
    let (client, mut server) = pair(Config::default());

    let mut out = client.open().expect("open should succeed");
    out.write_all(b"hello felucca").await.unwrap();
    out.flush().await.unwrap();

    let mut inbound = timeout(server.accept())
        .await
        .expect("accept timed out")
        .unwrap();
    let mut buf = [0u8; 13];
    timeout(inbound.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf, b"hello felucca");
}

#[tokio::test]
async fn data_flows_in_both_directions() {
    let (client, mut server) = pair(Config::default());

    let echo = tokio::spawn(async move {
        let mut s = server.accept().await.unwrap();
        let mut buf = vec![0u8; 5];
        s.read_exact(&mut buf).await.unwrap();
        s.write_all(b"world").await.unwrap();
        s.flush().await.unwrap();
        // Hold the stream open until the client has read the reply.
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let mut s = client.open().unwrap();
    s.write_all(b"hello").await.unwrap();
    s.flush().await.unwrap();

    let mut reply = [0u8; 5];
    timeout(s.read_exact(&mut reply)).await.unwrap().unwrap();
    assert_eq!(&reply, b"world");
    echo.await.unwrap();
}

#[tokio::test]
async fn a_bulk_transfer_arrives_intact() {
    // Larger than one window and one segment, so this exercises segmentation,
    // credit exhaustion and credit return together.
    const LEN: usize = 4 * 1024 * 1024;
    let (client, mut server) = pair(Config::default());

    let reader = tokio::spawn(async move {
        let mut s = server.accept().await.unwrap();
        let mut got = Vec::with_capacity(LEN);
        s.read_to_end(&mut got).await.unwrap();
        got
    });

    let payload: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
    let mut s = client.open().unwrap();
    s.write_all(&payload).await.unwrap();
    s.shutdown().await.unwrap();
    drop(s);

    let got = timeout(reader).await.expect("transfer timed out").unwrap();
    assert_eq!(got.len(), LEN, "short read");
    assert_eq!(got, payload, "payload corrupted in transit");
}

#[tokio::test]
async fn a_bulk_backlog_does_not_hide_another_stream_or_overtake_half_close() {
    use felucca_mux::{FrameHeader, FrameType, HEADER_LEN};

    // Hold the transport below one frame until both streams have queued data.
    // A FIFO writer makes the small reply wait for the entire bulk payload.
    const LEN: usize = 512 * 1024;
    let (io, mut raw) = tokio::io::duplex(32);
    let session = Session::new(io, Config::default(), Role::Client);
    let mut bulk = session.open().unwrap();
    let mut small = session.open().unwrap();
    let mut opened = Vec::new();
    while opened.len() < 2 {
        let mut bytes = [0; HEADER_LEN];
        timeout(raw.read_exact(&mut bytes)).await.unwrap().unwrap();
        let header = FrameHeader::decode(&bytes).unwrap();
        match header.ty {
            FrameType::Open => {
                opened.push(header.stream);
                raw.write_all(&FrameHeader::open_ack(header.stream, LEN as u32).encode())
                    .await
                    .unwrap();
            }
            FrameType::Ping => {
                raw.write_all(&FrameHeader::pong(header.value).encode())
                    .await
                    .unwrap();
            }
            other => panic!("unexpected setup frame: {other:?}"),
        }
    }
    assert_eq!(opened, [bulk.id(), small.id()], "OPEN order changed");
    let payload: Vec<_> = (0..LEN).map(|i| (i % 251) as u8).collect();
    let bulk_id = bulk.id();
    let sending = payload.clone();
    let writer = tokio::spawn(async move {
        bulk.write_all(&sending).await.unwrap();
        bulk.shutdown().await.unwrap();
        bulk
    });
    tokio::task::yield_now().await;
    assert!(
        !writer.is_finished(),
        "an unread transport must backpressure bulk writes"
    );
    small.write_all(b"reply").await.unwrap();
    small.shutdown().await.unwrap();

    let mut got = Vec::new();
    let mut before_reply = None;
    let mut ended = Vec::new();
    timeout(async {
        while ended.len() < 2 {
            let mut bytes = [0; HEADER_LEN];
            raw.read_exact(&mut bytes).await.unwrap();
            let header = FrameHeader::decode(&bytes).unwrap();
            let mut data = vec![0; header.payload_len()];
            raw.read_exact(&mut data).await.unwrap();
            match header.ty {
                FrameType::Data if header.stream == bulk_id => {
                    assert!(!ended.contains(&bulk_id), "DATA followed half-close");
                    got.extend_from_slice(&data);
                    raw.write_all(&FrameHeader::window_update(bulk_id, data.len() as u32).encode())
                        .await
                        .unwrap();
                }
                FrameType::Data if header.stream == small.id() => {
                    assert!(!ended.contains(&small.id()), "reply followed half-close");
                    assert_eq!(data, b"reply");
                    before_reply = Some(got.len());
                }
                FrameType::CloseWrite => ended.push(header.stream),
                FrameType::Ping => {
                    raw.write_all(&FrameHeader::pong(header.value).encode())
                        .await
                        .unwrap();
                }
                other => panic!("unexpected transfer frame: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    let _bulk = timeout(writer).await.unwrap().unwrap();
    assert_eq!(got, payload, "bulk payload changed");
    assert!(
        before_reply.is_some_and(|n| n <= 128 * 1024),
        "small reply waited behind {before_reply:?} bulk bytes"
    );
}

#[tokio::test]
async fn flow_control_stops_a_sender_whose_reader_stalls() {
    // One window's worth of credit, and nobody reading: the writer must block
    // rather than buffer without bound.
    let config = Config {
        initial_stream_window: MAX_SEGMENT as u32,
        max_stream_window: MAX_SEGMENT as u32,
        ..Default::default()
    };
    let (client, mut server) = pair(config);

    // Accept but deliberately do not read.
    let held = timeout(async {
        let mut s = client.open().unwrap();
        s.write_all(&[7u8; MAX_SEGMENT]).await.unwrap();
        let inbound = server.accept().await.unwrap();
        (s, inbound)
    })
    .await
    .expect("initial window should go through");

    let (mut s, mut inbound) = held;

    // The window is spent; this write cannot complete until credit returns.
    let blocked = tokio::spawn(async move {
        s.write_all(&[9u8; MAX_SEGMENT]).await.unwrap();
        s
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !blocked.is_finished(),
        "sender must block once its credit is spent"
    );

    // Draining the reader returns credit and unblocks the writer.
    let mut sink = vec![0u8; MAX_SEGMENT];
    inbound.read_exact(&mut sink).await.unwrap();
    assert_eq!(sink, vec![7u8; MAX_SEGMENT]);

    // Credit return must restart progress; the sender can still pace the
    // remainder, so keep draining rather than requiring a whole window to be
    // buffered before the next read.
    let _s = timeout(async {
        inbound.read_exact(&mut sink).await.unwrap();
        assert_eq!(sink, vec![9u8; MAX_SEGMENT]);
        blocked.await.unwrap()
    })
    .await
    .expect("credit return should unblock");
}

#[tokio::test]
async fn many_streams_run_concurrently_without_interfering() {
    const STREAMS: u32 = 64;
    let (client, mut server) = pair(Config::default());

    let acceptor = tokio::spawn(async move {
        let mut tasks = Vec::new();
        for _ in 0..STREAMS {
            let mut s = server.accept().await.unwrap();
            tasks.push(tokio::spawn(async move {
                // Echo the four-byte tag back.
                let mut tag = [0u8; 4];
                s.read_exact(&mut tag).await.unwrap();
                s.write_all(&tag).await.unwrap();
                s.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(300)).await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
    });

    let handle = client.handle();
    let mut writers = Vec::new();
    for i in 0..STREAMS {
        let mut s = handle.open().unwrap();
        writers.push(tokio::spawn(async move {
            s.write_all(&i.to_be_bytes()).await.unwrap();
            s.flush().await.unwrap();
            let mut back = [0u8; 4];
            s.read_exact(&mut back).await.unwrap();
            assert_eq!(
                u32::from_be_bytes(back),
                i,
                "reply landed on the wrong stream"
            );
        }));
    }
    for w in writers {
        timeout(w).await.expect("stream timed out").unwrap();
    }
    timeout(acceptor)
        .await
        .expect("acceptor timed out")
        .unwrap();
}

#[tokio::test]
async fn a_half_close_ends_the_read_side_without_killing_the_write_side() {
    // The `nc -N` / HTTP-request-body case: one side signals end-of-input and
    // still expects a reply.
    let (client, mut server) = pair(Config::default());

    let responder = tokio::spawn(async move {
        let mut s = server.accept().await.unwrap();
        let mut got = Vec::new();
        // Returns once the peer's shutdown arrives, not when the stream dies.
        s.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"request");
        s.write_all(b"response").await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let mut s = client.open().unwrap();
    s.write_all(b"request").await.unwrap();
    s.shutdown().await.unwrap();

    let mut reply = Vec::new();
    timeout(s.read_to_end(&mut reply)).await.unwrap().unwrap();
    assert_eq!(
        reply, b"response",
        "the response direction must survive a half-close"
    );
    timeout(responder).await.unwrap().unwrap();
}

#[tokio::test]
async fn writing_after_a_shutdown_is_refused() {
    let (client, mut server) = pair(Config::default());
    let _keep = tokio::spawn(async move { server.accept().await.map(|s| Box::leak(Box::new(s))) });

    let mut s = client.open().unwrap();
    s.shutdown().await.unwrap();
    assert!(
        s.write_all(b"late").await.is_err(),
        "a finished writer must stay finished"
    );
}

#[tokio::test]
async fn dropping_a_stream_tells_the_peer() {
    let (client, mut server) = pair(Config::default());

    let s = client.open().unwrap();
    let mut inbound = timeout(server.accept()).await.unwrap().unwrap();
    drop(s);

    // The peer's read must end rather than hang forever on a stream nobody owns.
    // A plain drop is an orderly close, like TCP `close()`, so this is EOF.
    let mut buf = Vec::new();
    let n = timeout(inbound.read_to_end(&mut buf))
        .await
        .expect("read should not hang")
        .unwrap();
    assert_eq!(n, 0, "nothing was written before the drop");
}

#[tokio::test]
async fn the_stream_registry_does_not_leak() {
    let (client, mut server) = pair(Config::default());
    let acceptor = tokio::spawn(async move {
        for _ in 0..32 {
            let s = server.accept().await.unwrap();
            drop(s);
        }
        server
    });

    for _ in 0..32 {
        let mut s = client.open().unwrap();
        // Best-effort: the peer may already have dropped its end.
        let _ = s.write_all(b"x").await;
        s.shutdown().await.ok();
        drop(s);
    }
    let server = timeout(acceptor).await.unwrap().unwrap();

    // Give the close frames time to land on both sides.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(client.stream_count(), 0, "client leaked stream entries");
    assert_eq!(server.stream_count(), 0, "server leaked stream entries");
}

#[tokio::test]
async fn the_stream_limit_is_enforced() {
    let config = Config {
        max_streams: 4,
        ..Default::default()
    };
    let (client, mut server) = pair(config);

    // Hold the streams open so they count against the limit.
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(client.open().expect("within the limit"));
    }
    let err = client.open().expect_err("the fifth open must be refused");
    assert!(err.to_string().contains("refused"), "{err}");

    // The peer's side accepted exactly four.
    let mut accepted = Vec::new();
    for _ in 0..4 {
        accepted.push(timeout(server.accept()).await.unwrap().unwrap());
    }
    assert_eq!(server.stream_count(), 4);
}

#[tokio::test]
async fn both_sides_can_open_at_once_without_colliding() {
    // Parity in the id space is what makes this safe without coordination.
    let (mut client, mut server) = pair(Config::default());

    let from_client = client.open().unwrap();
    let from_server = server.open().unwrap();
    assert_ne!(
        from_client.id(),
        from_server.id(),
        "id spaces must not overlap"
    );
    assert_eq!(from_client.id() % 2, 1, "client ids are odd");
    assert_eq!(from_server.id() % 2, 0, "server ids are even");

    let a = timeout(server.accept()).await.unwrap().unwrap();
    let b = timeout(client.accept()).await.unwrap().unwrap();
    assert_eq!(a.id(), from_client.id());
    assert_eq!(b.id(), from_server.id());
}

#[tokio::test]
async fn a_go_away_closes_the_session_and_its_streams() {
    let (client, mut server) = pair(Config::default());

    let mut s = client.open().unwrap();
    s.write_all(b"data").await.unwrap();
    let mut inbound = timeout(server.accept()).await.unwrap().unwrap();

    server.close(CloseReason::Shutdown);

    // A reader on the far side must be released, not left parked.
    let mut buf = Vec::new();
    timeout(s.read_to_end(&mut buf))
        .await
        .expect("reader must be woken")
        .ok();

    // And no new stream can be opened on a closed session.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        client.is_closed(),
        "the peer's go-away should have closed us"
    );
    assert!(
        client.open().is_err(),
        "a closed session must refuse new streams"
    );

    let mut sink = Vec::new();
    let _ = inbound.read_to_end(&mut sink).await;
}

#[tokio::test]
async fn a_transport_failure_wakes_every_blocked_stream() {
    // The case that hangs a naive implementation: the socket dies while a reader
    // is parked, and nothing ever wakes it.
    let (a, b) = tokio::io::duplex(64 * 1024);
    let client = Session::new(a, Config::default(), Role::Client);
    let mut server = Session::new(b, Config::default(), Role::Server);

    let mut s = client.open().unwrap();
    s.write_all(b"x").await.unwrap();
    let inbound = timeout(server.accept()).await.unwrap().unwrap();

    let parked = tokio::spawn(async move {
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Drop the server end entirely: its half of the pipe closes.
    drop(inbound);
    drop(server);

    timeout(parked)
        .await
        .expect("a dead transport must wake its readers")
        .unwrap()
        .ok();
}

#[tokio::test]
async fn keepalive_measures_a_round_trip() {
    let config = Config {
        keepalive_interval: Duration::from_millis(50),
        keepalive_timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let (client, _server) = pair(config);

    // Both sessions answer each other's pings, so an RTT should appear promptly.
    for _ in 0..40 {
        if client.rtt().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        client.rtt().is_some(),
        "a ping should have been answered by now"
    );
    assert!(
        !client.is_closed(),
        "a healthy session must not be killed by keepalive"
    );
}

#[tokio::test]
async fn a_silent_peer_is_eventually_declared_dead() {
    // A peer that never answers a ping: the session must notice rather than
    // hold a dead connection open forever.
    let (a, _b) = tokio::io::duplex(64 * 1024);
    let client = Session::new(
        a,
        Config {
            keepalive_interval: Duration::from_millis(50),
            keepalive_timeout: Duration::from_millis(150),
            ..Default::default()
        },
        Role::Client,
    );

    // `_b` is held but never serviced, so pings go unanswered.
    for _ in 0..60 {
        if client.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        client.is_closed(),
        "an unanswered ping should have closed the session"
    );
}

#[tokio::test]
async fn inbound_progress_keeps_a_delayed_pong_from_killing_the_session() {
    use felucca_mux::FrameHeader;

    let (io, mut raw) = tokio::io::duplex(64 * 1024);
    let mut session = Session::new(
        io,
        Config {
            keepalive_interval: Duration::from_millis(50),
            keepalive_timeout: Duration::from_millis(150),
            ..Default::default()
        },
        Role::Client,
    );
    raw.write_all(&FrameHeader::open(2).encode()).await.unwrap();
    let mut stream = timeout(session.accept()).await.unwrap().unwrap();
    // The peer's pong sits behind a slow DATA backlog. Valid incoming DATA
    // still proves it is alive. Afterwards, stop all traffic to test detection.
    for byte in 0..12 {
        tokio::time::sleep(Duration::from_millis(30)).await;
        raw.write_all(&FrameHeader::data(2, 1).encode())
            .await
            .unwrap();
        raw.write_all(&[byte]).await.unwrap();
        assert_eq!(timeout(stream.read_u8()).await.unwrap().unwrap(), byte);
        assert!(!session.is_closed(), "a progressing peer was timed out");
    }
    timeout(async {
        while !session.is_closed() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("an idle peer still needs to time out");
}

#[tokio::test]
async fn a_stream_survives_being_moved_between_tasks() {
    // Streams are used inside spawned tasks throughout the server, so they must
    // be Send.
    fn assert_send<T: Send>() {}
    assert_send::<felucca_mux::Stream>();

    let (client, mut server) = pair(Config::default());
    let s = Arc::new(tokio::sync::Mutex::new(client.open().unwrap()));

    let writer = tokio::spawn({
        let s = Arc::clone(&s);
        async move {
            let mut g = s.lock().await;
            g.write_all(b"moved").await.unwrap();
            g.flush().await.unwrap();
        }
    });
    writer.await.unwrap();

    let mut inbound = timeout(server.accept()).await.unwrap().unwrap();
    let mut buf = [0u8; 5];
    timeout(inbound.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf, b"moved");
}
