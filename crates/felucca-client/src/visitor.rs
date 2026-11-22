//! Local visitor listeners share the agent's authenticated bridge.
use felucca_core::{
    Acl, ConfigError,
    config::{VisitorConfig, VisitorMode},
    geodata::GeoResolver,
};
use felucca_proto::{
    StreamRequest, datagram, read_frame,
    visitor::{Side, VisitChallenge, VisitRequest, VisitResult},
    write_frame,
};
use felucca_transport::{BoxedConn, TunnelHandle, network::Network};
use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinSet,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

mod socks;

type Bridge = Arc<parking_lot::RwLock<Option<ActiveBridge>>>;

#[derive(Clone, Debug)]
struct ActiveBridge {
    handle: TunnelHandle,
    cancel: CancellationToken,
}

#[derive(Debug)]
pub(crate) struct Visitors {
    network: Arc<Network>,
    local_ip: Option<std::net::IpAddr>,
    listeners: parking_lot::Mutex<Option<Vec<Prepared>>>,
    bridge: Bridge,
    bytes: Arc<Semaphore>,
}

#[derive(Debug)]
struct Prepared {
    config: VisitorConfig,
    acl: Acl,
    socket: Socket,
}

#[derive(Debug)]
enum Socket {
    Tcp(std::net::TcpListener),
    Udp(std::net::UdpSocket),
}

impl Visitors {
    /// Binding here makes an occupied local port a startup error, before the
    /// agent announces successful readiness on its server connection.
    pub fn prepare(
        config: &[VisitorConfig],
        network: Arc<Network>,
        local_ip: Option<std::net::IpAddr>,
        geo: &GeoResolver,
    ) -> Result<Self, ConfigError> {
        let mut listeners = Vec::new();
        for item in config {
            let addr = felucca_core::config::parse_bind(&item.bind)?;
            let socket = (|| -> io::Result<Socket> {
                match item.mode {
                    VisitorMode::Tcp | VisitorMode::Socks5 => {
                        let socket = std::net::TcpListener::bind(addr)?;
                        socket.set_nonblocking(true)?;
                        Ok(Socket::Tcp(socket))
                    }
                    VisitorMode::Udp => {
                        let socket = std::net::UdpSocket::bind(addr)?;
                        socket.set_nonblocking(true)?;
                        let options = socket2::SockRef::from(&socket);
                        options
                            .set_recv_buffer_size(felucca_transport::udp::INGRESS_RECEIVE_BYTES)?;
                        options.set_send_buffer_size(256 * 1024)?;
                        Ok(Socket::Udp(socket))
                    }
                }
            })()
            .map_err(|error| {
                ConfigError::invalid(format!("visitor {} cannot bind {addr}: {error}", item.name))
            })?;
            let acl = Acl::parse_with_geo(item.acl_mode, &item.acl_rules, geo)
                .map_err(|error| ConfigError::invalid(error.to_string()))?;
            listeners.push(Prepared {
                config: item.clone(),
                acl,
                socket,
            });
        }
        Ok(Self {
            network,
            local_ip,
            listeners: parking_lot::Mutex::new(Some(listeners)),
            bridge: Arc::new(parking_lot::RwLock::new(None)),
            bytes: Arc::new(Semaphore::new(8 * 1024 * 1024)),
        })
    }

    pub fn connected(&self, handle: TunnelHandle) -> Connected {
        *self.bridge.write() = Some(ActiveBridge {
            handle,
            cancel: CancellationToken::new(),
        });
        Connected(self.bridge.clone())
    }

    pub fn start(
        &self,
        tasks: &mut JoinSet<()>,
        key_id: &str,
        timeout: Duration,
        global_slots: Arc<Semaphore>,
        cancel: CancellationToken,
    ) {
        for item in self.listeners.lock().take().unwrap_or_default() {
            let runtime = Arc::new(Runtime {
                network: self.network.clone(),
                local_ip: self.local_ip,
                slots: Arc::new(Semaphore::new(item.config.max_connections)),
                config: item.config,
                acl: item.acl,
                key_id: key_id.to_owned(),
                timeout,
                global_slots: global_slots.clone(),
                bridge: self.bridge.clone(),
                bytes: self.bytes.clone(),
            });
            let cancel = cancel.clone();
            tasks.spawn(async move {
                let name = runtime.config.name.clone();
                let result = tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    result = async {
                        match item.socket {
                            Socket::Tcp(socket) => runtime.tcp(TcpListener::from_std(socket)?).await,
                            Socket::Udp(socket) => runtime.udp(Arc::new(UdpSocket::from_std(socket)?)).await,
                        }
                    } => result,
                };
                if let Err(error) = result {
                    tracing::error!(visitor = name, %error, "visitor listener stopped");
                }
            });
        }
    }
}

/// Handles must not keep a failed bridge alive after serve_once is cancelled.
pub(crate) struct Connected(Bridge);
impl Drop for Connected {
    fn drop(&mut self) {
        if let Some(bridge) = self.0.write().take() {
            bridge.cancel.cancel();
            bridge.handle.close(felucca_mux::CloseReason::Shutdown);
        }
    }
}

struct Runtime {
    network: Arc<Network>,
    local_ip: Option<std::net::IpAddr>,
    config: VisitorConfig,
    acl: Acl,
    key_id: String,
    timeout: Duration,
    slots: Arc<Semaphore>,
    global_slots: Arc<Semaphore>,
    bridge: Bridge,
    bytes: Arc<Semaphore>,
}

impl Runtime {
    fn admit(&self, peer: SocketAddr) -> Option<(OwnedSemaphorePermit, OwnedSemaphorePermit)> {
        if !self.acl.permits(peer.ip())
            || self
                .bridge
                .read()
                .as_ref()
                .is_none_or(|b| b.handle.is_closed())
        {
            return None;
        }
        Some((
            self.slots.clone().try_acquire_owned().ok()?,
            self.global_slots.clone().try_acquire_owned().ok()?,
        ))
    }

    async fn open(&self) -> io::Result<BoxedConn> {
        self.open_target(
            self.config.target.clone(),
            self.config.mode == VisitorMode::Udp,
        )
        .await
    }
    async fn open_target(&self, target: Option<String>, datagram: bool) -> io::Result<BoxedConn> {
        let bridge = self
            .bridge
            .read()
            .clone()
            .ok_or(io::ErrorKind::NotConnected)?;
        let timeout =
            if self.config.connection == felucca_core::config::peer::VisitorConnection::Relay {
                self.timeout
            } else {
                // Two bounded setup phases plus provider cleanup and a fresh relay dial.
                self.timeout.saturating_add(Duration::from_secs(65))
            };
        tokio::time::timeout(timeout, async {
            let mut stream = bridge.handle.open().await.map_err(io::Error::other)?;
            let request = VisitRequest {
                connection: self.config.connection,
                share: self.config.share.clone(),
                nonce: felucca_proto::handshake::generate_nonce(),
                datagram,
                target,
            };
            write_frame(&mut stream, &StreamRequest::Visit(request.clone()))
                .await
                .map_err(io::Error::other)?;
            let challenge: VisitChallenge =
                read_frame(&mut stream).await.map_err(io::Error::other)?;
            let proof = felucca_proto::visitor::prove(
                self.config.secret.as_bytes(),
                &request,
                &challenge,
                &self.key_id,
                Side::Visitor,
            );
            write_frame(&mut stream, &proof)
                .await
                .map_err(io::Error::other)?;
            let mut response: VisitResult =
                read_frame(&mut stream).await.map_err(io::Error::other)?;
            if let VisitResult::Direct { offer, proof } = response {
                if request.connection == felucca_core::config::peer::VisitorConnection::Relay
                    || !felucca_proto::visitor::verify_offer(
                        self.config.secret.as_bytes(),
                        &request,
                        &challenge,
                        &self.key_id,
                        &offer,
                        &proof,
                    )
                {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                match crate::peer::connect(&mut stream, &offer, &self.network, self.local_ip).await
                {
                    Ok(direct) => {
                        tracing::debug!(visitor = self.config.name, "visitor connected directly");
                        return Ok(direct.pipe(stream));
                    }
                    Err(error) => {
                        tracing::debug!(visitor = self.config.name, %error, "direct setup failed");
                        response = read_frame(&mut stream).await.map_err(io::Error::other)?;
                    }
                }
            }
            match response {
                VisitResult::Ok { proof }
                    if request.connection
                        != felucca_core::config::peer::VisitorConnection::Direct
                        && felucca_proto::visitor::verify(
                            self.config.secret.as_bytes(),
                            &request,
                            &challenge,
                            &self.key_id,
                            Side::Server,
                            &proof,
                        ) =>
                {
                    Ok(Box::new(stream) as BoxedConn)
                }
                _ => Err(io::ErrorKind::PermissionDenied.into()),
            }
        })
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
    }

    async fn tcp(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        let mut listener = felucca_transport::tcp::Acceptor::new(listener);
        let mut sessions = JoinSet::new();
        loop {
            tokio::select! {
                (mut local, peer) = listener.accept() => {
                    let Some(slots) = self.admit(peer) else { continue };
                    let Some(cancel) = self.bridge.read().as_ref().map(|b| b.cancel.clone()) else { continue };
                    let runtime = self.clone();
                    sessions.spawn(async move {
                        let _slots = slots;
                        let work = async {
                            local.set_nodelay(true)?;
                            if runtime.config.mode == VisitorMode::Socks5 {
                                return socks::serve(runtime.clone(), local, peer).await;
                            }
                            let mut remote = runtime.open().await?;
                            tokio::io::copy_bidirectional(&mut local, &mut remote).await?;
                            Ok::<_, io::Error>(())
                        };
                        let work = tokio::select! {
                            result = work => result,
                            () = cancel.cancelled() => Ok(()),
                        };
                        if let Err(error) = work {
                            tracing::debug!(visitor = runtime.config.name, %peer, %error, "visitor connection closed");
                        }
                    });
                }
                _ = sessions.join_next(), if !sessions.is_empty() => {}
            }
        }
    }

    async fn udp(self: Arc<Self>, socket: Arc<UdpSocket>) -> io::Result<()> {
        let mut peers: HashMap<SocketAddr, Peer> = HashMap::new();
        let mut sessions = JoinSet::new();
        let mut buffer = vec![0; 65536];
        let mut receiver = felucca_transport::udp::Receiver::new();
        loop {
            tokio::select! {
                (len, peer) = receiver.recv_from(&socket, &mut buffer) => {
                    if len > datagram::MAX_DATAGRAM || !self.acl.permits(peer.ip()) { continue; }
                    // Also clears entries whose task panicked, without retaining
                    // one stale record per source address forever.
                    peers.retain(|_, session| !session.send.is_closed());
                    let Ok(budget) = self.bytes.clone().try_acquire_many_owned(len.max(1) as u32) else { continue; };
                    if let std::collections::hash_map::Entry::Vacant(entry) = peers.entry(peer) {
                        let Some(slots) = self.admit(peer) else { continue };
                        let (send, receive) = mpsc::channel(64);
                        let generation = Arc::new(());
                        entry.insert(Peer { send, generation: generation.clone() });
                        let runtime = self.clone();
                        let socket = socket.clone();
                        sessions.spawn(async move {
                            let _slots = slots;
                            if let Err(error) = runtime.udp_session(socket, peer, receive).await {
                                tracing::debug!(visitor = runtime.config.name, %peer, %error, "UDP visitor session closed");
                            }
                            (peer, generation)
                        });
                    }
                    if let Some(session) = peers.get(&peer) {
                        let _ = session.send.try_send(Packet { data: buffer[..len].to_vec(), _budget: budget });
                    }
                }
                result = sessions.join_next(), if !sessions.is_empty() => {
                    if let Some(Ok((peer, generation))) = result
                        && peers.get(&peer).is_some_and(|s| Arc::ptr_eq(&s.generation, &generation)) {
                        peers.remove(&peer);
                    }
                }
            }
        }
    }

    async fn udp_session(
        &self,
        socket: Arc<UdpSocket>,
        peer: SocketAddr,
        receive: mpsc::Receiver<Packet>,
    ) -> io::Result<()> {
        self.udp_target(
            socket,
            peer,
            receive,
            self.config.target.clone(),
            Vec::new(),
        )
        .await
    }
    async fn udp_target(
        &self,
        socket: Arc<UdpSocket>,
        peer: SocketAddr,
        mut receive: mpsc::Receiver<Packet>,
        target: Option<String>,
        prefix: Vec<u8>,
    ) -> io::Result<()> {
        let remote = self.open_target(target, true).await?;
        let (mut read, mut write) = tokio::io::split(remote);
        let activity = parking_lot::Mutex::new(Instant::now());
        // Keep each framing future alive while the other direction progresses.
        let outbound = async {
            while let Some(packet) = receive.recv().await {
                datagram::write(&mut write, &packet.data).await?;
                *activity.lock() = Instant::now();
            }
            Ok::<_, io::Error>(())
        };
        let inbound = async {
            loop {
                let packet = datagram::read(&mut read).await?;
                if prefix.len() + packet.len() > datagram::MAX_DATAGRAM {
                    continue;
                }
                let mut response = prefix.clone();
                response.extend_from_slice(&packet);
                socket.send_to(&response, peer).await?;
                *activity.lock() = Instant::now();
            }
        };
        let expiry = async {
            loop {
                let deadline = *activity.lock() + self.config.udp_idle;
                tokio::time::sleep_until(deadline).await;
                if activity.lock().elapsed() >= self.config.udp_idle {
                    return Ok(());
                }
            }
        };
        tokio::select! {
            result = outbound => result,
            result = inbound => result,
            result = expiry => result,
        }
    }
}

struct Packet {
    data: Vec<u8>,
    _budget: OwnedSemaphorePermit,
}
struct Peer {
    send: mpsc::Sender<Packet>,
    generation: Arc<()>,
}
