//! Bandwidth limiting.
//!
//! A token bucket that computes *how long to wait*, rather than sleeping
//! itself. Keeping the decision synchronous and side-effect free means
//! `felucca-core` needs no async runtime, and lets the caller decide what to do
//! with the delay: await a timer on a forwarding task, or drop a datagram that
//! would arrive too late to matter.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Tokens are bytes; the bucket refills at `rate` bytes per second.
#[derive(Debug)]
pub struct RateLimiter {
    rate: u64,
    capacity: u64,
    state: Mutex<BucketState>,
}

#[derive(Debug)]
struct BucketState {
    /// Scaled by [`SCALE`] so a sub-byte-per-tick refill is not lost to rounding.
    tokens: i128,
    last: Instant,
}

/// Fixed-point denominator for the token count.
const SCALE: u64 = 1 << 16;

impl RateLimiter {
    /// A limiter allowing `bytes_per_sec`, bursting up to one second's worth.
    ///
    /// A rate of zero is treated as unlimited, matching the Go original's
    /// convention where `0` in the config field means "no limit".
    pub fn new(bytes_per_sec: u64) -> Self {
        Self::with_burst(bytes_per_sec, bytes_per_sec)
    }

    /// A limiter with an explicit burst size.
    pub fn with_burst(bytes_per_sec: u64, burst: u64) -> Self {
        // A burst below one byte could never admit a single byte, deadlocking
        // the caller; floor it so progress is always possible.
        let capacity = burst.max(1);
        Self {
            rate: bytes_per_sec,
            capacity,
            state: Mutex::new(BucketState {
                tokens: i128::from(capacity) * i128::from(SCALE),
                last: Instant::now(),
            }),
        }
    }

    /// Whether this limiter enforces anything.
    pub fn is_unlimited(&self) -> bool {
        self.rate == 0
    }

    pub fn rate(&self) -> u64 {
        self.rate
    }

    pub fn burst(&self) -> u64 {
        self.capacity
    }

    /// Charges `n` bytes, returning how long the caller must wait first.
    ///
    /// The debt is always taken, even when the bucket cannot cover it, so a
    /// write larger than the burst size is paced rather than rejected. Callers
    /// that ignore the returned delay get no limiting at all.
    pub fn take(&self, n: u64) -> Option<Duration> {
        self.take_at(n, Instant::now())
    }

    /// [`take`](Self::take) against a caller-supplied clock reading, for tests.
    pub fn take_at(&self, n: u64, now: Instant) -> Option<Duration> {
        if self.rate == 0 || n == 0 {
            return None;
        }

        let want = i128::from(n) * i128::from(SCALE);
        let mut st = self.state.lock();

        // Refill for the time since the last charge. `saturating_duration_since`
        // keeps a non-monotonic reading from panicking.
        let elapsed = now.saturating_duration_since(st.last);
        if elapsed > Duration::ZERO {
            let gained = elapsed
                .as_nanos()
                .saturating_mul(self.rate as u128)
                .saturating_mul(SCALE as u128)
                / 1_000_000_000u128;
            let ceiling = i128::from(self.capacity) * i128::from(SCALE);
            st.tokens = st
                .tokens
                .saturating_add(gained.min(i128::MAX as u128) as i128)
                .min(ceiling);
            st.last = now;
        }

        st.tokens = st.tokens.saturating_sub(want);
        if st.tokens >= 0 {
            return None;
        }
        // Carry debt across concurrent callers: they reserve different future
        // bytes rather than all sleeping for the same empty bucket.
        let deficit = st.tokens.unsigned_abs();
        let nanos = deficit
            .saturating_mul(1_000_000_000u128)
            .div_ceil((self.rate as u128).saturating_mul(SCALE as u128));
        // Round up: truncating to zero would let a caller busy-loop.
        Some(Duration::from_nanos(
            nanos.max(1).min(u64::MAX as u128) as u64
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_rate_never_delays() {
        let l = RateLimiter::new(0);
        assert!(l.is_unlimited());
        assert_eq!(l.take(1_000_000), None);
    }

    #[test]
    fn a_full_bucket_admits_its_burst_immediately() {
        let l = RateLimiter::new(1000);
        assert_eq!(l.take(1000), None);
    }

    #[test]
    fn exceeding_the_bucket_costs_time() {
        let l = RateLimiter::new(1000);
        let t = Instant::now();
        assert_eq!(l.take_at(1000, t), None);
        // Bucket is empty; another 500 bytes at 1000 B/s is ~500ms.
        let d = l.take_at(500, t).expect("should be throttled");
        let ms = d.as_millis();
        assert!((480..=520).contains(&ms), "unexpected delay {ms}ms");
    }

    #[test]
    fn tokens_refill_over_time() {
        let l = RateLimiter::new(1000);
        let t0 = Instant::now();
        assert_eq!(l.take_at(1000, t0), None);
        // After a full second the bucket is back to its capacity.
        assert_eq!(l.take_at(1000, t0 + Duration::from_secs(1)), None);
    }

    #[test]
    fn refill_is_capped_at_the_burst_size() {
        let l = RateLimiter::new(1000);
        let t0 = Instant::now();
        // Idle for an hour, then ask for far more than one burst.
        let d = l.take_at(5000, t0 + Duration::from_secs(3600));
        assert!(d.is_some(), "hoarding past the cap should not be possible");
    }

    #[test]
    fn a_write_larger_than_the_burst_is_paced_not_rejected() {
        let l = RateLimiter::with_burst(1000, 100);
        let t = Instant::now();
        // 10 000 bytes at 1000 B/s is ~10s of debt, minus the 100 in the bucket.
        let d = l.take_at(10_000, t).expect("should be throttled");
        assert!(d.as_millis() >= 9_800, "unexpected delay {:?}", d);
    }

    #[test]
    fn small_charges_still_round_up_to_a_real_wait() {
        // 1 byte/sec, empty bucket, ask for 1 byte: must not return Some(0).
        let l = RateLimiter::with_burst(1, 1);
        let t = Instant::now();
        assert_eq!(l.take_at(1, t), None);
        let d = l.take_at(1, t).expect("should be throttled");
        assert!(d > Duration::ZERO, "a zero delay would busy-loop");
    }

    #[test]
    fn zero_length_charges_are_free() {
        let l = RateLimiter::new(10);
        assert_eq!(l.take(0), None);
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_panic() {
        let l = RateLimiter::new(1000);
        let t = Instant::now();
        assert_eq!(l.take_at(1000, t), None);
        // An earlier reading than the last charge: refill nothing, stay sane.
        let _ = l.take_at(10, t - Duration::from_secs(5));
    }

    #[test]
    fn burst_below_one_byte_still_makes_progress() {
        let l = RateLimiter::with_burst(1000, 0);
        assert_eq!(l.burst(), 1);
        let t = Instant::now();
        assert_eq!(l.take_at(1, t), None);
    }

    #[test]
    fn concurrent_reservations_share_debt_and_repay_it_over_time() {
        let limiter = RateLimiter::new(1000);
        let now = Instant::now();
        assert_eq!(limiter.take_at(1000, now), None);
        assert_eq!(limiter.take_at(500, now), Some(Duration::from_millis(500)));
        assert_eq!(limiter.take_at(500, now), Some(Duration::from_secs(1)));
        assert_eq!(
            limiter.take_at(500, now + Duration::from_millis(500)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(limiter.take_at(500, now + Duration::from_secs(2)), None);
    }
}
