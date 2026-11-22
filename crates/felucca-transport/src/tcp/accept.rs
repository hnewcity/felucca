use std::{io, net::SocketAddr, time::Duration};

use tokio::{
    net::{TcpListener, TcpStream},
    time::{Instant, sleep_until},
};

const INITIAL_DELAY: Duration = Duration::from_millis(10);
const MAX_DELAY: Duration = Duration::from_secs(1);
const LOG_INTERVAL: Duration = Duration::from_secs(30);

/// Accepts TCP connections without retiring a listener on a transient OS error.
///
/// File descriptor or socket buffer exhaustion can clear when existing sessions
/// finish. Keep ownership of the bound socket and retry with bounded backoff.
/// `accept` is cancellation safe: its retry deadline survives other `select!`
/// branches completing, and dropping this acceptor closes the listener.
#[derive(Debug)]
pub struct Acceptor {
    listener: TcpListener,
    retry_at: Option<Instant>,
    delay: Duration,
    log_after: Option<Instant>,
}

impl Acceptor {
    pub fn new(listener: TcpListener) -> Self {
        Self {
            listener,
            retry_at: None,
            delay: INITIAL_DELAY,
            log_after: None,
        }
    }

    pub async fn accept(&mut self) -> (TcpStream, SocketAddr) {
        loop {
            if let Some(deadline) = self.retry_at {
                sleep_until(deadline).await;
            }
            match self.listener.accept().await {
                Ok((stream, peer)) => {
                    self.retry_at = None;
                    self.delay = INITIAL_DELAY;
                    super::tune(&stream);
                    return (stream, peer);
                }
                Err(error) => self.failed(&error),
            }
        }
    }

    fn failed(&mut self, error: &io::Error) {
        let now = Instant::now();
        self.retry_at = Some(now + self.delay);
        self.delay = (self.delay * 2).min(MAX_DELAY);
        if self.log_after.is_none_or(|deadline| now >= deadline) {
            tracing::warn!(
                local_addr = ?self.listener.local_addr().ok(),
                %error,
                "TCP accept failed; retrying"
            );
            self.log_after = Some(now + LOG_INTERVAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{advance, timeout};

    #[tokio::test(start_paused = true)]
    async fn cancelling_accept_does_not_reset_the_retry_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let mut acceptor = Acceptor::new(listener);
        acceptor.failed(&io::Error::other("temporary resource exhaustion"));
        let start = Instant::now();
        assert!(
            timeout(Duration::from_millis(5), acceptor.accept())
                .await
                .is_err()
        );
        let (stream, peer) = acceptor.accept().await;
        assert_eq!(Instant::now() - start, INITIAL_DELAY);
        assert_eq!(peer, client.local_addr().unwrap());
        assert!(stream.nodelay().unwrap());
        assert!(acceptor.retry_at.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn prolonged_errors_back_off_but_a_success_restores_fast_recovery() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut acceptor = Acceptor::new(listener);
        for _ in 0..16 {
            acceptor.failed(&io::Error::other("temporary resource exhaustion"));
            let delay = acceptor.retry_at.unwrap() - Instant::now();
            assert!((INITIAL_DELAY..=MAX_DELAY).contains(&delay));
            advance(delay).await;
        }
        assert_eq!(acceptor.delay, MAX_DELAY);
        let client = TcpStream::connect(addr).await.unwrap();
        let (_stream, peer) = timeout(MAX_DELAY, acceptor.accept()).await.unwrap();
        assert_eq!(peer, client.local_addr().unwrap());
        acceptor.failed(&io::Error::other("another transient error"));
        assert_eq!(acceptor.retry_at.unwrap() - Instant::now(), INITIAL_DELAY);
    }
}
