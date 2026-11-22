//! The data path: copying bytes while metering and pacing them.

use std::sync::Arc;

use felucca_core::{RateLimiter, TrafficCounter};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Buffer size for one copy direction.
///
/// Matches the mux segment size, so a full read becomes exactly one frame
/// instead of being split across two.
const BUF_LEN: usize = 16 * 1024;

async fn copy_scoped<R, W>(
    reader: &mut R,
    writer: &mut W,
    direction: Direction,
    resources: &crate::resources::MeterGroup,
) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0; BUF_LEN];
    let mut total = 0;
    loop {
        let size = reader.read(&mut buffer).await?;
        if size == 0 {
            break;
        }
        resources.pace(size as u64).await?;
        let mut offset = 0;
        while offset < size {
            let count = writer.write(&buffer[offset..size]).await?;
            if count == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            // Account each successful write, including a partial write followed
            // by an error; write_all would hide that transferred prefix.
            resources.record(direction, count as u64);
            offset += count;
            total += count as u64;
        }
    }
    writer.shutdown().await?;
    Ok(total)
}

pub async fn splice_scoped<A, B>(
    public: &mut A,
    tunnel: &mut B,
    resources: &crate::resources::MeterGroup,
) -> std::io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut public_read, mut public_write) = tokio::io::split(public);
    let (mut tunnel_read, mut tunnel_write) = tokio::io::split(tunnel);
    tokio::try_join!(
        copy_scoped(
            &mut public_read,
            &mut tunnel_write,
            Direction::Inlet,
            resources
        ),
        copy_scoped(
            &mut tunnel_read,
            &mut public_write,
            Direction::Export,
            resources
        ),
    )
}

/// Which way bytes are moving, from the tunnel's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the public side toward the agent.
    Inlet,
    /// From the agent back toward the public side.
    Export,
}

/// Copies one direction, counting bytes and pacing them.
///
/// Returns the bytes copied. Accounting happens as data moves rather than at the
/// end, so a connection that runs for hours is still visible in the dashboard,
/// and a quota is enforced while it is being exceeded rather than afterwards.
pub async fn copy_metered<R, W>(
    reader: &mut R,
    writer: &mut W,
    direction: Direction,
    traffic: &TrafficCounter,
    rate: &RateLimiter,
) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut buf = vec![0u8; BUF_LEN];
    let mut total = 0u64;

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }

        // Pace before writing, not after: charging afterwards lets a burst
        // through in full and only delays the next one.
        if let Some(wait) = rate.take(n as u64) {
            tokio::time::sleep(wait).await;
        }

        writer.write_all(&buf[..n]).await?;

        let n = n as u64;
        total += n;
        match direction {
            Direction::Inlet => traffic.add_inlet(n),
            Direction::Export => traffic.add_export(n),
        }
    }

    // Tell the peer this direction is done while leaving the other running: a
    // protocol that signals end-of-request by half-closing depends on it.
    let _ = writer.shutdown().await;
    Ok(total)
}

/// Splices two connections together, metering both directions.
///
/// Returns `(inlet, export)` byte counts on success. EOF half-closes one direction
/// while the other drains; an I/O failure cancels both directions so an idle
/// backend and its connection slot cannot outlive a reset public connection.
pub async fn splice<A, B>(
    public: &mut A,
    tunnel: &mut B,
    traffic: Arc<TrafficCounter>,
    rate: Arc<RateLimiter>,
) -> (u64, u64)
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    let (mut public_read, mut public_write) = tokio::io::split(public);
    let (mut tunnel_read, mut tunnel_write) = tokio::io::split(tunnel);

    let copied = tokio::try_join!(
        copy_metered(
            &mut public_read,
            &mut tunnel_write,
            Direction::Inlet,
            &traffic,
            &rate
        ),
        copy_metered(
            &mut tunnel_read,
            &mut public_write,
            Direction::Export,
            &traffic,
            &rate
        ),
    );

    // Each successful write already updated the shared counters. An error can
    // interrupt either direction; its partial count is not available here.
    copied.unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn a_copy_moves_bytes_and_counts_them() {
        let (mut src, mut src_peer) = tokio::io::duplex(64 * 1024);
        let (mut dst_peer, mut dst) = tokio::io::duplex(64 * 1024);

        src_peer.write_all(b"twelve bytes").await.unwrap();
        drop(src_peer);

        let traffic = TrafficCounter::new();
        let rate = RateLimiter::new(0);
        let n = copy_metered(&mut src, &mut dst_peer, Direction::Inlet, &traffic, &rate)
            .await
            .unwrap();

        assert_eq!(n, 12);
        assert_eq!(traffic.stats().inlet, 12);
        assert_eq!(
            traffic.stats().export,
            0,
            "the other direction stays untouched"
        );

        let mut got = Vec::new();
        dst.read_to_end(&mut got).await.unwrap();
        assert_eq!(&got, b"twelve bytes");
    }

    #[tokio::test]
    async fn export_bytes_are_counted_separately() {
        let (mut src, mut src_peer) = tokio::io::duplex(1024);
        let (mut dst_peer, _dst) = tokio::io::duplex(1024);
        src_peer.write_all(b"back").await.unwrap();
        drop(src_peer);

        let traffic = TrafficCounter::new();
        copy_metered(
            &mut src,
            &mut dst_peer,
            Direction::Export,
            &traffic,
            &RateLimiter::new(0),
        )
        .await
        .unwrap();

        assert_eq!(traffic.stats().export, 4);
        assert_eq!(traffic.stats().inlet, 0);
    }

    #[tokio::test]
    async fn a_large_transfer_is_counted_exactly() {
        const LEN: usize = 512 * 1024;
        let (mut src, mut src_peer) = tokio::io::duplex(64 * 1024);
        let (mut dst_peer, mut dst) = tokio::io::duplex(64 * 1024);

        let payload: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        let expected = payload.clone();
        tokio::spawn(async move {
            src_peer.write_all(&payload).await.unwrap();
        });

        let drain = tokio::spawn(async move {
            let mut got = Vec::with_capacity(LEN);
            dst.read_to_end(&mut got).await.unwrap();
            got
        });

        let traffic = TrafficCounter::new();
        let n = copy_metered(
            &mut src,
            &mut dst_peer,
            Direction::Inlet,
            &traffic,
            &RateLimiter::new(0),
        )
        .await
        .unwrap();

        assert_eq!(n as usize, LEN);
        assert_eq!(traffic.stats().inlet as usize, LEN);
        drop(dst_peer);
        assert_eq!(drain.await.unwrap(), expected, "payload corrupted");
    }

    #[tokio::test]
    async fn a_rate_limit_slows_the_copy() {
        // 32 KiB at 16 KiB/s should take about a second beyond the first burst.
        let (mut src, mut src_peer) = tokio::io::duplex(128 * 1024);
        let (mut dst_peer, mut dst) = tokio::io::duplex(128 * 1024);

        src_peer.write_all(&[0u8; 32 * 1024]).await.unwrap();
        drop(src_peer);
        let drain = tokio::spawn(async move {
            let mut sink = Vec::new();
            dst.read_to_end(&mut sink).await.unwrap();
        });

        let started = Instant::now();
        let n = copy_metered(
            &mut src,
            &mut dst_peer,
            Direction::Inlet,
            &TrafficCounter::new(),
            &RateLimiter::new(16 * 1024),
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(n, 32 * 1024);
        assert!(
            elapsed >= Duration::from_millis(500),
            "not actually paced: {elapsed:?}"
        );
        drop(dst_peer);
        drain.await.unwrap();
    }

    #[tokio::test]
    async fn an_unlimited_rate_does_not_delay() {
        let (mut src, mut src_peer) = tokio::io::duplex(128 * 1024);
        let (mut dst_peer, mut dst) = tokio::io::duplex(128 * 1024);
        src_peer.write_all(&[0u8; 64 * 1024]).await.unwrap();
        drop(src_peer);
        let drain = tokio::spawn(async move {
            let mut sink = Vec::new();
            dst.read_to_end(&mut sink).await.unwrap();
        });

        let started = Instant::now();
        copy_metered(
            &mut src,
            &mut dst_peer,
            Direction::Inlet,
            &TrafficCounter::new(),
            &RateLimiter::new(0),
        )
        .await
        .unwrap();
        assert!(started.elapsed() < Duration::from_millis(300));
        drop(dst_peer);
        drain.await.unwrap();
    }

    #[tokio::test]
    async fn a_splice_carries_both_directions_and_reports_each() {
        // Stand in for the public client and the agent-side target.
        let (mut client, client_peer) = tokio::io::duplex(64 * 1024);
        let (target_peer, target) = tokio::io::duplex(64 * 1024);

        let echo = tokio::spawn(async move {
            let mut t = target;
            let mut buf = vec![0u8; 7];
            t.read_exact(&mut buf).await.unwrap();
            t.write_all(b"response!").await.unwrap();
            t.shutdown().await.unwrap();
        });

        let traffic = Arc::new(TrafficCounter::new());
        let rate = Arc::new(RateLimiter::new(0));

        let mut public = client_peer;
        let mut tunnel = target_peer;
        let spliced = tokio::spawn({
            let traffic = Arc::clone(&traffic);
            async move { splice(&mut public, &mut tunnel, traffic, rate).await }
        });

        client.write_all(b"request").await.unwrap();
        client.shutdown().await.unwrap();
        let mut back = Vec::new();
        client.read_to_end(&mut back).await.unwrap();
        assert_eq!(&back, b"response!");

        let (inlet, export) = spliced.await.unwrap();
        assert_eq!(inlet, 7, "request bytes");
        assert_eq!(export, 9, "response bytes");
        assert_eq!(traffic.stats().inlet, 7);
        assert_eq!(traffic.stats().export, 9);
        echo.await.unwrap();
    }

    #[tokio::test]
    async fn one_direction_finishing_does_not_cut_off_the_other() {
        // A client that half-closes after its request still needs the response.
        let (mut client, client_peer) = tokio::io::duplex(64 * 1024);
        let (target_peer, mut target) = tokio::io::duplex(64 * 1024);

        let responder = tokio::spawn(async move {
            let mut buf = Vec::new();
            target.read_to_end(&mut buf).await.unwrap();
            assert_eq!(&buf, b"done asking");
            // The inlet direction has already finished by now.
            target.write_all(b"still answering").await.unwrap();
            target.shutdown().await.unwrap();
        });

        let mut public = client_peer;
        let mut tunnel = target_peer;
        let spliced = tokio::spawn(async move {
            splice(
                &mut public,
                &mut tunnel,
                Arc::new(TrafficCounter::new()),
                Arc::new(RateLimiter::new(0)),
            )
            .await
        });

        client.write_all(b"done asking").await.unwrap();
        client.shutdown().await.unwrap();

        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut back))
            .await
            .expect("the response direction must survive")
            .unwrap();
        assert_eq!(&back, b"still answering");

        let (inlet, export) = spliced.await.unwrap();
        assert_eq!(inlet, 11);
        assert_eq!(export, 15);
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn a_peer_vanishing_mid_copy_is_not_a_panic() {
        let (public, public_peer) = tokio::io::duplex(1024);
        let (tunnel, tunnel_peer) = tokio::io::duplex(1024);
        drop(public);
        drop(tunnel);

        let mut a = public_peer;
        let mut b = tunnel_peer;
        let (inlet, export) = splice(
            &mut a,
            &mut b,
            Arc::new(TrafficCounter::new()),
            Arc::new(RateLimiter::new(0)),
        )
        .await;
        assert_eq!((inlet, export), (0, 0));
    }
}
