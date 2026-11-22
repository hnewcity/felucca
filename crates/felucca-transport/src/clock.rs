//! Bounded SNTP calibration for bridge authentication (RFC 5905).
//! A sample anchors UTC to a monotonic clock. TLS certificate validity, quota
//! expiry, and the operating system clock retain their normal system-time rules.
use crate::network::Network;
use felucca_core::config::NtpConfig;
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{net::UdpSocket, time::Instant};
use tokio_util::sync::CancellationToken;

const SECOND: i128 = 1_000_000_000;
const NTP_EPOCH: i128 = 2_208_988_800;
const ERA: i128 = 1 << 32;

#[derive(Debug)]
struct Sample {
    utc_nanos: i128,
    received: Instant,
    offset_nanos: i128,
}

#[derive(Debug)]
pub struct Clock {
    config: Option<NtpConfig>,
    sample: Mutex<Option<Sample>>,
}
impl Clock {
    pub fn new(config: Option<NtpConfig>) -> Self {
        Self {
            config,
            sample: Mutex::new(None),
        }
    }

    pub fn now_unix(&self) -> i64 {
        let sample = self.sample.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = sample.as_ref().filter(|sample| {
            self.config
                .as_ref()
                .is_some_and(|config| sample.received.elapsed() <= config.interval * 3)
        });
        let nanos = fresh.map_or_else(system_nanos, |sample| {
            sample.utc_nanos + sample.received.elapsed().as_nanos() as i128
        });
        (nanos / SECOND).clamp(0, i64::MAX as i128) as i64
    }

    /// Failure preserves a recent sample; after three intervals it expires.
    pub async fn synchronize(&self, network: &Network) -> io::Result<()> {
        let Some(config) = &self.config else {
            return Ok(());
        };
        let sample = tokio::time::timeout(config.timeout, query(config, network))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "NTP query timed out"))??;
        tracing::info!(
            offset_ms = sample.offset_nanos / 1_000_000,
            "calibrated bridge authentication clock"
        );
        *self.sample.lock().unwrap_or_else(|e| e.into_inner()) = Some(sample);
        Ok(())
    }

    /// Call synchronize once before accepting/authenticating, then own this
    /// refresh future for the runtime's lifetime.
    pub async fn refresh(self: Arc<Self>, network: Arc<Network>, cancel: CancellationToken) {
        let Some(config) = &self.config else {
            return;
        };
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(config.interval) => {},
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                result = self.synchronize(&network) => {
                    if let Err(error) = result {
                        tracing::warn!(%error, "NTP refresh failed; using last fresh sample or system time");
                    }
                }
            }
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn system_nanos() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as i128)
        .unwrap_or(0)
}

/// Era unfolding relative to the local epoch, including the 2036 wrap.
fn decode(stamp: &[u8], near: i128) -> i128 {
    let seconds = u32::from_be_bytes(stamp[..4].try_into().unwrap()) as i128;
    let fraction = u32::from_be_bytes(stamp[4..8].try_into().unwrap()) as i128;
    let near_seconds = near.div_euclid(SECOND) + NTP_EPOCH;
    let era = (near_seconds - seconds + ERA / 2).div_euclid(ERA);
    (seconds + era * ERA - NTP_EPOCH) * SECOND + fraction * SECOND / ERA
}
fn encode(nanos: i128) -> [u8; 8] {
    let seconds = (nanos.div_euclid(SECOND) + NTP_EPOCH).rem_euclid(ERA) as u32;
    let fraction = (nanos.rem_euclid(SECOND) * ERA / SECOND) as u32;
    let mut stamp = [0; 8];
    stamp[..4].copy_from_slice(&seconds.to_be_bytes());
    stamp[4..].copy_from_slice(&fraction.to_be_bytes());
    stamp
}

async fn query(config: &NtpConfig, network: &Network) -> io::Result<Sample> {
    let addresses = network.resolve(&config.server).await?;
    // Race address families; a black-holed IPv6 source must not prevent a
    // reachable IPv4 time source from answering. All work stays within timeout.
    let mut attempts = tokio::task::JoinSet::new();
    for address in addresses.into_iter().take(8) {
        if network
            .local_ip()
            .is_some_and(|ip| ip.is_ipv4() != address.is_ipv4())
        {
            continue;
        }
        let config = config.clone();
        let local = network.local_ip();
        attempts.spawn(async move { query_address(&config, address, local).await });
    }
    let mut last = invalid("no source-compatible NTP address");
    while let Some(result) = attempts.join_next().await {
        match result {
            Ok(Ok(sample)) => return Ok(sample),
            Ok(Err(error)) => last = error,
            Err(error) => last = io::Error::other(error),
        }
    }
    Err(last)
}
async fn query_address(
    config: &NtpConfig,
    address: SocketAddr,
    local: Option<IpAddr>,
) -> io::Result<Sample> {
    let bind = local.unwrap_or(if address.is_ipv4() {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        Ipv6Addr::UNSPECIFIED.into()
    });
    let socket = UdpSocket::bind(SocketAddr::new(bind, 0)).await?;
    socket.connect(address).await?;
    let sent = Instant::now();
    let mut stamp = encode(system_nanos());
    // Keep timestamp semantics while adding unpredictable low bits (<=16us).
    stamp[6..].copy_from_slice(&rand::random::<u16>().to_be_bytes());
    let t1 = decode(&stamp, system_nanos());
    let mut request = [0u8; 48];
    request[0] = (4 << 3) | 3;
    request[40..].copy_from_slice(&stamp);
    socket.send(&request).await?;
    let mut packet = [0u8; 1024];
    loop {
        let count = socket.recv(&mut packet).await?;
        let received = Instant::now();
        // Ignore unrelated/stale datagrams; connected UDP already filters source.
        if count < 48 || packet[24..32] != stamp {
            continue;
        }
        let t4 = system_nanos();
        if ((t4 - t1) - sent.elapsed().as_nanos() as i128).abs() > SECOND / 10 {
            return Err(invalid("system clock changed during NTP exchange"));
        }
        let offset = offset(&packet[..count], t1, t4, config)?;
        return Ok(Sample {
            utc_nanos: t4 + offset,
            received,
            offset_nanos: offset,
        });
    }
}
fn offset(packet: &[u8], t1: i128, t4: i128, config: &NtpConfig) -> io::Result<i128> {
    let version = (packet[0] >> 3) & 7;
    if packet[0] >> 6 == 3
        || !matches!(version, 3 | 4)
        || packet[0] & 7 != 4
        || !(1..=15).contains(&packet[1])
        || packet[32..40] == [0; 8]
        || packet[40..48] == [0; 8]
    {
        return Err(invalid(
            "NTP server is unsynchronized or returned an invalid reply",
        ));
    }
    let root_delay = i32::from_be_bytes(packet[4..8].try_into().unwrap()) as i128;
    let dispersion = u32::from_be_bytes(packet[8..12].try_into().unwrap()) as i128;
    if root_delay.max(0) / 2 + dispersion > 5 * 65536 {
        return Err(invalid("NTP root distance exceeds five seconds"));
    }
    let t2 = decode(&packet[32..40], t1);
    let t3 = decode(&packet[40..48], t4);
    let delay = (t4 - t1) - (t3 - t2);
    if t3 < t2 || delay < -1_000_000 || delay > config.timeout.as_nanos() as i128 {
        return Err(invalid("NTP timestamps imply an invalid round-trip delay"));
    }
    let offset = ((t2 - t1) + (t3 - t4)) / 2;
    if offset.abs() > config.max_offset.as_nanos() as i128 {
        return Err(invalid("NTP offset exceeds max_offset"));
    }
    Ok(offset)
}

#[cfg(test)]
#[path = "clock_tests.rs"]
mod tests;
