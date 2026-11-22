//! Credit accounting for stream flow control.
//!
//! A sender may only put as many bytes on a stream as the receiver has granted
//! credit for. The receiver grants credit up front when the stream opens, and
//! returns more as the application consumes what arrived. A stream whose reader
//! stalls therefore stops its sender rather than growing an unbounded buffer.
//!
//! This module is the whole policy, kept free of I/O so it can be tested
//! exhaustively. The Go original spreads the same job across
//! `conn_send_window.go`, `conn_receive_window.go` and `budget.go`, with the
//! window size derived from a measured bandwidth-delay product. felucca grows
//! windows when readers consume them within a few RTTs; growth and returned
//! credit are distinct wire operations with separate validation.

use std::task::{Context, Poll};
use std::{sync::Arc, time::Duration};

use futures_util::task::AtomicWaker;
use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::frame::CloseReason;

/// Sender-side credit: how many bytes this side may still write.
#[derive(Debug)]
pub struct SendCredit {
    state: Mutex<SendState>,
    /// Woken when credit arrives or the stream closes.
    wake: Notify,
    /// Waker for the poll-based path. A stream has exactly one writer, so a
    /// single slot is enough and costs no allocation per write.
    poll_wake: AtomicWaker,
}

#[derive(Debug)]
struct SendState {
    available: u64,
    outstanding: u64,
    window: u32,
    closed: Option<CloseReason>,
    pacing: Option<Pacing>,
}

// Keep initial DATA below the receive advertisement: an advertisement is a
// memory bound, not evidence that the transport can drain that much promptly.
// This is local policy; wire credit and over-grant validation stay unchanged.
const INITIAL_FLIGHT: u64 = crate::frame::MAX_SEGMENT as u64;
// Once delivery is measured, a slow path needs less than a full segment in
// flight: negotiation and multiple request/reply steps share the same pipe.
const MIN_FLIGHT: u64 = INITIAL_FLIGHT / 4;

#[derive(Debug)]
struct Pacing {
    limit: u64,
    sent: u64,
    delivered: u64,
    round: Option<DeliveryRound>,
    rtt: Arc<Mutex<crate::RttTracker>>,
}

#[derive(Debug)]
struct DeliveryRound {
    started: tokio::time::Instant,
    base: u64,
    // Freeze a watermark when the sender fills its flight budget. Later
    // writes must not extend the measurement indefinitely.
    end: Option<u64>,
}

impl Pacing {
    fn reserved(&mut self, bytes: u64, outstanding: u64) {
        if bytes == 0 {
            return;
        }
        self.sent += bytes;
        let round = self.round.get_or_insert_with(|| DeliveryRound {
            started: tokio::time::Instant::now(),
            base: self.delivered,
            end: None,
        });
        if outstanding >= self.limit && round.end.is_none() {
            round.end = Some(self.sent);
        }
    }

    fn granted(&mut self, bytes: u64, outstanding: u64, window: u32) {
        self.delivered += bytes;
        if let Some(round) = &self.round
            && let Some(end) = round.end
            && self.delivered >= end
        {
            let elapsed = round.started.elapsed().as_nanos().max(1);
            let rtt = self
                .rtt
                .lock()
                .minimum()
                .unwrap_or(Duration::from_millis(100));
            let target = rtt.saturating_mul(2) + Duration::from_millis(100);
            let flight = (u128::from(end - round.base) * target.as_nanos() / elapsed)
                .min(u128::from(u64::MAX)) as u64;
            self.limit = flight
                .max(MIN_FLIGHT)
                .min(self.limit.saturating_mul(2))
                .min(u64::from(window));
            self.round = None;
        }
        if outstanding == 0 {
            // Idle time is not a delivery sample and sparse RPCs must not grow
            // the budget before a later bulk transfer starts.
            self.round = None;
        } else if self.round.is_none() {
            self.round = Some(DeliveryRound {
                started: tokio::time::Instant::now(),
                base: self.delivered,
                end: (outstanding >= self.limit).then_some(self.sent),
            });
        }
    }
}

impl SendState {
    fn reserve(&mut self, want: u32) -> u32 {
        let capacity = self.pacing.as_ref().map_or(self.available, |p| {
            self.available.min(p.limit.saturating_sub(self.outstanding))
        });
        let take = capacity.min(u64::from(want));
        self.available -= take;
        self.outstanding += take;
        if let Some(pacing) = &mut self.pacing {
            pacing.reserved(take, self.outstanding);
        }
        take as u32
    }
}

impl SendCredit {
    pub fn new(initial: u32) -> Self {
        Self {
            state: Mutex::new(SendState {
                available: u64::from(initial),
                outstanding: 0,
                window: initial,
                closed: None,
                pacing: None,
            }),
            wake: Notify::new(),
            poll_wake: AtomicWaker::new(),
        }
    }
    pub(crate) fn paced(initial: u32, rtt: Arc<Mutex<crate::RttTracker>>) -> Self {
        let mut credit = Self::new(initial);
        credit.state.get_mut().pacing = Some(Pacing {
            limit: INITIAL_FLIGHT,
            sent: 0,
            delivered: 0,
            round: None,
            rtt,
        });
        credit
    }
    pub(crate) fn check_open(&self, cx: &Context<'_>) -> Result<(), CloseReason> {
        self.poll_wake.register(cx.waker());
        match self.state.lock().closed {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }

    /// Reserves up to `want` bytes, waiting until at least one is available.
    ///
    /// Returns however much it could take, which may be less than `want`. A
    /// caller that insisted on the full amount could deadlock whenever the peer's
    /// window is smaller than one write, so partial grants are the contract:
    /// write what you were given, then ask again.
    pub async fn reserve(&self, want: u32) -> Result<u32, CloseReason> {
        if want == 0 {
            return Ok(0);
        }
        loop {
            // Register interest before re-checking, so a grant landing between
            // the check and the await is not lost.
            let notified = self.wake.notified();
            {
                let mut st = self.state.lock();
                if let Some(reason) = st.closed {
                    return Err(reason);
                }
                let take = st.reserve(want);
                if take > 0 {
                    return Ok(take);
                }
            }
            notified.await;
        }
    }

    /// [`reserve`](Self::reserve) for a poll context.
    ///
    /// Exists because [`tokio::io::AsyncWrite::poll_write`] cannot await; the
    /// alternative would be boxing a future on every write.
    pub fn poll_reserve(&self, cx: &mut Context<'_>, want: u32) -> Poll<Result<u32, CloseReason>> {
        if want == 0 {
            return Poll::Ready(Ok(0));
        }
        // Register before the check, so a grant landing in between wakes us
        // rather than being missed.
        self.poll_wake.register(cx.waker());
        let mut st = self.state.lock();
        if let Some(reason) = st.closed {
            return Poll::Ready(Err(reason));
        }
        let take = st.reserve(want);
        if take > 0 {
            return Poll::Ready(Ok(take));
        }
        Poll::Pending
    }

    /// Takes up to `want` bytes if any are available right now.
    pub fn try_reserve(&self, want: u32) -> Result<u32, CloseReason> {
        let mut st = self.state.lock();
        if let Some(reason) = st.closed {
            return Err(reason);
        }
        Ok(st.reserve(want))
    }

    /// Adds credit granted by the peer.
    ///
    /// Credit can only return bytes previously reserved for transmission.
    pub fn grant(&self, credit: u32) -> Result<(), CreditViolation> {
        if credit == 0 {
            return Ok(());
        }
        {
            let mut st = self.state.lock();
            if st.closed.is_some() {
                return Ok(());
            }
            if u64::from(credit) > st.outstanding {
                return Err(CreditViolation {
                    got: credit,
                    allowed: st.outstanding as u32,
                });
            }
            st.outstanding -= u64::from(credit);
            st.available += u64::from(credit);
            let (outstanding, window) = (st.outstanding, st.window);
            if let Some(pacing) = &mut st.pacing {
                pacing.granted(u64::from(credit), outstanding, window);
            }
        }
        self.wake.notify_waiters();
        self.poll_wake.wake();
        Ok(())
    }

    /// Expands capacity explicitly; it cannot be disguised as a credit return.
    pub fn grow(&self, additional: u32) -> Result<(), CreditViolation> {
        {
            let mut st = self.state.lock();
            if st.closed.is_some() {
                return Ok(());
            }
            let allowed = crate::frame::MAX_STREAM_WINDOW.saturating_sub(st.window);
            if additional > allowed {
                return Err(CreditViolation {
                    got: additional,
                    allowed,
                });
            }
            st.window += additional;
            st.available += u64::from(additional);
        }
        self.wake.notify_waiters();
        self.poll_wake.wake();
        Ok(())
    }

    /// Fails every current and future reservation.
    pub fn close(&self, reason: CloseReason) {
        {
            let mut st = self.state.lock();
            if st.closed.is_some() {
                return;
            }
            st.closed = Some(reason);
        }
        self.wake.notify_waiters();
        self.poll_wake.wake();
    }

    pub fn available(&self) -> u64 {
        self.state.lock().available
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().closed.is_some()
    }
}

/// Receiver-side credit: how much the peer may still send us, and how much to
/// give back as the application reads.
#[derive(Debug)]
pub struct RecvWindow {
    /// Bytes the peer is still entitled to send.
    peer_credit: u32,
    /// Bytes read out by the application but not yet advertised back.
    unreturned: u32,
    /// Credit to accumulate before spending a frame to return it.
    threshold: u32,
    /// Total credit outstanding when the window is full.
    window: u32,
}

impl RecvWindow {
    pub fn capacity(&self) -> u32 {
        self.window
    }
    pub fn grow(&mut self, additional: u32) {
        self.window += additional;
        self.peer_credit += additional;
        self.threshold = (self.window / 2).max(1);
    }
    /// A window of `window` bytes, fully granted to the peer up front.
    pub fn new(window: u32) -> Self {
        // Returning credit every read would cost a frame per read; waiting for
        // the window to empty would stall the peer. Half is the usual compromise.
        let threshold = (window / 2).max(1);
        Self {
            peer_credit: window,
            unreturned: 0,
            threshold,
            window,
        }
    }

    /// Records `len` bytes arriving.
    ///
    /// `Err` means the peer sent more than it was granted, which is a protocol
    /// violation rather than something to absorb: honouring it would defeat the
    /// memory bound the window exists to enforce.
    pub fn on_data(&mut self, len: u32) -> Result<(), CreditViolation> {
        if len > self.peer_credit {
            return Err(CreditViolation {
                got: len,
                allowed: self.peer_credit,
            });
        }
        self.peer_credit -= len;
        Ok(())
    }

    /// Records `len` bytes handed to the application.
    ///
    /// Returns the credit to advertise, once enough has accumulated to be worth
    /// a frame.
    pub fn on_consumed(&mut self, len: u32) -> Option<u32> {
        self.unreturned = self.unreturned.saturating_add(len);
        if self.unreturned < self.threshold {
            return None;
        }
        let grant = std::mem::take(&mut self.unreturned);
        self.peer_credit = self.peer_credit.saturating_add(grant);
        Some(grant)
    }

    /// Returns any accumulated credit regardless of the threshold.
    ///
    /// Used when the reader has drained everything and the peer would otherwise
    /// wait for a grant that never reaches the threshold — the case that
    /// deadlocks a naive implementation on a small final read.
    pub fn flush(&mut self) -> Option<u32> {
        if self.unreturned == 0 {
            return None;
        }
        let grant = std::mem::take(&mut self.unreturned);
        self.peer_credit = self.peer_credit.saturating_add(grant);
        Some(grant)
    }

    /// Bytes the peer may still send.
    pub fn peer_credit(&self) -> u32 {
        self.peer_credit
    }

    /// Bytes currently held by us, granted but not yet returned.
    pub fn in_flight(&self) -> u32 {
        self.window
            .saturating_sub(self.peer_credit)
            .saturating_sub(self.unreturned)
    }
}

/// The peer sent more than its remaining credit allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditViolation {
    pub got: u32,
    pub allowed: u32,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn paced_credit_adapts_to_delivery_without_growing_wire_credit() {
        let rtt = Arc::new(Mutex::new(crate::RttTracker::new()));
        rtt.lock().observe(Duration::from_millis(40));
        let c = SendCredit::paced(1024 * 1024, rtt);
        assert_eq!(c.try_reserve(u32::MAX), Ok(INITIAL_FLIGHT as u32));
        assert_eq!(c.try_reserve(1), Ok(0));
        assert!(c.grant(INITIAL_FLIGHT as u32 + 1).is_err());
        tokio::time::sleep(Duration::from_millis(40)).await;
        c.grant(INITIAL_FLIGHT as u32).unwrap();
        assert_eq!(c.try_reserve(u32::MAX), Ok(2 * INITIAL_FLIGHT as u32));
        tokio::time::sleep(Duration::from_secs(4)).await;
        c.grant(2 * INITIAL_FLIGHT as u32).unwrap();
        assert_eq!(c.try_reserve(u32::MAX), Ok(MIN_FLIGHT as u32));
        assert_eq!(c.available(), 1024 * 1024 - MIN_FLIGHT);
    }

    #[tokio::test(start_paused = true)]
    async fn sparse_messages_do_not_expand_a_later_bulk_flight() {
        let c = SendCredit::paced(1024 * 1024, Arc::new(Mutex::new(crate::RttTracker::new())));
        for _ in 0..20 {
            assert_eq!(c.try_reserve(100), Ok(100));
            tokio::time::sleep(Duration::from_millis(1)).await;
            c.grant(100).unwrap();
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(c.try_reserve(u32::MAX), Ok(INITIAL_FLIGHT as u32));
    }

    #[tokio::test(start_paused = true)]
    async fn paced_writers_wake_for_credit_and_cancel_without_using_growth_as_an_ack() {
        let c = Arc::new(SendCredit::paced(
            INITIAL_FLIGHT as u32,
            Arc::new(Mutex::new(crate::RttTracker::new())),
        ));
        c.reserve(INITIAL_FLIGHT as u32).await.unwrap();
        let writer = {
            let c = c.clone();
            tokio::spawn(async move { std::future::poll_fn(|cx| c.poll_reserve(cx, 100)).await })
        };
        c.grow(1024 * 1024).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(
            !writer.is_finished(),
            "window growth is not delivery evidence"
        );
        c.grant(100).unwrap();
        assert_eq!(writer.await.unwrap(), Ok(100));
        let writer = {
            let c = c.clone();
            tokio::spawn(async move { c.reserve(1).await })
        };
        tokio::task::yield_now().await;
        assert!(!writer.is_finished());
        c.close(CloseReason::Cancelled);
        assert_eq!(writer.await.unwrap(), Err(CloseReason::Cancelled));
    }

    // --- send side ---

    #[tokio::test]
    async fn a_reservation_within_the_window_is_immediate() {
        let c = SendCredit::new(1000);
        assert_eq!(c.reserve(400).await, Ok(400));
        assert_eq!(c.available(), 600);
    }

    #[tokio::test]
    async fn a_reservation_larger_than_the_window_is_granted_in_part() {
        // Insisting on the full amount would deadlock whenever the peer's window
        // is smaller than one write.
        let c = SendCredit::new(100);
        assert_eq!(c.reserve(4096).await, Ok(100));
        assert_eq!(c.available(), 0);
    }

    #[tokio::test]
    async fn a_zero_length_reservation_needs_no_credit() {
        let c = SendCredit::new(0);
        assert_eq!(c.reserve(0).await, Ok(0));
    }

    #[tokio::test]
    async fn an_exhausted_sender_waits_for_a_grant() {
        let c = Arc::new(SendCredit::new(10));
        assert_eq!(c.reserve(10).await, Ok(10));

        let waiter = tokio::spawn({
            let c = Arc::clone(&c);
            async move { c.reserve(50).await }
        });

        // Nothing should complete while the window is empty.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "a sender with no credit must block");

        c.grant(10).unwrap();
        assert_eq!(waiter.await.unwrap(), Ok(10));
    }

    #[tokio::test]
    async fn a_grant_arriving_during_the_check_is_not_lost() {
        // Guards the register-then-check ordering: a naive implementation drops
        // this wakeup and hangs forever.
        let c = Arc::new(SendCredit::new(50));
        c.reserve(50).await.unwrap();
        let waiter = tokio::spawn({
            let c = Arc::clone(&c);
            async move { c.reserve(10).await }
        });
        // Hammer grants to interleave with the waiter's registration.
        for _ in 0..50 {
            c.grant(1).unwrap();
            tokio::task::yield_now().await;
        }
        let got = tokio::time::timeout(Duration::from_secs(5), waiter).await;
        assert!(got.is_ok(), "reservation should have woken");
        assert!(matches!(got.unwrap().unwrap(), Ok(n) if n > 0));
    }

    #[tokio::test]
    async fn closing_wakes_a_blocked_sender() {
        let c = Arc::new(SendCredit::new(0));
        let waiter = tokio::spawn({
            let c = Arc::clone(&c);
            async move { c.reserve(10).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        c.close(CloseReason::Cancelled);

        let got = tokio::time::timeout(Duration::from_secs(5), waiter).await;
        assert_eq!(
            got.expect("must not hang").unwrap(),
            Err(CloseReason::Cancelled)
        );
    }

    #[tokio::test]
    async fn reservations_after_a_close_fail_immediately() {
        let c = SendCredit::new(1000);
        c.close(CloseReason::Shutdown);
        assert_eq!(c.reserve(1).await, Err(CloseReason::Shutdown));
        assert_eq!(c.try_reserve(1), Err(CloseReason::Shutdown));
        assert!(c.is_closed());
    }

    #[tokio::test]
    async fn the_first_close_reason_is_the_one_reported() {
        let c = SendCredit::new(0);
        c.close(CloseReason::ConnectionRefused);
        c.close(CloseReason::Internal);
        assert_eq!(c.reserve(1).await, Err(CloseReason::ConnectionRefused));
    }

    #[test]
    fn try_reserve_takes_what_is_there_without_waiting() {
        let c = SendCredit::new(50);
        assert_eq!(c.try_reserve(4096), Ok(50));
        // Now empty: a non-blocking attempt yields nothing rather than hanging.
        assert_eq!(c.try_reserve(10), Ok(0));
    }

    #[test]
    fn an_over_grant_is_rejected_without_changing_credit() {
        let c = SendCredit::new(u32::MAX);
        for _ in 0..8 {
            assert!(c.grant(u32::MAX).is_err());
        }
        assert_eq!(c.available(), u64::from(u32::MAX));
    }

    #[test]
    fn granting_after_a_close_changes_nothing() {
        let c = SendCredit::new(0);
        c.close(CloseReason::Normal);
        c.grant(1000).unwrap();
        assert_eq!(c.available(), 0);
    }

    #[tokio::test]
    async fn the_poll_path_wakes_on_a_grant() {
        use std::future::poll_fn;

        let c = Arc::new(SendCredit::new(40));
        c.reserve(40).await.unwrap();
        let waiter = tokio::spawn({
            let c = Arc::clone(&c);
            async move { poll_fn(|cx| c.poll_reserve(cx, 100)).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "must block with no credit");

        c.grant(40).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), waiter).await;
        assert_eq!(got.expect("must not hang").unwrap(), Ok(40));
    }

    #[tokio::test]
    async fn the_poll_path_wakes_on_a_close() {
        use std::future::poll_fn;

        let c = Arc::new(SendCredit::new(0));
        let waiter = tokio::spawn({
            let c = Arc::clone(&c);
            async move { poll_fn(|cx| c.poll_reserve(cx, 100)).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        c.close(CloseReason::Cancelled);

        let got = tokio::time::timeout(Duration::from_secs(5), waiter).await;
        assert_eq!(
            got.expect("must not hang").unwrap(),
            Err(CloseReason::Cancelled)
        );
    }

    // --- receive side ---

    #[test]
    fn the_peer_starts_with_the_whole_window() {
        let w = RecvWindow::new(8000);
        assert_eq!(w.peer_credit(), 8000);
        assert_eq!(w.in_flight(), 0);
    }

    #[test]
    fn arriving_data_consumes_peer_credit() {
        let mut w = RecvWindow::new(8000);
        w.on_data(3000).unwrap();
        assert_eq!(w.peer_credit(), 5000);
        assert_eq!(w.in_flight(), 3000);
    }

    #[test]
    fn a_peer_exceeding_its_credit_is_a_violation() {
        let mut w = RecvWindow::new(1000);
        w.on_data(1000).unwrap();
        // Absorbing this would defeat the memory bound the window enforces.
        assert_eq!(w.on_data(1), Err(CreditViolation { got: 1, allowed: 0 }));
    }

    #[test]
    fn exactly_filling_the_window_is_allowed() {
        let mut w = RecvWindow::new(1000);
        assert!(w.on_data(1000).is_ok());
        assert_eq!(w.peer_credit(), 0);
    }

    #[test]
    fn credit_is_returned_only_once_it_is_worth_a_frame() {
        let mut w = RecvWindow::new(1000);
        w.on_data(1000).unwrap();

        // Below the half-window threshold: no frame yet.
        assert_eq!(w.on_consumed(100), None);
        assert_eq!(w.on_consumed(399), None);
        // Crossing it returns everything accumulated.
        assert_eq!(w.on_consumed(1), Some(500));
        assert_eq!(w.peer_credit(), 500);
    }

    #[test]
    fn a_full_drain_returns_the_whole_window() {
        let mut w = RecvWindow::new(1000);
        w.on_data(1000).unwrap();
        let returned: u32 = (0..10).filter_map(|_| w.on_consumed(100)).sum();
        assert_eq!(returned, 1000);
        assert_eq!(w.peer_credit(), 1000, "window should be whole again");
        assert_eq!(w.in_flight(), 0);
    }

    #[test]
    fn flush_releases_a_remainder_the_threshold_would_strand() {
        // The deadlock case: the peer waits for credit that never reaches the
        // threshold because the last read was small.
        let mut w = RecvWindow::new(1000);
        w.on_data(1000).unwrap();
        assert_eq!(w.on_consumed(10), None);
        assert_eq!(w.flush(), Some(10));
        assert_eq!(w.peer_credit(), 10);
        // Nothing pending now.
        assert_eq!(w.flush(), None);
    }

    #[test]
    fn a_tiny_window_still_makes_progress() {
        // threshold must never round to zero, or credit would return on every
        // byte — or worse, never.
        let mut w = RecvWindow::new(1);
        w.on_data(1).unwrap();
        assert_eq!(w.on_consumed(1), Some(1));
        assert_eq!(w.peer_credit(), 1);
    }

    #[test]
    fn zero_length_arrivals_and_reads_are_harmless() {
        let mut w = RecvWindow::new(1000);
        assert!(w.on_data(0).is_ok());
        assert_eq!(w.on_consumed(0), None);
        assert_eq!(w.peer_credit(), 1000);
    }

    #[test]
    fn accounting_holds_over_many_cycles() {
        let mut w = RecvWindow::new(4096);
        let mut granted_back = 0u64;
        let mut sent = 0u64;
        for _ in 0..1000 {
            // The peer sends as much as it may, we read it all back.
            let chunk = w.peer_credit().min(1024);
            if chunk > 0 {
                w.on_data(chunk).unwrap();
                sent += u64::from(chunk);
                if let Some(g) = w.on_consumed(chunk) {
                    granted_back += u64::from(g);
                }
            }
        }
        if let Some(g) = w.flush() {
            granted_back += u64::from(g);
        }
        assert_eq!(sent, granted_back, "every consumed byte must be re-granted");
        assert_eq!(w.peer_credit(), 4096, "the window must return to full");
    }
}
