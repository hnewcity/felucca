//! Reliable UDP driven by one owned Tokio task per KCP session.
//! All queues and sessions are bounded. Socket backpressure drops UDP packets;
//! KCP retransmission recovers them without an unbounded delayed-send queue.
use felucca_core::config::KcpConfig;
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf},
    net::UdpSocket,
    sync::mpsc,
    task::{JoinHandle, JoinSet},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

const INPUT_QUEUE: usize = 32;
const PIPE_SIZE: usize = 32 * 1024;
#[path = "kcp_fec.rs"]
mod fec;

#[derive(Debug)]
pub struct KcpStream {
    io: DuplexStream,
    close: CancellationToken,
    local: SocketAddr,
}
impl KcpStream {
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }
}
impl Drop for KcpStream {
    fn drop(&mut self) {
        self.close.cancel();
    }
}
impl AsyncRead for KcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}
impl AsyncWrite for KcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[derive(Debug)]
pub struct KcpListener {
    address: SocketAddr,
    accepted: mpsc::Receiver<(KcpStream, SocketAddr)>,
    driver: JoinHandle<()>,
}
impl Drop for KcpListener {
    fn drop(&mut self) {
        self.driver.abort();
    }
}
impl KcpListener {
    pub async fn bind(config: &KcpConfig, address: SocketAddr) -> io::Result<Self> {
        config.validate().map_err(io::Error::other)?;
        let socket = Arc::new(crate::udp::bind_plain(address).await?);
        let address = socket.local_addr()?;
        let (tx, accepted) = mpsc::channel(config.max_sessions.min(128));
        let config = config.clone();
        let driver = tokio::spawn(async move {
            let mut sessions: HashMap<SocketAddr, (u32, mpsc::Sender<Vec<u8>>)> = HashMap::new();
            let mut tasks = JoinSet::new();
            let mut buffer = [0u8; 1473];
            let mut receiver = crate::udp::Receiver::new();
            let mut cleanup = tokio::time::interval(Duration::from_millis(100));
            cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = cleanup.tick() => sessions.retain(|_, (_, tx)| !tx.is_closed()),
                    result = tasks.join_next(), if !tasks.is_empty() => {
                        if let Some(Err(error)) = result && !error.is_cancelled() { tracing::warn!(%error, "KCP driver failed"); }
                    },
                    (len, peer) = receiver.recv_from(&socket, &mut buffer) => {
                        let packet = &buffer[..len];
                        let Some(envelope) = fec::inspect(packet, &config) else { continue; };
                        let conv = envelope.conv;
                        if envelope.data.is_some_and(|data| valid_packet(data, fec::raw_mtu(&config)) != Some(conv)) { continue; }
                        if sessions.get(&peer).is_some_and(|(_, tx)| tx.is_closed()) { sessions.remove(&peer); }
                        if let Some((active, input)) = sessions.get(&peer) {
                            if *active == conv { let _ = input.try_send(packet.to_vec()); }
                            continue;
                        }
                        // A new session must start with an unfragmented data
                        // segment at sequence zero, not arbitrary ACK traffic.
                        let Some(data) = envelope.data else { continue; };
                        if sessions.len() >= config.max_sessions || data[4] != 81 || data[12..16] != [0; 4] { continue; }
                        let Ok(permit) = tx.try_reserve() else { continue; };
                        let (input, rx) = mpsc::channel(INPUT_QUEUE);
                        let (io, app) = tokio::io::duplex(PIPE_SIZE);
                        let close = CancellationToken::new();
                        let driver = tasks.spawn(drive(config.clone(), socket.clone(), peer, conv, app, Some(rx), close.clone()));
                        if input.try_send(packet.to_vec()).is_err() { driver.abort(); continue; }
                        sessions.insert(peer, (conv, input));
                        permit.send((KcpStream { io, close, local: address }, peer));
                    }
                }
            }
        });
        Ok(Self {
            address,
            accepted,
            driver,
        })
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }
    pub async fn accept(&mut self) -> io::Result<(KcpStream, SocketAddr)> {
        self.accepted
            .recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "KCP listener closed"))
    }
}

pub async fn connect(
    config: &KcpConfig,
    address: SocketAddr,
    local_ip: Option<IpAddr>,
) -> io::Result<KcpStream> {
    config.validate().map_err(io::Error::other)?;
    if local_ip.is_some_and(|ip| ip.is_ipv4() != address.is_ipv4()) {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "KCP local_ip has the wrong address family",
        ));
    }
    let local = local_ip.unwrap_or(if address.is_ipv4() {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        Ipv6Addr::UNSPECIFIED.into()
    });
    let socket = Arc::new(UdpSocket::bind(SocketAddr::new(local, 0)).await?);
    let local = socket.local_addr()?;
    // A connected UDP socket also rejects packets from unrelated peers.
    socket.connect(address).await?;
    let mut conv = rand::random::<u32>();
    while conv == 0 {
        conv = rand::random();
    }
    let (io, app) = tokio::io::duplex(PIPE_SIZE);
    let close = CancellationToken::new();
    tokio::spawn(drive(
        config.clone(),
        socket,
        address,
        conv,
        app,
        None,
        close.clone(),
    ));
    Ok(KcpStream { io, close, local })
}

struct Output<S> {
    send: S,
    fec: fec::Encoder,
}
impl<S: FnMut(&[u8]) -> io::Result<usize>> io::Write for Output<S> {
    fn write(&mut self, packet: &[u8]) -> io::Result<usize> {
        for frame in self.fec.encode(packet)? {
            let sent = (self.send)(&frame);
            match sent {
                Ok(size) if size == frame.len() => {}
                Ok(_) => return Err(io::ErrorKind::WriteZero.into()),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        Ok(packet.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

enum Input {
    Socket(Arc<UdpSocket>),
    Queue(mpsc::Receiver<Vec<u8>>),
}

impl Input {
    async fn receive(&mut self, buffer: &mut [u8]) -> io::Result<Option<Vec<u8>>> {
        match self {
            Self::Socket(socket) => {
                let size = socket.recv(buffer).await?;
                Ok(Some(buffer[..size].to_vec()))
            }
            Self::Queue(receiver) => Ok(receiver.recv().await),
        }
    }
}

fn valid_packet(mut packet: &[u8], mtu: usize) -> Option<u32> {
    if !(kcp::KCP_OVERHEAD..=mtu).contains(&packet.len()) {
        return None;
    }
    let conv = u32::from_le_bytes(packet[..4].try_into().ok()?);
    if conv == 0 {
        return None;
    }
    while !packet.is_empty() {
        if packet.len() < kcp::KCP_OVERHEAD
            || u32::from_le_bytes(packet[..4].try_into().ok()?) != conv
            || !matches!(packet[4], 81..=84)
            || packet[5] != 0
        {
            return None;
        }
        let size = u32::from_le_bytes(packet[20..24].try_into().ok()?) as usize;
        if (packet[4] != 81 && size != 0) || size > packet.len() - kcp::KCP_OVERHEAD {
            return None;
        }
        packet = &packet[kcp::KCP_OVERHEAD + size..];
    }
    Some(conv)
}

async fn drive(
    config: KcpConfig,
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    conv: u32,
    app: DuplexStream,
    input: Option<mpsc::Receiver<Vec<u8>>>,
    close: CancellationToken,
) {
    let connected = input.is_none();
    let input = match input {
        Some(input) => Input::Queue(input),
        None => Input::Socket(socket.clone()),
    };
    let send = move |packet: &[u8]| {
        if connected {
            socket.try_send(packet)
        } else {
            socket.try_send_to(packet, peer)
        }
    };
    if let Err(error) = drive_session(config, conv, app, input, send, close).await {
        tracing::debug!(%peer, %error, "KCP session closed");
    }
}

async fn drive_session<S: FnMut(&[u8]) -> io::Result<usize> + Send>(
    config: KcpConfig,
    conv: u32,
    app: DuplexStream,
    mut input: Input,
    send: S,
    close: CancellationToken,
) -> io::Result<()> {
    let raw_mtu = fec::raw_mtu(&config);
    let mut decoder = fec::Decoder::new(&config, conv)?;
    let mut kcp = kcp::Kcp::new_stream(
        conv,
        Output {
            send,
            fec: fec::Encoder::new(&config, conv)?,
        },
    );
    kcp.set_mtu(raw_mtu).map_err(io::Error::other)?;
    kcp.set_wndsize(config.send_window, config.receive_window);
    kcp.set_nodelay(
        config.nodelay,
        config.interval_ms as i32,
        config.fast_resend as i32,
        !config.congestion_control,
    );
    kcp.update(0).map_err(io::Error::other)?;
    let mut tick = tokio::time::interval(Duration::from_millis(config.interval_ms as u64));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let start = Instant::now();
    let mut touched = start;
    let (mut app_read, mut app_write) = tokio::io::split(app);
    let mut outbound = [0u8; 1472];
    let mut inbound = [0u8; 1473];
    let mut pending = vec![0; config.mtu as usize];
    let (mut position, mut length) = (0, 0);
    let mut eof = false;
    let mut closing_at = None;
    loop {
        if close.is_cancelled() {
            closing_at.get_or_insert_with(Instant::now);
            // The application closed its read side. Continue sending its
            // already accepted writes and receiving ACKs for at most 5s.
            position = length;
        }
        if position == length && kcp.peeksize().is_ok() {
            length = kcp.recv(&mut pending).map_err(io::Error::other)?;
            position = 0;
        }
        let datagram = tokio::select! {
            biased;
            _ = tick.tick() => {
                if touched.elapsed() >= config.session_timeout || kcp.is_dead_link()
                    || closing_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(5)) {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "KCP session expired"));
                }
                kcp.update(start.elapsed().as_millis() as u32).map_err(io::Error::other)?;
                None
            },
            result = app_write.write(&pending[position..length]), if position < length && !close.is_cancelled() => {
                let size = match result {
                    Ok(size) => size,
                    Err(_) if close.is_cancelled() => { position = length; continue; },
                    Err(error) => return Err(error),
                };
                if size == 0 { return Err(io::ErrorKind::WriteZero.into()); }
                position += size;
                None
            },
            result = app_read.read(&mut outbound[..raw_mtu - kcp::KCP_OVERHEAD]), if !eof && kcp.wait_snd() < config.send_window as usize => {
                let size = result?;
                if size == 0 { eof = true; }
                else {
                    kcp.update(start.elapsed().as_millis() as u32).map_err(io::Error::other)?;
                    kcp.send(&outbound[..size]).map_err(io::Error::other)?;
                    kcp.flush().map_err(io::Error::other)?;
                }
                None
            },
            packet = input.receive(&mut inbound) => {
                let Some(packet) = packet? else { return Ok(()); };
                Some(packet)
            },
        };
        if let Some(packet) = datagram {
            let mut received = false;
            for packet in decoder.input(&packet, Instant::now()).unwrap_or_default() {
                if valid_packet(&packet, raw_mtu) != Some(conv) {
                    continue;
                }
                if !received {
                    // input() measures RTT against KCP's current clock. A
                    // timer-only update understates RTT by up to one interval.
                    kcp.update(start.elapsed().as_millis() as u32)
                        .map_err(io::Error::other)?;
                    received = true;
                }
                kcp.input(&packet).map_err(io::Error::other)?;
                touched = Instant::now();
            }
            if received {
                // kcp 0.6 flush_ack() only encodes ACKs into its internal
                // buffer. Flush that buffer now so small ACKs do not wait for
                // the next timer tick and cause avoidable retransmissions.
                kcp.flush().map_err(io::Error::other)?;
            }
        }
        if eof && kcp.wait_snd() == 0 {
            // KCP has no transport FIN. Felucca-mux carries stream FIN and
            // session shutdown, while expiry detects an unreachable peer.
            return Ok(());
        }
    }
}

#[cfg(test)]
#[path = "kcp_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "kcp_simulation_tests.rs"]
mod simulation_tests;
