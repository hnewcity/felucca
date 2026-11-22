//! Exercise real sessions behind a buffered, rate-limited ordered transport.
use std::time::Duration;

use felucca_mux::{CloseReason, Config, Role, Session};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

async fn relay(source: DuplexStream, destination: DuplexStream) {
    let (mut ar, mut aw) = tokio::io::split(source);
    let (mut br, mut bw) = tokio::io::split(destination);
    async fn copy<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
        read: &mut R,
        write: &mut W,
    ) {
        let mut bytes = [0; 128];
        while let Ok(n) = read.read(&mut bytes).await {
            if n == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_micros(15_625)).await;
            if write.write_all(&bytes[..n]).await.is_err() {
                break;
            }
        }
    }
    tokio::join!(copy(&mut ar, &mut bw), copy(&mut br, &mut aw));
}

#[tokio::test]
async fn four_bulk_streams_echo_sixteen_mib_each_without_stalling() {
    let (a, b) = tokio::io::duplex(256 * 1024);
    let client = Session::new(a, Config::default(), Role::Client);
    let mut server = Session::new(b, Config::default(), Role::Server);
    let echo = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let stream = server.accept().await.unwrap();
            tasks.spawn(async move {
                let (mut read, mut write) = tokio::io::split(stream);
                tokio::io::copy(&mut read, &mut write).await.unwrap();
                write.shutdown().await.unwrap();
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
        server
    });
    let mut transfers = tokio::task::JoinSet::new();
    for offset in 0..4 {
        let stream = client.open().unwrap();
        transfers.spawn(async move {
            let (mut read, mut write) = tokio::io::split(stream);
            let payload: Vec<u8> = (0..16 * 1024 * 1024)
                .map(|i| ((i + offset) % 251) as u8)
                .collect();
            let mut received = Vec::new();
            let (sent, got) = tokio::join!(
                async {
                    write.write_all(&payload).await?;
                    write.shutdown().await
                },
                read.read_to_end(&mut received)
            );
            sent.unwrap();
            got.unwrap();
            assert_eq!(received, payload);
        });
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(result) = transfers.join_next().await {
            result.unwrap();
        }
        let _server = echo.await.unwrap();
    })
    .await
    .expect("concurrent full-duplex bulk transfer stalled");
    assert!(!client.is_closed());
}

#[tokio::test(start_paused = true)]
async fn small_exchanges_finish_during_a_bulk_transfer_at_eight_kib_per_second() {
    slow_transfer(false).await;
    slow_transfer(true).await;
}

async fn slow_transfer(reverse: bool) {
    let (a, ar) = tokio::io::duplex(256 * 1024);
    let (b, br) = tokio::io::duplex(256 * 1024);
    let wire = tokio::spawn(relay(ar, br));
    let a = Session::new(a, Config::default(), Role::Client);
    let b = Session::new(b, Config::default(), Role::Server);
    let (client, mut server) = (a, b);
    let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let expected = payload.clone();
    let mut bulk = client.open().unwrap();
    let bulk_sender = tokio::spawn(async move {
        bulk.write_all(&payload).await.unwrap();
        bulk.shutdown().await.unwrap();
    });
    let mut inbound = server.accept().await.unwrap();
    let bulk_reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        inbound.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, expected);
    });
    // Exercise bulk data in the request direction and in the response direction.
    let (client, mut server) = if reverse {
        (server, client)
    } else {
        (client, server)
    };
    let responder = tokio::spawn(async move {
        while let Ok(mut stream) = server.accept().await {
            tokio::spawn(async move {
                for _ in 0..3 {
                    let mut request = [0; 16];
                    if stream.read_exact(&mut request).await.is_err() {
                        return;
                    }
                    if stream.write_all(&request).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    let mut latencies = Vec::new();
    let mut failures = 0;
    while !bulk_reader.is_finished() {
        let start = tokio::time::Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            let mut stream = client.open().unwrap();
            for _ in 0..3 {
                stream.write_all(&[42; 16]).await.unwrap();
                let mut response = [0; 16];
                stream.read_exact(&mut response).await.unwrap();
                assert_eq!(response, [42; 16]);
            }
            // Route negotiation plus ECHO1's two exchanges and orderly EOF are
            // all part of the original public probe's five-second deadline.
            let mut remainder = Vec::new();
            stream.read_to_end(&mut remainder).await.unwrap();
            assert!(remainder.is_empty());
        })
        .await;
        latencies.push(start.elapsed());
        if result.is_err() {
            failures += 1;
        }
        // A stalled transfer must not leave the test running indefinitely.
        assert!(latencies.len() < 400, "bulk transfer stalled");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bulk_sender.await.unwrap();
    bulk_reader.await.unwrap();
    assert!(!client.is_closed());
    client.close(CloseReason::Normal);
    responder.abort();
    let _ = responder.await;
    wire.abort();
    let _ = wire.await;
    eprintln!(
        "bulk_on_response={reverse}: {} small requests, {failures} timeouts, max {:?}",
        latencies.len(),
        latencies.iter().max()
    );
    assert!(
        latencies.len() >= 20,
        "must exercise requests during the bulk transfer"
    );
    assert_eq!(
        failures, 0,
        "small requests exceeded their original 5s deadline"
    );
}
