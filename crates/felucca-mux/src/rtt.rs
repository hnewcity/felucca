//! Round-trip time tracking, and the keepalive deadline derived from it.
//!
//! A fixed pong deadline has to be set for the worst link you intend to serve,
//! which makes it uselessly lax for everyone else: a 30-second timeout on a
//! 10 ms LAN link means a dead peer sits undetected for 30 seconds. Scaling the
//! deadline off measured RTT gives a tight bound on a fast link and still
//! tolerates a satellite hop.
//!
//! Mirrors the intent of the Go original's adaptive timeout
//! (`lib/mux/mux.go: effectivePingTimeout`), which clamps
//! `peak_rtt * multiplier` into a configured range. The peak rather than the
//! average is deliberate — a link that occasionally spikes to 2 s must not be
//! killed on the next spike.

use std::time::{Duration, Instant};

/// Multiplier applied to the peak RTT to get a pong deadline.
const TIMEOUT_MULTIPLIER: u32 = 6;

/// Smoothing factor for the moving average, as a reciprocal: each sample moves
/// the average by 1/8 of the difference. Matches TCP's own RTT smoothing.
const SMOOTHING_SHIFT: u32 = 3;

/// Tracks RTT samples for one session.
#[derive(Debug, Default)]
pub struct RttTracker {
    /// Exponentially weighted moving average.
    smoothed: Option<Duration>,
    /// Most recent sample.
    last: Option<Duration>,
    /// Largest sample seen since the last reset.
    peak: Option<Duration>,
    /// Best observed path delay, without treating queued bulk data as capacity.
    minimum: Option<Duration>,
    /// The ping we are waiting on: its nonce and when it went out.
    ///
    /// Only the newest is tracked. An older pong arriving late is discarded by
    /// the nonce check rather than folded in as a bogus sample — the reason the
    /// nonce is echoed at all.
    pending: Option<(u32, Instant)>,
}

impl RttTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Notes that a ping went out at `at`.
    ///
    /// An already-outstanding ping is left alone. Overwriting it would restart
    /// the clock on every tick, so `pending_age` could never exceed one ping
    /// interval and the timeout would never fire — a keepalive that reports
    /// nothing. Callers should not ping while one is pending anyway; this keeps
    /// the invariant even if they do.
    pub fn record_ping(&mut self, nonce: u32, at: Instant) {
        if self.pending.is_none() {
            self.pending = Some((nonce, at));
        }
    }

    /// Folds in the round trip for a matching pong, returning whether it matched.
    ///
    /// Does the match and the update together so a caller holding a lock needs
    /// to take it only once.
    pub fn observe_pong(&mut self, nonce: u32, now: Instant) -> bool {
        match self.pending {
            Some((want, sent)) if want == nonce => {
                self.pending = None;
                self.observe(now.saturating_duration_since(sent));
                true
            }
            _ => false,
        }
    }

    /// How long the outstanding ping has gone unanswered, if there is one.
    pub fn pending_age(&self, now: Instant) -> Option<Duration> {
        self.pending
            .map(|(_, sent)| now.saturating_duration_since(sent))
    }

    /// Whether a ping is awaiting its pong.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Records one round-trip measurement.
    pub fn observe(&mut self, sample: Duration) {
        self.last = Some(sample);
        self.peak = Some(self.peak.map_or(sample, |p| p.max(sample)));
        self.minimum = Some(self.minimum.map_or(sample, |p| p.min(sample)));
        self.smoothed = Some(match self.smoothed {
            None => sample,
            // avg += (sample - avg) / 8, in integer nanoseconds so no float
            // rounding creeps into a value that gates connection liveness.
            Some(avg) => {
                let avg_ns = avg.as_nanos();
                let s_ns = sample.as_nanos();
                let next = if s_ns > avg_ns {
                    avg_ns + ((s_ns - avg_ns) >> SMOOTHING_SHIFT)
                } else {
                    avg_ns - ((avg_ns - s_ns) >> SMOOTHING_SHIFT)
                };
                Duration::from_nanos(next.min(u128::from(u64::MAX)) as u64)
            }
        });
    }

    /// The smoothed RTT, if any sample has landed.
    pub fn smoothed(&self) -> Option<Duration> {
        self.smoothed
    }

    pub(crate) fn minimum(&self) -> Option<Duration> {
        self.minimum
    }

    /// The most recent sample.
    pub fn last(&self) -> Option<Duration> {
        self.last
    }

    /// The largest sample since the last [`reset_peak`](Self::reset_peak).
    pub fn peak(&self) -> Option<Duration> {
        self.peak
    }

    /// Forgets the historical peak, keeping the latest sample as the new one.
    ///
    /// Without this, one transient spike would inflate the deadline for the rest
    /// of the session.
    pub fn reset_peak(&mut self) {
        self.peak = self.last;
    }

    /// How long to wait for a pong, given the configured bounds.
    ///
    /// Before any sample lands there is nothing to scale from, so the configured
    /// maximum applies — the safe direction, since a premature timeout would
    /// tear down a healthy session during startup.
    pub fn pong_deadline(&self, min: Duration, max: Duration) -> Duration {
        // A caller that passes an inverted range gets a coherent answer rather
        // than a deadline below its own floor.
        let (min, max) = if min > max { (max, min) } else { (min, max) };
        match self.peak {
            None => max,
            Some(peak) => peak.saturating_mul(TIMEOUT_MULTIPLIER).clamp(min, max),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(2);
    const MAX: Duration = Duration::from_secs(30);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_fresh_tracker_has_nothing_to_report() {
        let t = RttTracker::new();
        assert_eq!(t.smoothed(), None);
        assert_eq!(t.peak(), None);
        assert_eq!(t.last(), None);
    }

    #[test]
    fn the_first_sample_seeds_every_statistic() {
        let mut t = RttTracker::new();
        t.observe(ms(50));
        assert_eq!(t.smoothed(), Some(ms(50)));
        assert_eq!(t.last(), Some(ms(50)));
        assert_eq!(t.peak(), Some(ms(50)));
    }

    #[test]
    fn the_average_moves_gradually_toward_a_new_level() {
        let mut t = RttTracker::new();
        t.observe(ms(100));
        t.observe(ms(200));
        let avg = t.smoothed().unwrap();
        // One sample must not drag the average all the way across.
        assert!(avg > ms(100) && avg < ms(200), "unexpected {avg:?}");

        // Repeated samples converge on the new level.
        for _ in 0..60 {
            t.observe(ms(200));
        }
        let avg = t.smoothed().unwrap();
        assert!(avg > ms(195), "should have converged, got {avg:?}");
    }

    #[test]
    fn the_average_converges_downward_too() {
        let mut t = RttTracker::new();
        t.observe(ms(500));
        for _ in 0..80 {
            t.observe(ms(10));
        }
        assert!(t.smoothed().unwrap() < ms(20), "got {:?}", t.smoothed());
    }

    #[test]
    fn the_peak_holds_while_the_average_recovers() {
        let mut t = RttTracker::new();
        t.observe(ms(10));
        t.observe(ms(900));
        for _ in 0..40 {
            t.observe(ms(10));
        }
        assert_eq!(
            t.peak(),
            Some(ms(900)),
            "a spike must keep the deadline lax"
        );
        assert!(t.smoothed().unwrap() < ms(100));
        assert_eq!(t.last(), Some(ms(10)));
    }

    #[test]
    fn resetting_the_peak_adopts_the_latest_sample() {
        let mut t = RttTracker::new();
        t.observe(ms(900));
        t.observe(ms(20));
        t.reset_peak();
        assert_eq!(t.peak(), Some(ms(20)));
    }

    #[test]
    fn the_deadline_is_the_configured_max_before_any_sample() {
        // Nothing to scale from; timing out early would kill a healthy session
        // during startup.
        assert_eq!(RttTracker::new().pong_deadline(MIN, MAX), MAX);
    }

    #[test]
    fn a_fast_link_gets_a_tight_deadline() {
        let mut t = RttTracker::new();
        t.observe(ms(5));
        // 5ms * 6 = 30ms, below the floor, so the floor applies.
        assert_eq!(t.pong_deadline(MIN, MAX), MIN);
    }

    #[test]
    fn a_middling_link_scales_between_the_bounds() {
        let mut t = RttTracker::new();
        t.observe(ms(1000));
        // 1s * 6 = 6s, inside [2s, 30s].
        assert_eq!(t.pong_deadline(MIN, MAX), Duration::from_secs(6));
    }

    #[test]
    fn a_slow_link_is_capped_rather_than_unbounded() {
        let mut t = RttTracker::new();
        t.observe(Duration::from_secs(20));
        // 20s * 6 = 120s, clamped to the configured ceiling.
        assert_eq!(t.pong_deadline(MIN, MAX), MAX);
    }

    #[test]
    fn the_deadline_follows_the_peak_not_the_average() {
        let mut t = RttTracker::new();
        t.observe(ms(2000));
        for _ in 0..50 {
            t.observe(ms(1));
        }
        // The average has collapsed to ~1ms, which on its own would give the
        // floor. The peak is what counts: 2s * 6 = 12s, inside the bounds.
        assert_eq!(t.pong_deadline(MIN, MAX), Duration::from_secs(12));
    }

    #[test]
    fn an_inverted_bound_pair_still_yields_a_sane_deadline() {
        let mut t = RttTracker::new();
        t.observe(ms(1000));
        let d = t.pong_deadline(MAX, MIN);
        assert!((MIN..=MAX).contains(&d), "got {d:?}");
    }

    #[test]
    fn an_absurd_sample_does_not_overflow_the_deadline() {
        let mut t = RttTracker::new();
        // A bogus clock reading must not panic in the multiply.
        t.observe(Duration::MAX);
        assert_eq!(t.pong_deadline(MIN, MAX), MAX);
    }

    #[test]
    fn a_matching_pong_yields_a_sample() {
        let mut t = RttTracker::new();
        let sent = Instant::now();
        t.record_ping(7, sent);
        assert!(t.has_pending());

        assert!(t.observe_pong(7, sent + ms(40)));
        assert_eq!(t.last(), Some(ms(40)));
        assert!(!t.has_pending(), "the ping is answered");
    }

    #[test]
    fn an_outstanding_ping_keeps_its_original_timestamp() {
        // The bug this guards: restarting the clock each tick means the age
        // never crosses the deadline and a dead peer is never detected.
        let mut t = RttTracker::new();
        let t0 = Instant::now();
        t.record_ping(1, t0);
        t.record_ping(2, t0 + ms(500));
        assert_eq!(
            t.pending_age(t0 + ms(500)),
            Some(ms(500)),
            "age must be measured from the first unanswered ping"
        );
    }

    #[test]
    fn a_stale_pong_is_ignored() {
        // A late reply to a previous ping must not be timed against the new one.
        let mut t = RttTracker::new();
        let sent = Instant::now();
        t.record_ping(9, sent);
        assert!(!t.observe_pong(8, sent + ms(900)));
        assert_eq!(t.last(), None, "no sample from a mismatched nonce");
        assert!(t.has_pending(), "we are still waiting on ping 9");
    }

    #[test]
    fn an_unsolicited_pong_is_ignored() {
        let mut t = RttTracker::new();
        assert!(!t.observe_pong(1, Instant::now()));
        assert_eq!(t.last(), None);
    }

    #[test]
    fn the_pending_age_grows_until_answered() {
        let mut t = RttTracker::new();
        let sent = Instant::now();
        assert_eq!(t.pending_age(sent), None);

        t.record_ping(1, sent);
        assert_eq!(t.pending_age(sent + ms(500)), Some(ms(500)));
        t.observe_pong(1, sent + ms(600));
        assert_eq!(t.pending_age(sent + ms(700)), None);
    }

    #[test]
    fn a_clock_that_goes_backwards_yields_zero_not_a_panic() {
        let mut t = RttTracker::new();
        let sent = Instant::now();
        t.record_ping(1, sent + ms(100));
        assert_eq!(t.pending_age(sent), Some(Duration::ZERO));
    }

    #[test]
    fn a_zero_rtt_sample_is_handled() {
        let mut t = RttTracker::new();
        t.observe(Duration::ZERO);
        assert_eq!(t.smoothed(), Some(Duration::ZERO));
        assert_eq!(t.pong_deadline(MIN, MAX), MIN);
    }
}
