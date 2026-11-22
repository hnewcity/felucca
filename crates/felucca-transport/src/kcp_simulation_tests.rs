//! Drive the real KCP/FEC task through bounded virtual links, without sockets.
use super::*;
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Clone, Copy)]
struct Conditions {
    rate: u64,
    delay_ms: u64,
    lose_every: u64,
    reorder_every: u64,
}

const WAN: Conditions = Conditions {
    rate: 1024 * 1024,
    delay_ms: 70,
    lose_every: 101,
    reorder_every: 19,
};

struct Link {
    conditions: Conditions,
    pending: BTreeMap<(Instant, u64), Vec<u8>>,
    bytes: usize,
    next_send: Instant,
    sequence: u64,
    dropped: usize,
    delivered: usize,
    wake: Arc<tokio::sync::Notify>,
}

impl Link {
    fn new(conditions: Conditions) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            conditions,
            pending: BTreeMap::new(),
            bytes: 0,
            next_send: Instant::now(),
            sequence: 0,
            dropped: 0,
            delivered: 0,
            wake: Arc::new(tokio::sync::Notify::new()),
        }))
    }

    // Finite buffering and deterministic loss/reordering; recovery uses the
    // actual KCP/FEC driver rather than a substitute reliable channel.
    fn send(&mut self, packet: &[u8]) -> io::Result<usize> {
        self.sequence += 1;
        if (self.conditions.lose_every != 0
            && self.sequence.is_multiple_of(self.conditions.lose_every))
            || self.bytes + packet.len() > 256 * 1024
        {
            self.dropped += 1;
            return Ok(packet.len());
        }
        self.next_send = self.next_send.max(Instant::now())
            + Duration::from_nanos(packet.len() as u64 * 1_000_000_000 / self.conditions.rate);
        let reorder = if self.conditions.reorder_every != 0
            && self.sequence.is_multiple_of(self.conditions.reorder_every)
        {
            15
        } else {
            0
        };
        let due = self.next_send + Duration::from_millis(self.conditions.delay_ms + reorder);
        self.pending.insert((due, self.sequence), packet.to_vec());
        self.bytes += packet.len();
        self.wake.notify_one();
        Ok(packet.len())
    }
}

async fn forward(link: Arc<Mutex<Link>>, input: mpsc::Sender<Vec<u8>>) {
    loop {
        let (wake, deadline) = {
            let mut state = link.lock().unwrap();
            while state
                .pending
                .first_key_value()
                .is_some_and(|(key, _)| key.0 <= Instant::now())
            {
                let (_, packet) = state.pending.pop_first().unwrap();
                state.bytes -= packet.len();
                match input.try_send(packet) {
                    Ok(()) => state.delivered += 1,
                    Err(mpsc::error::TrySendError::Full(_)) => state.dropped += 1,
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                }
            }
            (
                state.wake.clone(),
                state.pending.first_key_value().map(|(key, _)| key.0),
            )
        };
        tokio::select! {
            () = wake.notified() => {},
            () = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {},
        }
    }
}

async fn transfer(config: KcpConfig, ab_conditions: Conditions, ba_conditions: Conditions) {
    transfer_with_mux(config, ab_conditions, ba_conditions, false).await;
}

async fn transfer_with_mux(
    config: KcpConfig,
    ab_conditions: Conditions,
    ba_conditions: Conditions,
    multiplex: bool,
) {
    let a_to_b = Link::new(ab_conditions);
    let b_to_a = Link::new(ba_conditions);
    let (a_tx, a_rx) = mpsc::channel(INPUT_QUEUE);
    let (b_tx, b_rx) = mpsc::channel(INPUT_QUEUE);
    let forward_ab = tokio::spawn(forward(a_to_b.clone(), b_tx));
    let forward_ba = tokio::spawn(forward(b_to_a.clone(), a_tx));
    let (client, app_a) = tokio::io::duplex(PIPE_SIZE);
    let (server, app_b) = tokio::io::duplex(PIPE_SIZE);
    let send_a = a_to_b.clone();
    let send_b = b_to_a.clone();
    let driver_a = tokio::spawn(drive_session(
        config.clone(),
        42,
        app_a,
        Input::Queue(a_rx),
        move |packet| send_a.lock().unwrap().send(packet),
        CancellationToken::new(),
    ));
    let driver_b = tokio::spawn(drive_session(
        config,
        42,
        app_b,
        Input::Queue(b_rx),
        move |packet| send_b.lock().unwrap().send(packet),
        CancellationToken::new(),
    ));
    let (mut client, mut server, sessions): (crate::BoxedConn, crate::BoxedConn, _) = if multiplex {
        let a = felucca_mux::Session::new(
            client,
            felucca_mux::Config::default(),
            felucca_mux::Role::Client,
        );
        let mut b = felucca_mux::Session::new(
            server,
            felucca_mux::Config::default(),
            felucca_mux::Role::Server,
        );
        let client = a.open().unwrap();
        let server = tokio::time::timeout(Duration::from_secs(10), b.accept())
            .await
            .unwrap()
            .unwrap();
        (Box::new(client), Box::new(server), Some((a, b)))
    } else {
        (Box::new(client), Box::new(server), None)
    };
    let payload: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let expected = payload.clone();
    let (done, hold) = tokio::sync::oneshot::channel();
    let (uploaded, upload_received) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let mut received = Vec::with_capacity(expected.len());
        if multiplex {
            // The public upload backend only replies after the request FIN.
            server.read_to_end(&mut received).await.unwrap();
        } else {
            received.resize(expected.len(), 0);
            server.read_exact(&mut received).await.unwrap();
        }
        assert_eq!(received, expected);
        let _ = uploaded.send(());
        server.write_all(&received).await.unwrap();
        if multiplex {
            server.shutdown().await.unwrap();
        }
        let _ = hold.await;
    });
    let start = Instant::now();
    let mut upload_elapsed = None;
    // Preserve the stricter raw-driver 120s TOTAL regression. The additional
    // mux test mirrors the public probe: each direction independently <=120s.
    let total_limit = Duration::from_secs(if multiplex { 240 } else { 120 });
    let transfer = tokio::time::timeout(total_limit, async {
        tokio::time::timeout(Duration::from_secs(120), async {
            client.write_all(&payload).await.unwrap();
            if multiplex {
                client.shutdown().await.unwrap();
            }
            upload_received.await.unwrap();
        })
        .await?;
        upload_elapsed = Some(start.elapsed());
        tokio::time::timeout(Duration::from_secs(120), async {
            let mut received = Vec::with_capacity(payload.len());
            if multiplex {
                client.read_to_end(&mut received).await.unwrap();
            } else {
                received.resize(payload.len(), 0);
                client.read_exact(&mut received).await.unwrap();
            }
            assert_eq!(received, payload);
        })
        .await?;
        Ok::<_, tokio::time::error::Elapsed>(())
    })
    .await;
    let stats = |link: &Arc<Mutex<Link>>| {
        let state = link.lock().unwrap();
        (state.delivered, state.dropped, state.bytes)
    };
    let ab = stats(&a_to_b);
    let ba = stats(&b_to_a);
    eprintln!(
        "KCP simulation mux={multiplex} upload={upload_elapsed:?} elapsed={:?}, ab={ab:?}, ba={ba:?}",
        start.elapsed()
    );
    let _ = done.send(());
    peer.abort();
    let _ = peer.await;
    drop(sessions);
    for task in [driver_a, driver_b] {
        task.abort();
        let _ = task.await;
    }
    for task in [forward_ab, forward_ba] {
        task.abort();
        let _ = task.await;
    }
    assert!(
        matches!(transfer, Ok(Ok(()))),
        "4 MiB each direction timed out after {:?}; ab={ab:?}, ba={ba:?}",
        start.elapsed()
    );
    if ab_conditions.lose_every != 0 {
        assert!(ab.1 > 0);
    }
    if ba_conditions.lose_every != 0 {
        assert!(ba.1 > 0);
    }
}

#[tokio::test(start_paused = true)]
async fn large_transfers_cross_a_delayed_lossy_link_in_both_directions() {
    transfer(KcpConfig::default(), WAN, WAN).await;
}

#[tokio::test(start_paused = true)]
async fn large_transfers_cross_a_lossless_link_with_delay() {
    let link = Conditions {
        lose_every: 0,
        reorder_every: 0,
        ..WAN
    };
    transfer(KcpConfig::default(), link, link).await;
}

#[tokio::test(start_paused = true)]
async fn large_transfers_cross_an_asymmetric_link_without_fec() {
    let config = KcpConfig {
        parity_shards: 0,
        ..KcpConfig::default()
    };
    let upload = Conditions {
        rate: 128 * 1024,
        delay_ms: 30,
        reorder_every: 0,
        ..WAN
    };
    let download = Conditions {
        delay_ms: 30,
        reorder_every: 0,
        ..WAN
    };
    transfer(config, upload, download).await;
}

#[tokio::test(start_paused = true)]
async fn a_blackholed_writer_is_bounded_and_cancellation_joins_the_driver() {
    let (mut application, driver_io) = tokio::io::duplex(PIPE_SIZE);
    let (input, receive) = mpsc::channel(INPUT_QUEUE);
    let close = CancellationToken::new();
    let driver = tokio::spawn(drive_session(
        KcpConfig::default(),
        42,
        driver_io,
        Input::Queue(receive),
        |packet| Ok(packet.len()),
        close.clone(),
    ));
    let writer =
        tokio::spawn(async move { application.write_all(&vec![42; 4 * 1024 * 1024]).await });
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !writer.is_finished(),
        "an unresponsive peer must backpressure a bulk writer"
    );
    close.cancel();
    let result = tokio::time::timeout(Duration::from_secs(6), driver)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert!(
        writer.await.unwrap().is_err(),
        "shutdown must wake the application writer"
    );
    assert!(
        input.is_closed(),
        "the driver must release its packet queue"
    );
}

#[tokio::test(start_paused = true)]
async fn a_silent_session_expires_and_releases_both_ends() {
    let (mut application, driver_io) = tokio::io::duplex(PIPE_SIZE);
    let (input, receive) = mpsc::channel(INPUT_QUEUE);
    let config = KcpConfig::default();
    let deadline = config.session_timeout + Duration::from_secs(1);
    let driver = tokio::spawn(drive_session(
        config,
        42,
        driver_io,
        Input::Queue(receive),
        |packet| Ok(packet.len()),
        CancellationToken::new(),
    ));
    let result = tokio::time::timeout(deadline, driver)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert!(input.is_closed());
    assert_eq!(application.read(&mut [0; 1]).await.unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn multiplexed_transfers_cross_the_lossy_kcp_link_within_the_public_deadline() {
    transfer_with_mux(KcpConfig::default(), WAN, WAN, true).await;
}
