//! A single multiplexed stream.
//!
//! Implements [`AsyncRead`] and [`AsyncWrite`], so anything that copies between
//! sockets — `tokio::io::copy_bidirectional`, a TLS acceptor, an HTTP server —
//! works over a stream unchanged.

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::Instant;

use bytes::{Buf, Bytes, BytesMut};
use futures_util::task::AtomicWaker;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::credit::{CreditViolation, RecvWindow, SendCredit};
use crate::frame::{CloseReason, FrameHeader, MAX_SEGMENT};
use crate::session::Outbound;

/// State the session's read loop and the application's reader share.
#[derive(Debug)]
struct RecvState {
    /// Arrived but unread payload, in arrival order.
    ///
    /// Unbounded by construction yet bounded in practice: the peer cannot send
    /// more than its credit, and credit only returns as the application reads.
    /// Using a bounded queue here instead would let one slow reader stall the
    /// session's read loop and head-of-line block every other stream.
    chunks: VecDeque<BytesMut>,
    buffered: usize,
    window: RecvWindow,
    /// The peer will send no more data; reads drain and then report EOF.
    peer_finished: bool,
    /// The stream is finished in both directions.
    closed: Option<CloseReason>,
    consumed_sample: u64,
    sample_at: Instant,
}

/// Shared between the [`Stream`] handle and the session driving it.
#[derive(Debug)]
pub(crate) struct StreamShared {
    id: u32,
    credit: SendCredit,
    recv: Mutex<RecvState>,
    /// Woken when data arrives, the peer finishes, or the stream closes.
    read_waker: AtomicWaker,
    /// Set once this side has sent a close, so teardown is not sent twice.
    close_sent: AtomicBool,
    outbound: Outbound,
    opening: Mutex<Option<Instant>>,
    max_window: u32,
    budget: Arc<crate::budget::WindowBudget>,
    rtt: Arc<Mutex<crate::RttTracker>>,
}

impl StreamShared {
    pub(crate) fn new(
        id: u32,
        peer_window: u32,
        config: &crate::Config,
        outbound: Outbound,
        budget: Arc<crate::budget::WindowBudget>,
        rtt: Arc<Mutex<crate::RttTracker>>,
        pending: bool,
    ) -> Option<Arc<Self>> {
        if !budget.take(config.initial_stream_window) {
            return None;
        }
        Some(Arc::new(Self {
            id,
            credit: SendCredit::paced(peer_window, rtt.clone()),
            recv: Mutex::new(RecvState {
                chunks: VecDeque::new(),
                buffered: 0,
                window: RecvWindow::new(config.initial_stream_window),
                peer_finished: false,
                closed: None,
                consumed_sample: 0,
                sample_at: Instant::now(),
            }),
            read_waker: AtomicWaker::new(),
            close_sent: AtomicBool::new(false),
            outbound,
            opening: Mutex::new(pending.then(Instant::now)),
            max_window: config.max_stream_window,
            budget,
            rtt,
        }))
    }

    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    /// Records payload arriving from the peer.
    ///
    /// `Err` means the peer overran its credit — a protocol violation the
    /// session must escalate, since absorbing it would defeat the memory bound.
    pub(crate) fn on_data(&self, mut payload: Bytes) -> Result<(), CreditViolation> {
        {
            let mut st = self.recv.lock();
            // Data after a close is discarded rather than rejected: the peer may
            // simply not have seen our close yet.
            if st.closed.is_some() || st.peer_finished {
                return Ok(());
            }
            st.window.on_data(payload.len() as u32)?;
            if !payload.is_empty() {
                st.buffered += payload.len();
                // A byte of credit must not buy an entire heap allocation:
                // coalesce tiny frames into segment-sized receive chunks.
                while !payload.is_empty() {
                    if st
                        .chunks
                        .back()
                        .is_none_or(|chunk| chunk.len() == MAX_SEGMENT)
                    {
                        st.chunks.push_back(BytesMut::with_capacity(MAX_SEGMENT));
                    }
                    let chunk = st.chunks.back_mut().expect("created a receive chunk");
                    let n = payload.len().min(MAX_SEGMENT - chunk.len());
                    chunk.extend_from_slice(&payload[..n]);
                    payload.advance(n);
                }
            }
        }
        self.read_waker.wake();
        Ok(())
    }

    /// Records credit granted by the peer.
    pub(crate) fn on_window_update(&self, credit: u32) -> Result<(), CreditViolation> {
        self.credit.grant(credit)
    }

    pub(crate) fn on_open_ack(&self, window: u32) -> Result<(), crate::MuxError> {
        if self.opening.lock().take().is_none() {
            return Err(crate::MuxError::Protocol(
                "unexpected or repeated open acknowledgement",
            ));
        }
        self.credit
            .grow(window - MAX_SEGMENT as u32)
            .map_err(|_| crate::MuxError::Protocol("invalid acknowledged window"))
    }

    pub(crate) fn on_window_grow(&self, additional: u32) -> Result<(), crate::MuxError> {
        if self.opening.lock().is_some() {
            return Err(crate::MuxError::Protocol(
                "window grew before open acknowledgement",
            ));
        }
        self.credit
            .grow(additional)
            .map_err(|_| crate::MuxError::Protocol("receive window exceeds protocol limit"))
    }

    pub(crate) fn open_expired(&self, timeout: Duration) -> bool {
        self.opening
            .lock()
            .is_some_and(|opened| opened.elapsed() >= timeout)
    }

    /// The peer finished writing; our reads drain and then see EOF.
    pub(crate) fn on_peer_finished(&self) {
        self.recv.lock().peer_finished = true;
        self.read_waker.wake();
    }

    /// The stream is over in both directions.
    ///
    /// Buffered data is kept: a peer that sends a payload and immediately closes
    /// still expects the reader to see those bytes, so the close only takes
    /// effect once the buffer drains.
    pub(crate) fn on_closed(&self, reason: CloseReason) {
        {
            let mut st = self.recv.lock();
            if st.closed.is_none() {
                st.closed = Some(reason);
            }
            st.peer_finished = true;
        }
        self.credit.close(reason);
        self.read_waker.wake();
    }

    /// Marks the stream closed locally, returning whether this call was the
    /// first to do so and therefore owes the peer a close frame.
    fn claim_close(&self) -> bool {
        !self.close_sent.swap(true, Ordering::AcqRel)
    }
}

impl Drop for StreamShared {
    fn drop(&mut self) {
        self.budget.release(self.recv.get_mut().window.capacity());
    }
}

/// One multiplexed stream.
///
/// Dropping a stream tells the peer, so the far side is not left waiting on a
/// stream nobody will read again.
#[derive(Debug)]
pub struct Stream {
    shared: Arc<StreamShared>,
    /// Set by [`AsyncWrite::poll_shutdown`]; further writes are refused.
    write_finished: bool,
    queue: tokio_util::sync::PollSemaphore,
}

impl Stream {
    pub(crate) fn new(shared: Arc<StreamShared>) -> Self {
        Self {
            queue: tokio_util::sync::PollSemaphore::new(shared.outbound.data.clone()),
            shared,
            write_finished: false,
        }
    }

    /// This stream's id on its session, for correlating logs.
    pub fn id(&self) -> u32 {
        self.shared.id
    }

    /// Bytes this side may write before waiting on the peer.
    pub fn send_credit(&self) -> u64 {
        self.shared.credit.available()
    }
    pub fn receive_window(&self) -> u32 {
        self.shared.recv.lock().window.capacity()
    }

    /// Closes the stream and tells the peer why.
    ///
    /// The explicit-reason counterpart to dropping: a caller that knows the
    /// target refused a connection can say so, which the far side sees instead
    /// of a bare cancellation.
    pub fn close_with(&mut self, reason: CloseReason) {
        if self.shared.claim_close() {
            self.shared
                .outbound
                .send(FrameHeader::close(self.shared.id, reason), Bytes::new());
        }
        self.shared.on_closed(reason);
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Dropping is how Rust code finishes with a resource, and writing a
        // reply then dropping is the common shape — the equivalent of TCP
        // `close()`, where the peer still sees a clean EOF. Reporting a fault
        // here would make every backend that closes after responding look like
        // an error to the far side. A caller that knows something went wrong
        // says so with `close_with`.
        if self.shared.claim_close() {
            self.shared.outbound.send(
                FrameHeader::close(self.shared.id, CloseReason::Normal),
                Bytes::new(),
            );
        }
        self.shared.credit.close(CloseReason::Normal);
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let shared = &self.shared;

        // Register before inspecting state, so data arriving in between wakes us.
        shared.read_waker.register(cx.waker());

        let (grant, growth) = {
            let mut st = shared.recv.lock();

            if st.chunks.is_empty() {
                // Nothing buffered: report the terminal condition, if any.
                if let Some(reason) = st.closed {
                    return if reason.is_graceful() {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Ready(Err(crate::MuxError::StreamClosed { reason }.into()))
                    };
                }
                if st.peer_finished {
                    return Poll::Ready(Ok(()));
                }
                return Poll::Pending;
            }

            // Copy out as much as the caller's buffer will take, spanning as
            // many queued chunks as fit.
            let mut taken = 0usize;
            while buf.remaining() > 0 {
                let Some(front) = st.chunks.front_mut() else {
                    break;
                };
                let n = front.len().min(buf.remaining());
                buf.put_slice(&front[..n]);
                front.advance(n);
                taken += n;
                if front.is_empty() {
                    st.chunks.pop_front();
                }
            }
            st.buffered -= taken;
            st.consumed_sample += taken as u64;

            // Every byte handed to the caller has to be accounted, or credit is
            // never returned and the peer stalls the moment it spends its
            // initial window.
            let mut grant = st.window.on_consumed(taken as u32);

            // Returning credit only at a threshold saves a frame per read, but
            // would strand a sub-threshold remainder forever. Once the buffer is
            // drained there is nothing more to wait for, so release it.
            if grant.is_none() && st.chunks.is_empty() {
                grant = st.window.flush();
            }
            let window = st.window.capacity();
            let mut growth = 0;
            if st.consumed_sample >= u64::from(window) {
                // A reader draining one window within four RTTs is keeping up
                // with the sender. Grow geometrically, reserving the entire
                // extra advertisement from the session budget first.
                let rtt = shared
                    .rtt
                    .lock()
                    .smoothed()
                    .unwrap_or(Duration::from_millis(25))
                    .max(Duration::from_millis(10));
                let extra = window.min(shared.max_window.saturating_sub(window));
                if extra > 0 && st.sample_at.elapsed() <= rtt * 4 && shared.budget.take(extra) {
                    st.window.grow(extra);
                    growth = extra;
                }
                st.sample_at = Instant::now();
                st.consumed_sample = 0;
            }
            (grant, growth)
        };

        if growth > 0 {
            shared
                .outbound
                .send(FrameHeader::window_grow(shared.id, growth), Bytes::new());
        }
        if let Some(credit) = grant {
            shared
                .outbound
                .send(FrameHeader::window_update(shared.id, credit), Bytes::new());
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_finished {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if let Err(reason) = self.shared.credit.check_open(cx) {
            return Poll::Ready(Err(crate::MuxError::StreamClosed { reason }.into()));
        }

        // One frame at a time: a larger write comes back for another turn.
        let want = buf.len().min(MAX_SEGMENT) as u32;
        let mut permit = match self
            .queue
            .poll_acquire_many(cx, want + crate::HEADER_LEN as u32)
        {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Some(permit)) => permit,
            Poll::Ready(None) => return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        };
        let allowed = match self.shared.credit.poll_reserve(cx, want) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Ok(n)) => n as usize,
            Poll::Ready(Err(reason)) => {
                return Poll::Ready(Err(crate::MuxError::StreamClosed { reason }.into()));
            }
        };
        if allowed == 0 {
            // No credit and no waker fired: treat as pending rather than
            // reporting a zero-length write, which callers read as a stall.
            return Poll::Pending;
        }

        let payload = Bytes::copy_from_slice(&buf[..allowed]);
        let permit = permit
            .split(allowed + crate::HEADER_LEN)
            .expect("reserved a complete frame");
        let id = self.shared.id;
        if !self
            .shared
            .outbound
            .send_data(FrameHeader::data(id, allowed), payload, permit)
        {
            self.write_finished = true;
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        Poll::Ready(Ok(allowed))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Frames are handed to the session's write loop, which flushes the
        // transport whenever its queue drains. There is nothing held back here.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write_finished {
            return Poll::Ready(Ok(()));
        }
        self.write_finished = true;

        // Half-close: stop writing, keep reading. A proxied protocol that
        // signals end-of-request by shutting down its write half — an HTTP
        // request body, or `nc -N` — depends on this reaching the far side
        // without tearing down the response direction.
        let id = self.shared.id;
        self.shared
            .outbound
            .send(FrameHeader::close_write(id), Bytes::new());
        self.shared.credit.close(CloseReason::Normal);
        Poll::Ready(Ok(()))
    }
}
