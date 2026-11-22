use super::*;
use crate::test_dns as dns;
use std::time::Duration;
#[path = "../tests/support/ntp.rs"]
mod ntp;

fn config(server: SocketAddr) -> NtpConfig {
    NtpConfig {
        server: server.to_string(),
        interval: Duration::from_secs(1),
        timeout: Duration::from_millis(200),
        max_offset: Duration::from_secs(3600),
    }
}

#[test]
fn ntp_era_and_four_timestamp_offset_preserve_subsecond_precision() {
    for seconds in [
        0i128,
        1_789_520_400,
        2_085_978_496,
        2_085_978_497,
        4_500_000_000,
    ] {
        let nanos = seconds * SECOND + 876_543_210;
        assert!((decode(&encode(nanos), nanos) - nanos).abs() <= 1);
    }
    let config = config("127.0.0.1:123".parse().unwrap());
    let t1 = 1_789_520_400 * SECOND;
    let t4 = t1 + SECOND / 10;
    let mut packet = [0; 48];
    packet[0] = (4 << 3) | 4;
    packet[1] = 1;
    packet[32..40].copy_from_slice(&encode(t1 + 500 * SECOND + SECOND / 50));
    packet[40..48].copy_from_slice(&encode(t1 + 500 * SECOND + SECOND / 50));
    assert!(
        (offset(&packet, t1, t4, &config).unwrap() - (500 * SECOND - SECOND * 3 / 100)).abs() <= 1
    );
    for (at, value) in [(0, 0xe4), (0, 0x23), (0, 0x14), (1, 0), (1, 16), (8, 255)] {
        let mut invalid = packet;
        invalid[at] = value;
        assert!(offset(&invalid, t1, t4, &config).is_err(), "{at}={value}");
    }
    let mut impossible = packet;
    impossible[32..40].copy_from_slice(&encode(t1 + 600 * SECOND));
    assert!(offset(&impossible, t1, t4, &config).is_err());
    let mut bounded = config;
    bounded.max_offset = Duration::from_secs(10);
    assert!(offset(&packet, t1, t4, &bounded).is_err());
}

#[tokio::test]
async fn sntp_ignores_stale_packets_and_expires_cached_time_without_altering_system_clock() {
    let ntp = ntp::Ntp::start(600, true).await;
    let clock = Clock::new(Some(config(ntp.address)));
    let before = system_nanos() / SECOND;
    clock.synchronize(&Network::default()).await.unwrap();
    assert!((clock.now_unix() as i128 - before - 600).abs() <= 1);
    assert!((system_nanos() / SECOND - before).abs() <= 1);
    assert_eq!(ntp.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!((clock.now_unix() as i128 - before - 602).abs() <= 1);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!((clock.now_unix() as i128 - system_nanos() / SECOND).abs() <= 1);
}

#[tokio::test]
async fn failed_refresh_preserves_recent_sample_and_pending_refresh_is_cancellable() {
    let ntp = ntp::Ntp::start(-600, false).await;
    let clock = Arc::new(Clock::new(Some(config(ntp.address))));
    let network = Arc::new(Network::default());
    clock.synchronize(&network).await.unwrap();
    let before = clock.now_unix();
    drop(ntp);
    assert!(clock.synchronize(&network).await.is_err());
    assert!((clock.now_unix() - before).abs() <= 1);

    let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut cfg = config(silent.local_addr().unwrap());
    cfg.interval = Duration::from_secs(40);
    cfg.timeout = Duration::from_secs(30);
    let clock = Arc::new(Clock::new(Some(cfg)));
    let cancel = CancellationToken::new();
    let task = tokio::spawn(clock.refresh(network, cancel.clone()));
    tokio::time::pause();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(41)).await;
    let mut packet = [0; 48];
    silent.recv_from(&mut packet).await.unwrap();
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(100), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn clock_refresh_uses_custom_dns_and_bounds_excessive_offsets() {
    let dns = dns::Dns::start(false).await;
    let ntp = ntp::Ntp::start(600, false).await;
    let network = Network::new(
        &felucca_core::config::network::NetworkConfig {
            dns: vec![dns.address],
            local_ip: Some("127.0.0.1".parse().unwrap()),
            ..Default::default()
        },
        Duration::from_secs(1),
    )
    .unwrap();
    let mut config = config(ntp.address);
    config.server = format!("ntp.test:{}", ntp.address.port());
    config.max_offset = Duration::from_secs(60);
    let clock = Clock::new(Some(config));
    assert!(clock.synchronize(&network).await.is_err());
    assert!((clock.now_unix() as i128 - system_nanos() / SECOND).abs() <= 1);
    assert!(
        dns.hits
            .lock()
            .unwrap()
            .iter()
            .any(|name| name == "ntp.test")
    );
    assert_eq!(dns.tcp_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}
