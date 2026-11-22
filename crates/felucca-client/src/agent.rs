//! The agent: holds a tunnel to a server and serves what arrives on it.

use std::sync::Arc;
use std::time::Duration;

use felucca_core::TransportKind;
use felucca_core::config::AgentConfig;
use felucca_proto::{
    ConnectionRole, Control, StreamReject, StreamRequest, StreamResponse, read_frame, write_frame,
};
use felucca_transport::{TransportError, Tunnel, network::Network, tls, ws};
use tokio::io::{AsyncRead, AsyncWrite};

use felucca_service::dial::verification;
use tokio_util::sync::CancellationToken;

/// A running agent.
#[derive(Debug)]
pub struct Agent {
    clock: Arc<felucca_transport::clock::Clock>,
    network: Arc<Network>,
    config: AgentConfig,
    initial_services: Arc<felucca_service::Services>,
    active: Arc<parking_lot::RwLock<RuntimeServices>>,
    authority: Option<felucca_service::remote::RemoteAuthority>,
    visitors: crate::visitor::Visitors,
    probes: parking_lot::Mutex<tokio::task::JoinSet<()>>,
    cancel: CancellationToken,
    slots: Arc<tokio::sync::Semaphore>,
    peer_slots: Arc<tokio::sync::Semaphore>,
    ready: std::sync::atomic::AtomicBool,
}

#[derive(Debug)]
struct RuntimeServices {
    services: Arc<felucca_service::Services>,
    tunnels: Vec<felucca_core::config::TunnelConfig>,
    revision: Option<[u8; 32]>,
    generations: std::collections::HashMap<u32, CancellationToken>,
}

/// How long a session must serve before a reconnect starts from the shortest delay.
const STABLE_SESSION: Duration = Duration::from_secs(30);

/// Spreads a reconnect delay over its upper half.
///
/// Every agent of a restarted server fails at the same instant. Without jitter
/// they retry in lockstep, and each wave of TLS handshakes lands at once.
fn jittered(delay: Duration) -> Duration {
    delay / 2 + delay.mul_f64(rand::random_range(0.0..0.5))
}

impl Agent {
    /// Builds an agent from a validated config.
    ///
    /// Tunnel ids come from config order. They have to be stable across restarts,
    /// or a reconnecting agent would renumber its tunnels and the server would
    /// route inbound traffic to the wrong one.
    pub fn new(mut config: AgentConfig) -> Result<Self, felucca_core::ConfigError> {
        config.validate()?;
        let network = Arc::new(Network::new(&config.network, config.dial_timeout).map_err(
            |error| felucca_core::ConfigError::invalid(format!("network configuration: {error}")),
        )?);
        let services = Arc::new(felucca_service::Services::new(
            &config.tunnels,
            &config.geo,
            network.clone(),
        )?);
        let authority = if config.remote.enabled {
            Some(felucca_service::remote::RemoteAuthority::new(
                &config.remote,
                &felucca_core::geodata::GeoResolver::new(&config.geo)?,
            )?)
        } else {
            None
        };
        let visitors = crate::visitor::Visitors::prepare(
            &config.visitors,
            network.clone(),
            config.p2p.bind_ip,
            &felucca_core::geodata::GeoResolver::new(&config.geo)?,
        )?;
        Ok(Self {
            clock: Arc::new(felucca_transport::clock::Clock::new(config.ntp.clone())),
            network,
            initial_services: services.clone(),
            active: Arc::new(parking_lot::RwLock::new(RuntimeServices {
                services,
                tunnels: config.tunnels.clone(),
                revision: None,
                generations: config
                    .tunnels
                    .iter()
                    .filter(|t| t.enabled)
                    .map(|t| (t.id.expect("validated id"), CancellationToken::new()))
                    .collect(),
            })),
            authority,
            visitors,
            probes: Default::default(),
            slots: Arc::new(tokio::sync::Semaphore::new(
                config
                    .max_connections
                    .unwrap_or(tokio::sync::Semaphore::MAX_PERMITS)
                    .min(tokio::sync::Semaphore::MAX_PERMITS),
            )),
            peer_slots: Arc::new(tokio::sync::Semaphore::new(config.p2p.max_sessions)),
            config,
            cancel: CancellationToken::new(),
            ready: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Connects, serves, and reconnects for as long as the process runs.
    ///
    /// Never returns on its own: a tunnel dropping is expected, not terminal.
    pub fn shutdown(&self) {
        self.cancel.cancel();
        self.probes.lock().abort_all();
        self.ready
            .store(false, std::sync::atomic::Ordering::Release);
    }
    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Acquire)
    }

    pub async fn run(self: Arc<Self>) {
        struct Probes<'a>(&'a Agent);
        impl Drop for Probes<'_> {
            fn drop(&mut self) {
                self.0.probes.lock().abort_all();
                self.0
                    .ready
                    .store(false, std::sync::atomic::Ordering::Release);
            }
        }
        let _probes = Probes(&self);
        let mut probes = tokio::task::JoinSet::new();
        self.visitors.start(
            &mut probes,
            self.config.key_id.as_deref().unwrap_or("felucca-agent"),
            self.config.dial_timeout,
            self.slots.clone(),
            self.cancel.clone(),
        );
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => return,
            result = self.clock.synchronize(&self.network) => {
                if let Err(error) = result {
                    tracing::warn!(%error, "initial NTP sync failed; using system time");
                }
            }
        }
        probes.spawn(
            self.clock
                .clone()
                .refresh(self.network.clone(), self.cancel.clone()),
        );
        self.active
            .read()
            .services
            .spawn_probes(&mut self.probes.lock());
        let mut backoff = self.config.reconnect_min;

        loop {
            let started = tokio::time::Instant::now();
            let result = tokio::select! {
                () = self.cancel.cancelled() => return,
                result = self.serve_once() => result,
            };
            // A session that served for a while proves the server healthy, so
            // retry quickly. One dropped right after registering, as by a server
            // in a crash loop, keeps backing off instead of retrying every second.
            if self.ready.swap(false, std::sync::atomic::Ordering::AcqRel)
                && started.elapsed() >= STABLE_SESSION
            {
                backoff = self.config.reconnect_min;
            }
            match result {
                Ok(()) => {
                    tracing::info!("tunnel closed cleanly; reconnecting");
                    // A clean close usually means a server restart, which will be
                    // back shortly, so start over from the shortest delay.
                    backoff = self.config.reconnect_min;
                }
                Err(e) => {
                    tracing::warn!(error = %e, backoff = ?backoff, "tunnel failed");
                }
            }

            tokio::select! {
                () = self.cancel.cancelled() => return,
                () = tokio::time::sleep(jittered(backoff)) => {},
            }
            // Exponential up to the ceiling: a server that is down for an hour
            // should not be probed every second for an hour.
            backoff = backoff.saturating_mul(2).min(self.config.reconnect_max);
        }
    }

    /// One connection's lifetime: handshake, register, serve until it ends.
    async fn serve_once(&self) -> Result<(), TransportError> {
        let mut tunnel = tokio::time::timeout(self.config.dial_timeout, self.connect())
            .await
            .map_err(|_| TransportError::Timeout)??;
        // Always negotiate, including an empty bootstrap: the server may own
        // desired state for this identity. Bound retries against a broken peer.
        let mut control = tunnel.open().await?;
        write_frame(&mut control, &StreamRequest::Control)
            .await
            .map_err(codec_error)?;
        let all_registered = self.negotiate(&mut control).await?;
        let live = enable_live_configuration(&mut control).await?;
        let refresh = async {
            if live {
                self.refresh_configuration(control).await
            } else {
                drop(control);
                std::future::pending().await
            }
        };
        tokio::pin!(refresh);

        let _connected = self.visitors.connected(tunnel.handle());
        self.ready
            .store(all_registered, std::sync::atomic::Ordering::Release);
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                result = &mut refresh => return result,
                stream = tunnel.accept() => {
                    let mut stream = stream?;
                    let active = self.active.clone();
                    let dial_timeout = self.config.dial_timeout;
                    let vkey = self.config.vkey.clone();
                    let slots = self.slots.clone();
                    let peer_slots = self.peer_slots.clone();
                    let peer_config = self.config.p2p.clone();
                    let network = self.network.clone();
                    tasks.spawn(async move {
                        let Ok(Ok(request)) = tokio::time::timeout(dial_timeout, read_frame::<_, StreamRequest>(&mut stream)).await else { return; };
                        let id = match &request {
                            StreamRequest::Tunnel { tunnel_id } | StreamRequest::Routed { tunnel_id, .. } | StreamRequest::Diagnostics { tunnel_id } => Some(*tunnel_id),
                            StreamRequest::Peer(offer) => Some(offer.route.tunnel_id),
                            _ => None,
                        };
                        let (services, cancel) = {
                            let active = active.read();
                            (active.services.clone(), id.and_then(|id| active.generations.get(&id)).cloned())
                        };
                        let serve = async {
                        if let StreamRequest::Peer(offer) = request {
                            let slot = slots.try_acquire_owned();
                            let peer_slot = peer_slots.try_acquire_owned();
                            if slot.is_ok() && peer_slot.is_ok() {
                                if let Err(error) = crate::peer::provide(&mut stream, *offer, services, &peer_config, &network, dial_timeout).await {
                                    tracing::debug!(%error, "direct provider session closed");
                                    let _ = write_frame(&mut stream, &felucca_proto::peer::ProviderMessage::Prepared { fingerprint: None }).await;
                                }
                            } else {
                                let _ = write_frame(&mut stream, &felucca_proto::peer::ProviderMessage::Prepared { fingerprint: None }).await;
                            }
                            drop(slot);
                            drop(peer_slot);
                            let _ = write_frame(&mut stream, &felucca_proto::peer::ProviderMessage::Finished).await;
                        } else if let Ok(_slot) = slots.try_acquire_owned() {
                            services.serve_request(stream, request, dial_timeout, &vkey).await;
                        } else {
                            let _ = write_frame(&mut stream, &StreamResponse::Err(StreamReject::TooManyConnections)).await;
                        }
                        };
                        tokio::select! {
                            biased;
                            () = async { match cancel { Some(cancel) => cancel.cancelled().await, None => std::future::pending().await } } => {},
                            () = serve => {},
                        }
                    });
                },
                _ = tasks.join_next(), if !tasks.is_empty() => {},
            }
        }
    }

    async fn refresh_configuration<T: AsyncRead + AsyncWrite + Unpin>(
        &self,
        mut control: T,
    ) -> Result<(), TransportError> {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            write_frame(&mut control, &Control::Ping { nonce: 0 })
                .await
                .map_err(codec_error)?;
            // This long-lived poll shares the transport with bulk traffic.
            // Its reply can be delayed while other streams still progress;
            // mux/QUIC liveness owns dead-connection detection and wakes this
            // reader on failure. A separate RPC deadline would tear down every
            // forwarded stream on an otherwise healthy, congested connection.
            match read_frame::<_, Control>(&mut control)
                .await
                .map_err(codec_error)?
            {
                Control::Pong { nonce: 0 } => {}
                Control::ConfigChanged => {
                    let ready = self.negotiate(&mut control).await?;
                    self.ready
                        .store(ready, std::sync::atomic::Ordering::Release);
                }
                _ => {
                    return Err(config_error(felucca_core::ConfigError::invalid(
                        "unexpected config response",
                    )));
                }
            }
        }
    }

    fn prepare_managed(
        &self,
        revision: [u8; 32],
        config: felucca_core::Secret,
    ) -> Result<RuntimeServices, felucca_core::ConfigError> {
        let authority = self.authority.as_ref().ok_or_else(|| {
            felucca_core::ConfigError::invalid("remote configuration is disabled")
        })?;
        let geo = felucca_core::geodata::GeoResolver::new(&self.config.geo)?;
        let desired = felucca_core::config::ManagedConfig::decode(config.expose(), revision, &geo)?;
        let authority = authority.scoped(desired.target_policy.as_ref(), &geo)?;
        let services = Arc::new(felucca_service::Services::with_authority(
            &desired.tunnels,
            &self.config.geo,
            self.network.clone(),
            Some(&authority),
        )?);
        Ok(RuntimeServices {
            services,
            tunnels: desired.tunnels,
            revision: Some(revision),
            generations: Default::default(),
        })
    }

    async fn negotiate<T: AsyncRead + AsyncWrite + Unpin>(
        &self,
        control: &mut T,
    ) -> Result<bool, TransportError> {
        let mut all_registered = false;
        for _ in 0..3 {
            let (specs, revision) = {
                let active = self.active.read();
                (felucca_service::specs(&active.tunnels), active.revision)
            };
            let expected = specs.len();
            write_frame(
                control,
                &Control::PushConfig {
                    tunnels: specs,
                    remote_config: self.config.remote.enabled,
                    revision,
                },
            )
            .await
            .map_err(codec_error)?;
            match tokio::time::timeout(Duration::from_secs(15), read_frame::<_, Control>(control))
                .await
                .map_err(|_| TransportError::Timeout)?
                .map_err(codec_error)?
            {
                Control::Configure { revision, config } => {
                    match self.prepare_managed(revision, config) {
                        Ok(next) => self.replace_services(next),
                        Err(error) => {
                            tracing::warn!(%error, "invalid managed configuration");
                            write_frame(
                                control,
                                &Control::ConfigResult {
                                    accepted: Vec::new(),
                                    rejected: vec![felucca_proto::ConfigRejection {
                                        tunnel_id: 0,
                                        reason: "invalid managed configuration".into(),
                                    }],
                                },
                            )
                            .await
                            .map_err(codec_error)?;
                            return Ok(false);
                        }
                    }
                }
                Control::RestoreConfig => {
                    if self.authority.is_none() {
                        return Err(config_error(felucca_core::ConfigError::invalid(
                            "remote configuration is disabled",
                        )));
                    }
                    self.replace_services(RuntimeServices {
                        services: self.initial_services.clone(),
                        tunnels: self.config.tunnels.clone(),
                        revision: None,
                        generations: Default::default(),
                    });
                }
                Control::ConfigResult { accepted, rejected } => {
                    all_registered = rejected.is_empty() && accepted.len() == expected;
                    for r in &rejected {
                        tracing::error!(tunnel = r.tunnel_id, reason = %r.reason, "the server refused a tunnel");
                    }
                    if !self.is_ready() {
                        tracing::info!(accepted = accepted.len(), "tunnels registered");
                    }
                    break;
                }
                _ => {
                    return Err(config_error(felucca_core::ConfigError::invalid(
                        "unexpected config response",
                    )));
                }
            }
        }
        Ok(all_registered)
    }

    fn replace_services(&self, mut next: RuntimeServices) {
        let mut active = self.active.write();
        for tunnel in next.tunnels.iter().filter(|t| t.enabled) {
            let id = tunnel.id.expect("validated id");
            let unchanged = active
                .tunnels
                .iter()
                .find(|t| t.id == Some(id))
                .is_some_and(|old| {
                    let mut old = old.clone();
                    old.remark.clone_from(&tunnel.remark);
                    serde_json::to_value(old).expect("tunnel")
                        == serde_json::to_value(tunnel).expect("tunnel")
                });
            let cancel = if unchanged {
                active.generations.remove(&id).unwrap_or_default()
            } else {
                CancellationToken::new()
            };
            next.generations.insert(id, cancel);
        }
        for cancel in active.generations.values() {
            cancel.cancel();
        }

        let mut probes = self.probes.lock();
        probes.abort_all();
        // Dropping the set drains completed/aborted entries as configurations
        // change; workers keep only their own immutable Services snapshot.
        *probes = tokio::task::JoinSet::new();
        if !self.cancel.is_cancelled() {
            next.services.spawn_probes(&mut probes);
        }
        *active = next;
    }

    /// Dials the server and completes the handshake.
    async fn connect(&self) -> Result<Tunnel, TransportError> {
        let key_id = self.config.key_id.as_deref().unwrap_or("felucca-agent");
        let policy = verification(&self.config.trust, &self.config.server);

        match self.config.transport {
            TransportKind::Kcp => {
                let addresses = self.network.resolve(&self.config.server).await?;
                let budget = self.config.dial_timeout / addresses.len().max(1) as u32;
                let mut last = TransportError::Io(std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    "no KCP destination",
                ));
                for address in addresses {
                    let attempt = async {
                        let mut io = felucca_transport::kcp::connect(
                            &self.config.kcp,
                            address,
                            self.network.local_ip(),
                        )
                        .await?;
                        let keepalive = self.shake(&mut io, key_id).await?;
                        Ok::<_, TransportError>(self.mux(io, keepalive))
                    };
                    match tokio::time::timeout(budget, attempt).await {
                        Ok(Ok(tunnel)) => return Ok(tunnel),
                        Ok(Err(error)) => last = error,
                        Err(_) => last = TransportError::Timeout,
                    }
                }
                Err(last)
            }
            TransportKind::Quic => {
                let host = self
                    .config
                    .server
                    .rsplit_once(':')
                    .expect("validated endpoint")
                    .0
                    .trim_matches(['[', ']']);
                let mut result = Err(TransportError::Io(std::io::Error::other(
                    "no QUIC destination",
                )));
                let addresses = self.network.resolve(&self.config.server).await?;
                let count = addresses.len();
                for (index, address) in addresses.into_iter().enumerate() {
                    let attempt = felucca_transport::quic::connect_to(
                        address,
                        host,
                        &policy,
                        self.network.local_ip(),
                    );
                    // As for TCP: a silent address must not use up the whole
                    // dial deadline before the next one is tried.
                    result = if index + 1 == count {
                        attempt.await
                    } else {
                        tokio::time::timeout(felucca_transport::network::ADDRESS_ATTEMPT, attempt)
                            .await
                            .unwrap_or(Err(TransportError::Timeout))
                    };
                    if result.is_ok() {
                        break;
                    }
                }
                let conn = Tunnel::over_quic(result?);
                // The handshake runs on a stream of its own, before anything else.
                let mut stream = conn.open().await?;
                self.shake(&mut stream, key_id).await?;
                drop(stream);
                Ok(conn)
            }
            TransportKind::Tcp => {
                let mut io = self.network.bridge_tcp(&self.config.server).await?;
                let keepalive = self.shake(&mut io, key_id).await?;
                Ok(self.mux(io, keepalive))
            }
            TransportKind::Tls => {
                let mut io = tls::handshake(
                    self.network.bridge_tcp(&self.config.server).await?,
                    self.config
                        .server
                        .rsplit_once(':')
                        .expect("validated endpoint")
                        .0
                        .trim_matches(['[', ']']),
                    &policy,
                    &[felucca_transport::BRIDGE_ALPN],
                )
                .await?;
                let keepalive = self.shake(&mut io, key_id).await?;
                Ok(self.mux(io, keepalive))
            }
            TransportKind::Ws | TransportKind::Wss => {
                let url = self.ws_url();
                let mut io = if self.config.transport == TransportKind::Wss {
                    let tcp = tls::handshake(
                        self.network.bridge_tcp(&self.config.server).await?,
                        self.config
                            .server
                            .rsplit_once(':')
                            .expect("validated endpoint")
                            .0
                            .trim_matches(['[', ']']),
                        &policy,
                        &[felucca_transport::BRIDGE_ALPN, b"http/1.1"],
                    )
                    .await?;
                    Either::Right(ws::WsConn::connect(&url, tcp).await?)
                } else {
                    let tcp = self.network.bridge_tcp(&self.config.server).await?;
                    Either::Left(ws::WsConn::connect(&url, tcp).await?)
                };
                let keepalive = self.shake(&mut io, key_id).await?;
                Ok(self.mux(io, keepalive))
            }
        }
    }

    fn ws_url(&self) -> String {
        let scheme = if self.config.transport == TransportKind::Wss {
            "wss"
        } else {
            "ws"
        };
        format!("{scheme}://{}{}", self.config.server, self.config.ws_path)
    }

    fn mux<T>(&self, io: T, keepalive: u32) -> Tunnel
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        Tunnel::over_stream(
            io,
            felucca_mux::Role::Client,
            felucca_mux::Config {
                keepalive_interval: Duration::from_secs(u64::from(keepalive)),
                keepalive_timeout: Duration::from_secs(u64::from(keepalive) * 2),
                ..felucca_transport::tunnel::mux_config(&self.config.mux)
            },
        )
    }

    async fn shake<T>(&self, io: &mut T, key_id: &str) -> Result<u32, TransportError>
    where
        T: AsyncRead + AsyncWrite + Unpin,
    {
        felucca_proto::session::connect_info_with_clock(
            io,
            key_id,
            &self.config.vkey,
            ConnectionRole::Tunnel,
            || self.clock.now_unix(),
        )
        .await
        .map(|info| info.keepalive_secs)
        .map_err(|e| TransportError::tls(format!("handshake failed: {e}")))
    }
}

fn codec_error(e: felucca_proto::CodecError) -> TransportError {
    TransportError::Io(std::io::Error::other(e.to_string()))
}

async fn enable_live_configuration<T: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut T,
) -> Result<bool, TransportError> {
    let nonce = felucca_proto::message::LIVE_CONFIG_QUERY;
    write_frame(stream, &Control::Ping { nonce })
        .await
        .map_err(codec_error)?;
    let reply = tokio::time::timeout(Duration::from_secs(15), read_frame::<_, Control>(stream))
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(codec_error)?;
    match reply {
        Control::Pong { nonce: echoed } if echoed == nonce => Ok(false),
        Control::ConfigChanged => {
            write_frame(stream, &Control::WatchConfig)
                .await
                .map_err(codec_error)?;
            Ok(true)
        }
        _ => Err(config_error(felucca_core::ConfigError::invalid(
            "unexpected config response",
        ))),
    }
}

/// One of two connection types, so the WebSocket paths share a code path.
enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L, R> AsyncRead for Either<L, R>
where
    L: AsyncRead + Unpin,
    R: AsyncRead + Unpin,
{
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Left(l) => std::pin::Pin::new(l).poll_read(cx, buf),
            Self::Right(r) => std::pin::Pin::new(r).poll_read(cx, buf),
        }
    }
}

impl<L, R> AsyncWrite for Either<L, R>
where
    L: AsyncWrite + Unpin,
    R: AsyncWrite + Unpin,
{
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Left(l) => std::pin::Pin::new(l).poll_write(cx, buf),
            Self::Right(r) => std::pin::Pin::new(r).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Left(l) => std::pin::Pin::new(l).poll_flush(cx),
            Self::Right(r) => std::pin::Pin::new(r).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Left(l) => std::pin::Pin::new(l).poll_shutdown(cx),
            Self::Right(r) => std::pin::Pin::new(r).poll_shutdown(cx),
        }
    }
}

fn config_error(error: felucca_core::ConfigError) -> TransportError {
    TransportError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reconnect_delay_is_spread_over_its_upper_half() {
        let delay = Duration::from_secs(8);
        let samples: Vec<_> = (0..1000).map(|_| jittered(delay)).collect();
        assert!(samples.iter().all(|d| (delay / 2..delay).contains(d)));
        let distinct: std::collections::HashSet<_> = samples.iter().collect();
        assert!(distinct.len() > 900, "agents must not retry in lockstep");
    }

    #[tokio::test(start_paused = true)]
    async fn a_delayed_configuration_poll_keeps_waiting_but_transport_failure_exits() {
        let config = serde_json::from_value(serde_json::json!({
            "server": "127.0.0.1:14028", "vkey": "poll-test-key"
        }))
        .unwrap();
        let agent = Agent::new(config).unwrap();
        let (control, mut peer) = tokio::io::duplex(1024);
        let poll = tokio::spawn(async move { agent.refresh_configuration(control).await });
        assert_eq!(
            read_frame::<_, Control>(&mut peer).await.unwrap(),
            Control::Ping { nonce: 0 }
        );
        tokio::time::advance(Duration::from_secs(20)).await;
        tokio::task::yield_now().await;
        assert!(!poll.is_finished(), "a delayed poll killed the session");
        write_frame(&mut peer, &Control::Pong { nonce: 0 })
            .await
            .unwrap();
        assert_eq!(
            read_frame::<_, Control>(&mut peer).await.unwrap(),
            Control::Ping { nonce: 0 }
        );
        drop(peer);
        assert!(
            poll.await.unwrap().is_err(),
            "transport failure must end the poll"
        );
    }

    #[tokio::test]
    async fn legacy_servers_receive_only_legacy_messages() {
        use tokio::io::AsyncReadExt;
        let (mut agent, mut server) = tokio::io::duplex(1024);
        let peer = tokio::spawn(async move {
            // The old v10 server understands Ping/Pong but fails decoding WatchConfig.
            let Control::Ping { nonce } = read_frame(&mut server).await.unwrap() else {
                panic!("agent sent an unsupported extension before negotiation");
            };
            write_frame(&mut server, &Control::Pong { nonce })
                .await
                .unwrap();
            let mut extra = Vec::new();
            server.read_to_end(&mut extra).await.unwrap();
            assert!(extra.is_empty(), "an extension was sent to a legacy server");
        });
        assert!(!enable_live_configuration(&mut agent).await.unwrap());
        drop(agent);
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn supporting_server_is_acknowledged_before_live_updates_start() {
        let (mut agent, mut server) = tokio::io::duplex(1024);
        let peer = tokio::spawn(async move {
            assert_eq!(
                read_frame::<_, Control>(&mut server).await.unwrap(),
                Control::Ping {
                    nonce: felucca_proto::message::LIVE_CONFIG_QUERY
                }
            );
            write_frame(&mut server, &Control::ConfigChanged)
                .await
                .unwrap();
            assert_eq!(
                read_frame::<_, Control>(&mut server).await.unwrap(),
                Control::WatchConfig
            );
        });
        assert!(enable_live_configuration(&mut agent).await.unwrap());
        peer.await.unwrap();
    }
}
