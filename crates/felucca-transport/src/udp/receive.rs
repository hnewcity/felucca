use std::{io, net::SocketAddr, time::Duration};

use tokio::{
    net::UdpSocket,
    time::{Instant, sleep_until},
};

const INITIAL_DELAY: Duration = Duration::from_millis(10);
const MAX_DELAY: Duration = Duration::from_secs(1);
const LOG_INTERVAL: Duration = Duration::from_secs(30);

/// Receives datagrams on a shared socket without retiring its loop on one error.
///
/// Windows reports an ICMP port-unreachable for an earlier `send_to` as
/// `ConnectionReset` on the next receive, and an oversized datagram as
/// `WSAEMSGSIZE`. Both consume one queued event and leave the socket usable, so
/// they are skipped at once. Any other error backs off like [`crate::tcp::Acceptor`]
/// so a persistent failure cannot spin. `recv_from` is cancellation safe.
#[derive(Debug)]
pub struct Receiver {
    retry_at: Option<Instant>,
    delay: Duration,
    log_after: Option<Instant>,
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Receiver {
    pub fn new() -> Self {
        Self {
            retry_at: None,
            delay: INITIAL_DELAY,
            log_after: None,
        }
    }

    pub async fn recv_from(
        &mut self,
        socket: &UdpSocket,
        buffer: &mut [u8],
    ) -> (usize, SocketAddr) {
        loop {
            if let Some(deadline) = self.retry_at {
                sleep_until(deadline).await;
            }
            match socket.recv_from(buffer).await {
                Ok(received) => {
                    self.retry_at = None;
                    self.delay = INITIAL_DELAY;
                    return received;
                }
                Err(error) => self.failed(socket, &error),
            }
        }
    }

    fn failed(&mut self, socket: &UdpSocket, error: &io::Error) {
        let now = Instant::now();
        let transient = per_datagram(error);
        if !transient {
            self.retry_at = Some(now + self.delay);
            self.delay = (self.delay * 2).min(MAX_DELAY);
        }
        if self.log_after.is_none_or(|deadline| now >= deadline) {
            if transient {
                tracing::debug!(local_addr = ?socket.local_addr().ok(), %error, "UDP receive skipped one datagram");
            } else {
                tracing::warn!(local_addr = ?socket.local_addr().ok(), %error, "UDP receive failed; retrying");
            }
            self.log_after = Some(now + LOG_INTERVAL);
        }
    }
}

/// Errors that describe one datagram or ICMP report rather than the socket.
/// Unix truncates an oversized datagram silently; Windows reports WSAEMSGSIZE.
fn per_datagram(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkUnreachable
    ) || (cfg!(windows) && error.raw_os_error() == Some(10040))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    #[tokio::test(start_paused = true)]
    async fn an_icmp_report_is_skipped_without_delay() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut receiver = Receiver::new();
        receiver.failed(&socket, &io::ErrorKind::ConnectionReset.into());
        assert!(receiver.retry_at.is_none());
        assert_eq!(receiver.delay, INITIAL_DELAY);
    }

    #[tokio::test(start_paused = true)]
    async fn other_errors_back_off_and_a_datagram_restores_fast_recovery() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut receiver = Receiver::new();
        for _ in 0..16 {
            receiver.failed(&socket, &io::Error::other("no buffer space"));
        }
        assert_eq!(receiver.delay, MAX_DELAY);
        sender
            .send_to(b"ping", socket.local_addr().unwrap())
            .await
            .unwrap();
        let mut buffer = [0; 16];
        let start = Instant::now();
        let (len, from) = timeout(MAX_DELAY * 2, receiver.recv_from(&socket, &mut buffer))
            .await
            .unwrap();
        assert!(Instant::now() - start <= MAX_DELAY);
        assert_eq!(&buffer[..len], b"ping");
        assert_eq!(from, sender.local_addr().unwrap());
        assert!(receiver.retry_at.is_none());
        assert_eq!(receiver.delay, INITIAL_DELAY);
    }

    #[test]
    fn socket_level_errors_are_not_per_datagram() {
        assert!(per_datagram(&io::ErrorKind::ConnectionRefused.into()));
        assert!(!per_datagram(&io::Error::other("bad descriptor")));
        assert_eq!(
            per_datagram(&io::Error::from_raw_os_error(10040)),
            cfg!(windows)
        );
    }
}
