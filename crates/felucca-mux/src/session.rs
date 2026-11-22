//! Session: drives one transport connection and the streams riding on it.
//!
//! Two tasks own the transport — one reading, one writing — so a slow reader on
//! one stream never blocks writes for another. Everything else talks to them
//! through an [`Outbound`] queue and a registry of live streams.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::time::Instant;

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

use crate::config::Config;
use crate::error::MuxError;
use crate::frame::{CloseReason, FrameHeader, FrameType, HEADER_LEN};
use crate::rtt::RttTracker;
use crate::stream::{Stream, StreamShared};

/// Which side of a session this endpoint is.
///
/// It decides stream-id parity, so both sides can open streams at once without
/// coordinating and without ever colliding on an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The side that dialled. Uses odd stream ids.
    Client,
    /// The side that accepted. Uses even stream ids.
    Server,
}

impl Role {
    fn first_id(self) -> u32 {
        match self {
            Self::Client => 1,
            Self::Server => 2,
        }
    }

    /// Whether `id` belongs to this role's half of the id space.
    fn owns(self, id: u32) -> bool {
        match self {
            Self::Client => !id.is_multiple_of(2),
            Self::Server => id.is_multiple_of(2),
        }
    }
}

/// A cheap handle for queueing frames toward the peer.
///
/// Each queued frame owns a budget permit even though the channel itself is
/// unbounded. DATA waits for byte permits. Control frames use a separate budget;
/// exhaustion terminates the session instead of silently dropping credit.
#[derive(Debug, Clone)]
pub(crate) struct Outbound {
    tx: mpsc::UnboundedSender<Queued>,
    pub(crate) data: Arc<Semaphore>,
    controls: Arc<Semaphore>,
    cancel: tokio_util::sync::CancellationToken,
}

#[derive(Debug)]
struct Queued {
    header: FrameHeader,
    payload: Bytes,
    permit: OwnedSemaphorePermit,
}

/// One frame per active stream per turn. In-stream order includes OPEN and
/// CLOSE, so scheduling cannot truncate data or reorder half-closes. New lanes
/// enter in enqueue order, preserving the monotonically increasing OPEN ids.
#[derive(Default)]
struct SendQueue {
    ready: VecDeque<u32>,
    streams: HashMap<u32, VecDeque<Queued>>,
}

impl SendQueue {
    fn push(&mut self, frame: Queued) {
        let id = frame.header.stream;
        let frames = self.streams.entry(id).or_insert_with(|| {
            self.ready.push_back(id);
            VecDeque::new()
        });
        frames.push_back(frame);
    }

    fn pop(&mut self) -> Option<Queued> {
        let id = self.ready.pop_front()?;
        let frames = self.streams.get_mut(&id).expect("ready stream has frames");
        let frame = frames.pop_front();
        if frames.is_empty() {
            self.streams.remove(&id);
        } else {
            self.ready.push_back(id);
        }
        frame
    }
}

impl Outbound {
    /// Queues a frame. Returns `false` once the session is gone.
    pub(crate) fn send(&self, header: FrameHeader, payload: Bytes) -> bool {
        debug_assert!(header.ty != FrameType::Data);
        let Ok(permit) = self.controls.clone().try_acquire_owned() else {
            // Never silently drop a credit update: terminate and wake every
            // waiter when control traffic exhausts its separate budget.
            self.cancel.cancel();
            return false;
        };
        self.tx
            .send(Queued {
                header,
                payload,
                permit,
            })
            .is_ok()
    }

    pub(crate) fn send_data(
        &self,
        header: FrameHeader,
        payload: Bytes,
        permit: OwnedSemaphorePermit,
    ) -> bool {
        self.tx
            .send(Queued {
                header,
                payload,
                permit,
            })
            .is_ok()
    }
}

#[derive(Debug)]
struct Shared {
    config: Config,
    role: Role,
    streams: Mutex<HashMap<u32, Arc<StreamShared>>>,
    next_id: AtomicU32,
    last_peer_id: AtomicU32,
    outbound: Outbound,
    closed: Mutex<Option<CloseReason>>,
    cancel: tokio_util::sync::CancellationToken,
    rtt: Arc<Mutex<RttTracker>>,
    last_received: Mutex<Instant>,
    budget: Arc<crate::budget::WindowBudget>,
}

impl Shared {
    /// Marks the session dead and tears down every live stream.
    ///
    /// Streams have to be failed explicitly: a reader parked on a stream whose
    /// transport just died would otherwise wait forever for data that can no
    /// longer arrive.
    fn shutdown(&self, reason: CloseReason) {
        {
            let mut closed = self.closed.lock();
            if closed.is_some() {
                return;
            }
            *closed = Some(reason);
        }
        self.cancel.cancel();
        self.outbound.data.close();
        self.outbound.controls.close();
        let streams: Vec<_> = self.streams.lock().drain().map(|(_, s)| s).collect();
        for s in streams {
            s.on_closed(reason);
        }
    }

    fn close_reason(&self) -> Option<CloseReason> {
        *self.closed.lock()
    }
}

/// A multiplexed session.
///
/// Accepting streams needs `&mut`, since only one acceptor makes sense; opening
/// them goes through a cloneable [`SessionHandle`].
#[derive(Debug)]
pub struct Session {
    shared: Arc<Shared>,
    accept_rx: mpsc::Receiver<Stream>,
}

impl Session {
    /// Starts a session over `io`, spawning the tasks that drive it.
    pub fn new<T>(io: T, config: Config, role: Role) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        let config = config.normalized();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (accept_tx, accept_rx) = mpsc::channel(config.accept_backlog);
        let cancel = tokio_util::sync::CancellationToken::new();

        let shared = Arc::new(Shared {
            role,
            streams: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(role.first_id()),
            last_peer_id: AtomicU32::new(0),
            outbound: Outbound {
                tx: out_tx,
                data: Arc::new(Semaphore::new(config.max_send_queue)),
                controls: Arc::new(Semaphore::new(config.max_control_queue)),
                cancel: cancel.clone(),
            },
            closed: Mutex::new(None),
            cancel,
            rtt: Arc::new(Mutex::new(RttTracker::new())),
            last_received: Mutex::new(Instant::now()),
            budget: crate::budget::WindowBudget::new(config.max_session_window),
            config,
        });

        let (reader, writer) = tokio::io::split(io);
        let writer_shared = shared.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = writer_shared.cancel.cancelled() => writer_shared.shutdown(CloseReason::ProtocolError),
                () = write_loop(writer_shared.clone(), writer, out_rx) => {},
            }
        });
        let reader_shared = shared.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = reader_shared.cancel.cancelled() => reader_shared.shutdown(CloseReason::ProtocolError),
                () = read_loop(reader_shared.clone(), reader, accept_tx) => {},
            }
        });
        tokio::spawn(keepalive_loop(Arc::clone(&shared)));

        Self { shared, accept_rx }
    }

    /// A handle for opening streams, usable from any task.
    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Opens a stream.
    pub fn open(&self) -> Result<Stream, MuxError> {
        self.handle().open()
    }

    /// Waits for the peer to open a stream.
    pub async fn accept(&mut self) -> Result<Stream, MuxError> {
        match self.accept_rx.recv().await {
            Some(s) => Ok(s),
            None => Err(MuxError::SessionClosed {
                reason: self
                    .shared
                    .close_reason()
                    .unwrap_or(CloseReason::TransportFailure),
            }),
        }
    }

    /// Smoothed round-trip time, once a ping has been answered.
    pub fn rtt(&self) -> Option<Duration> {
        self.shared.rtt.lock().smoothed()
    }

    /// Live stream count.
    pub fn stream_count(&self) -> usize {
        self.shared.streams.lock().len()
    }

    pub fn reserved_receive_bytes(&self) -> u64 {
        self.shared.budget.used()
    }

    pub fn queued_data_bytes(&self) -> usize {
        self.shared.config.max_send_queue - self.shared.outbound.data.available_permits()
    }

    pub fn is_closed(&self) -> bool {
        self.shared.close_reason().is_some()
    }

    /// Tells the peer we are going away, then tears the session down.
    pub fn close(&self, reason: CloseReason) {
        self.shared
            .outbound
            .send(FrameHeader::go_away(reason), Bytes::new());
        self.shared.shutdown(reason);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shared.shutdown(CloseReason::Shutdown);
    }
}

/// A cloneable handle for opening streams on a live session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    shared: Arc<Shared>,
}

impl SessionHandle {
    /// Opens a stream.
    ///
    /// Optimistic: one segment can travel with OPEN before the peer acknowledges
    /// its actual receive window. Missing acknowledgement expires at open_timeout.
    pub fn open(&self) -> Result<Stream, MuxError> {
        if let Some(reason) = self.shared.close_reason() {
            return Err(MuxError::SessionClosed { reason });
        }

        let cfg = &self.shared.config;
        let shared = {
            let mut streams = self.shared.streams.lock();
            if streams.len() >= cfg.max_streams {
                return Err(MuxError::OpenRejected {
                    reason: CloseReason::TooManyStreams,
                });
            }
            // Step by two to stay inside this role's parity.
            let Ok(id) =
                self.shared
                    .next_id
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(2))
            else {
                // The peer rejects reused ids, so a session that has opened
                // 2^31 streams (about 25 days at 1,000/s) cannot continue.
                // Ending it lets the agent reconnect instead of leaving a live
                // session that refuses every new stream.
                drop(streams);
                tracing::warn!("mux stream ids exhausted; closing session to reconnect");
                self.close(CloseReason::Shutdown);
                return Err(MuxError::SessionClosed {
                    reason: CloseReason::Shutdown,
                });
            };
            let s = StreamShared::new(
                id,
                crate::frame::MAX_SEGMENT as u32,
                cfg,
                self.shared.outbound.clone(),
                self.shared.budget.clone(),
                self.shared.rtt.clone(),
                true,
            )
            .ok_or(MuxError::OpenRejected {
                reason: CloseReason::TooManyStreams,
            })?;
            streams.insert(id, Arc::clone(&s));
            // Keep OPEN enqueue order identical to id allocation order even
            // when several application tasks open concurrently.
            self.shared.outbound.send(
                FrameHeader::open_with_window(s.id(), cfg.initial_stream_window),
                Bytes::new(),
            );
            s
        };

        Ok(Stream::new(shared))
    }

    pub fn close(&self, reason: CloseReason) {
        self.shared
            .outbound
            .send(FrameHeader::go_away(reason), Bytes::new());
        self.shared.shutdown(reason);
    }

    pub fn is_closed(&self) -> bool {
        self.shared.close_reason().is_some()
    }
}

/// Drains queued frames onto the transport.
async fn write_loop<W>(shared: Arc<Shared>, mut writer: W, mut rx: mpsc::UnboundedReceiver<Queued>)
where
    W: AsyncWrite + Unpin,
{
    // Coalesce whatever is already queued into one buffer before touching the
    // socket: a bulk transfer generates many frames per scheduler turn, and one
    // write syscall per frame would dominate the cost.
    let mut batch = Vec::with_capacity(64 * 1024);
    let mut permits = Vec::new();
    let mut queued = SendQueue::default();

    loop {
        if queued.ready.is_empty() {
            let Some(first) = rx.recv().await else { break };
            queued.push(first);
        }
        // All frames retain their original permits while scheduled: this adds
        // fairness without expanding either the data or control budget.
        while let Ok(frame) = rx.try_recv() {
            queued.push(frame);
        }
        batch.clear();
        while let Some(Queued {
            header,
            payload,
            permit,
        }) = queued.pop()
        {
            permits.push(permit);
            batch.extend_from_slice(&header.encode());
            batch.extend_from_slice(&payload);

            // A stream is done once its close goes out; leaving the entry would
            // leak an allocation per connection on a long-lived session.
            if header.ty == FrameType::Close {
                shared.streams.lock().remove(&header.stream);
            }

            if batch.len() >= 64 * 1024 {
                break;
            }
        }

        if writer.write_all(&batch).await.is_err() || writer.flush().await.is_err() {
            shared.shutdown(CloseReason::TransportFailure);
            return;
        }
        permits.clear();
    }
    // The queue closed, which happens only once every sender is gone.
    let _ = writer.shutdown().await;
}

/// Reads frames off the transport and dispatches them.
async fn read_loop<R>(shared: Arc<Shared>, mut reader: R, accept_tx: mpsc::Sender<Stream>)
where
    R: AsyncRead + Unpin,
{
    let reason = match dispatch_frames(&shared, &mut reader, &accept_tx).await {
        Ok(reason) => reason,
        Err(MuxError::Protocol(msg)) => {
            tracing::warn!(error = msg, "mux protocol violation; closing session");
            shared.outbound.send(
                FrameHeader::go_away(CloseReason::ProtocolError),
                Bytes::new(),
            );
            CloseReason::ProtocolError
        }
        Err(e) => {
            if !e.is_graceful() {
                tracing::debug!(error = %e, "mux transport failed");
            }
            CloseReason::TransportFailure
        }
    };
    shared.shutdown(reason);
}

async fn dispatch_frames<R>(
    shared: &Arc<Shared>,
    reader: &mut R,
    accept_tx: &mpsc::Sender<Stream>,
) -> Result<CloseReason, MuxError>
where
    R: AsyncRead + Unpin,
{
    let mut head = [0u8; HEADER_LEN];
    loop {
        match reader.read_exact(&mut head).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(CloseReason::Normal);
            }
            Err(e) => return Err(e.into()),
        }
        let header = FrameHeader::decode(&head)?;

        // Always consume the payload, even for a stream that no longer exists:
        // leaving it on the wire would desynchronise every later frame.
        let payload = if header.payload_len() > 0 {
            let mut buf = vec![0u8; header.payload_len()];
            reader.read_exact(&mut buf).await?;
            Bytes::from(buf)
        } else {
            Bytes::new()
        };

        let lookup = |id: u32| shared.streams.lock().get(&id).map(Arc::clone);

        match header.ty {
            FrameType::Data => {
                if let Some(s) = lookup(header.stream)
                    && let Err(v) = s.on_data(payload)
                {
                    tracing::warn!(
                        stream = header.stream,
                        got = v.got,
                        allowed = v.allowed,
                        "peer exceeded its receive credit"
                    );
                    return Err(MuxError::Protocol("peer exceeded its receive credit"));
                }
            }

            FrameType::WindowUpdate => {
                if let Some(s) = lookup(header.stream) {
                    s.on_window_update(header.value).map_err(|_| {
                        MuxError::Protocol("credit grant exceeds outstanding bytes")
                    })?;
                }
            }

            FrameType::OpenAck => {
                if let Some(s) = lookup(header.stream) {
                    s.on_open_ack(header.value)?;
                }
            }
            FrameType::WindowGrow => {
                if let Some(s) = lookup(header.stream) {
                    s.on_window_grow(header.value)?;
                }
            }

            FrameType::Open => {
                // The peer must open from its own half of the id space, or the
                // two sides could pick the same id for different streams.
                if shared.role.owns(header.stream) {
                    return Err(MuxError::Protocol("peer opened a stream in our id space"));
                }
                if header.stream <= shared.last_peer_id.load(Ordering::Relaxed) {
                    return Err(MuxError::Protocol("peer reused or reordered a stream id"));
                }
                shared.last_peer_id.store(header.stream, Ordering::Relaxed);
                let cfg = &shared.config;
                let new = {
                    let mut streams = shared.streams.lock();
                    if streams.contains_key(&header.stream) {
                        return Err(MuxError::Protocol("peer reused a live stream id"));
                    }
                    if streams.len() >= cfg.max_streams {
                        None
                    } else {
                        let s = StreamShared::new(
                            header.stream,
                            header.value,
                            cfg,
                            shared.outbound.clone(),
                            shared.budget.clone(),
                            shared.rtt.clone(),
                            false,
                        );
                        if let Some(s) = &s {
                            streams.insert(header.stream, Arc::clone(s));
                        }
                        s
                    }
                };

                match new {
                    None => {
                        shared.outbound.send(
                            FrameHeader::open_nack(header.stream, CloseReason::TooManyStreams),
                            Bytes::new(),
                        );
                    }
                    Some(s) => {
                        // A full backlog means the application is not accepting.
                        // Refusing beats queueing without bound.
                        match accept_tx.try_reserve() {
                            Ok(permit) => {
                                shared.outbound.send(
                                    FrameHeader::open_ack(header.stream, cfg.initial_stream_window),
                                    Bytes::new(),
                                );
                                permit.send(Stream::new(s));
                            }
                            Err(_) => {
                                shared.streams.lock().remove(&header.stream);
                                s.on_closed(CloseReason::TooManyStreams);
                                shared.outbound.send(
                                    FrameHeader::open_nack(
                                        header.stream,
                                        CloseReason::TooManyStreams,
                                    ),
                                    Bytes::new(),
                                );
                            }
                        }
                    }
                }
            }

            FrameType::OpenNack => {
                if let Some(s) = shared.streams.lock().remove(&header.stream) {
                    s.on_closed(header.reason());
                }
            }

            FrameType::CloseWrite => {
                if let Some(s) = lookup(header.stream) {
                    s.on_peer_finished();
                }
            }

            FrameType::Close => {
                if let Some(s) = shared.streams.lock().remove(&header.stream) {
                    s.on_closed(header.reason());
                }
            }

            FrameType::Ping => {
                shared
                    .outbound
                    .send(FrameHeader::pong(header.value), Bytes::new());
            }

            FrameType::Pong => {
                shared
                    .rtt
                    .lock()
                    .observe_pong(header.value, Instant::now().into_std());
            }

            FrameType::GoAway => return Ok(header.reason()),
        }
        // A complete, valid frame proves receive-side progress even when a
        // pong is still behind DATA in the peer's transport buffers.
        *shared.last_received.lock() = Instant::now();
    }
}

/// Sends pings and declares the session dead if one goes unanswered.
async fn keepalive_loop(shared: Arc<Shared>) {
    let cfg = &shared.config;
    let mut nonce = 0u32;
    let mut ticker = tokio::time::interval(
        cfg.keepalive_interval
            .min(cfg.open_timeout / 2)
            .max(Duration::from_millis(1)),
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_ping = None;

    loop {
        tokio::select! {
            () = shared.cancel.cancelled() => return,
            _ = ticker.tick() => {},
        }
        if shared.close_reason().is_some() {
            return;
        }

        // The deadline scales with observed RTT, so a slow link is not mistaken
        // for a dead one.
        let now = Instant::now();
        let expired: Vec<_> = shared
            .streams
            .lock()
            .values()
            .filter(|s| s.open_expired(cfg.open_timeout))
            .map(Arc::clone)
            .collect();
        for stream in expired {
            shared.streams.lock().remove(&stream.id());
            stream.on_closed(CloseReason::Timeout);
            shared.outbound.send(
                FrameHeader::close(stream.id(), CloseReason::Timeout),
                Bytes::new(),
            );
        }
        let (deadline, waiting) = {
            let rtt = shared.rtt.lock();
            (
                rtt.pong_deadline(cfg.keepalive_interval, cfg.keepalive_timeout),
                rtt.pending_age(now.into_std()),
            )
        };

        if let Some(waiting) = waiting {
            let idle = now.saturating_duration_since(*shared.last_received.lock());
            if waiting > deadline && idle > deadline {
                tracing::debug!(?waiting, ?deadline, "mux keepalive timed out");
                shared.shutdown(CloseReason::Timeout);
                return;
            }
            // One ping in flight is all it takes to tell a live peer from a dead
            // one. Sending another would only add a nonce to ignore.
            continue;
        }

        if last_ping.is_some_and(|last: Instant| last.elapsed() < cfg.keepalive_interval) {
            continue;
        }
        last_ping = Some(now);
        nonce = nonce.wrapping_add(1);
        shared.rtt.lock().record_ping(nonce, now.into_std());
        if !shared.outbound.send(FrameHeader::ping(nonce), Bytes::new()) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exhausting_stream_ids_closes_both_ends() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let client = Session::new(a, Config::default(), Role::Client);
        let mut server = Session::new(b, Config::default(), Role::Server);
        client.shared.next_id.store(u32::MAX - 2, Ordering::Relaxed);
        let last = client.open().unwrap();
        assert_eq!(last.id(), u32::MAX - 2);
        assert!(matches!(
            client.open(),
            Err(MuxError::SessionClosed {
                reason: CloseReason::Shutdown
            })
        ));
        assert!(client.is_closed());
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            while server.accept().await.is_ok() {}
        })
        .await;
        assert!(ended.is_ok());
        assert!(server.is_closed());
    }
}
