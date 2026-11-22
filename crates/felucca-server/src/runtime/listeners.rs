//! Bridge listeners, transport dispatch and certificate reload.
use super::*;
impl Server {
    pub async fn run(self: Arc<Self>) -> Result<u16, ServerError> {
        let network = Arc::new(felucca_transport::network::Network::default());
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "server stopped").into());
            },
            result = self.clock.synchronize(&network) => {
                if let Err(error) = result {
                    tracing::warn!(%error, "initial NTP sync failed; using system time");
                }
            }
        }
        let listener =
            tcp::bind(felucca_core::config::parse_bind(&self.config.bridge.bind)?).await?;
        let port = listener.local_addr()?.port();
        let kcp = match &self.config.bridge.kcp_bind {
            Some(bind) => Some(
                felucca_transport::kcp::KcpListener::bind(
                    &self.config.bridge.kcp,
                    felucca_core::config::parse_bind(bind)?,
                )
                .await?,
            ),
            None => None,
        };
        let peer_broker = match &self.config.p2p {
            Some(config) => Some(
                felucca_transport::rendezvous::Broker::bind(
                    felucca_core::config::parse_bind(&config.bind)?,
                    config.max_pending,
                )
                .await?,
            ),
            None => None,
        };
        let quic_address = self
            .config
            .bridge
            .quic_bind
            .as_deref()
            .map(felucca_core::config::parse_bind)
            .transpose()?;
        let quic = self
            .config
            .bridge
            .quic_bind
            .as_ref()
            .map(|bind| {
                Ok::<_, ServerError>(quinn::Endpoint::server(
                    self.quic_config(&self.identity.load())?,
                    felucca_core::config::parse_bind(bind)?,
                )?)
            })
            .transpose()?;
        if let Some(reverse) = &self.reverse {
            if self.shared_reverse(true).is_some() {
                *reverse.bridge.write() = Arc::downgrade(&self);
            }
            reverse
                .start_except(
                    self.cancel.clone(),
                    Some(felucca_core::config::parse_bind(&self.config.bridge.bind)?),
                    quic_address,
                )
                .await?;
            if let Some(config) = &reverse.config.acme {
                self.tasks.spawn(crate::acme::run(
                    reverse.clone(),
                    config.clone(),
                    self.cancel.clone(),
                ));
            }
        }
        if let Err(error) = self.start_locals().await {
            let _ = self.shutdown_and_wait().await;
            return Err(error);
        }
        if let Some(broker) = peer_broker {
            *self.peer_broker.write() = Some(broker.clone());
            let cancel = self.cancel.clone();
            self.tasks.spawn(async move {
                if let Err(error) = broker.run(cancel).await {
                    tracing::error!(%error, "P2P rendezvous stopped");
                }
            });
        }
        self.tasks
            .spawn(self.clock.clone().refresh(network, self.cancel.clone()));
        if let Some(state) = &self.config.state {
            let interval = state.save_interval;
            let store = self.registry.store.clone();
            let cancel = self.cancel.clone();
            self.tasks.spawn(async move {
                let mut tick = tokio::time::interval(interval);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => break,
                        _ = tick.tick() => {
                            let store = store.clone();
                            match tokio::task::spawn_blocking(move || store.save()).await {
                                Ok(Ok(())) => {},
                                Ok(Err(error)) => tracing::error!(%error, "traffic snapshot failed; retaining previous snapshot"),
                                Err(error) => tracing::error!(%error, "traffic snapshot task failed"),
                            }
                        }
                    }
                }
            });
        }
        if self.config.bridge.tls.is_some() {
            let this = self.clone();
            let quic = quic.clone();
            self.tasks.spawn(async move {
                let mut interval = tokio::time::interval(this.config.bridge.cert_reload_interval);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval.tick().await;
                loop {
                    tokio::select! {
                        biased;
                        () = this.cancel.cancelled() => break,
                        _ = interval.tick() => {
                            if let Err(error) = this.reload_bridge(quic.as_ref()).await {
                                tracing::warn!(%error, "bridge certificate reload failed; retaining current certificate");
                            }
                        }
                    }
                }
            });
        }
        if let Some(mut listener) = kcp {
            let this = self.clone();
            self.tasks.spawn(async move {
                let mut tasks = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        biased;
                        () = this.cancel.cancelled() => break,
                        _ = tasks.join_next(), if !tasks.is_empty() => {},
                        accepted = listener.accept() => {
                            let Ok((mut io, peer)) = accepted else { break; };
                            let this = this.clone();
                            tasks.spawn(async move {
                                let Ok(key) = this.auth(&mut io).await else { return; };
                                let tunnel = this.mux(io);
                                let _ = this.session(tunnel, key, peer, false).await;
                            });
                        }
                    }
                }
            });
        }
        if let Some(endpoint) = quic {
            let local = endpoint.local_addr()?;
            let this = self.clone();
            self.tasks.spawn(async move {
                let mut tasks = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        biased;
                        () = this.cancel.cancelled() => { endpoint.close(0u32.into(), b"shutdown"); break; },
                        _ = tasks.join_next(), if !tasks.is_empty() => {},
                        incoming = endpoint.accept() => {
                            let Some(incoming) = incoming else { break; }; let this = this.clone();
                            tasks.spawn(async move {
                                let Ok(Ok(connection)) = tokio::time::timeout(Duration::from_secs(15), incoming).await else { return; };
                                let is_http = connection.handshake_data()
                                    .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
                                    .is_some_and(|data| data.protocol.as_deref() == Some(b"h3"));
                                if is_http {
                                    if let Some(reverse) = this.shared_quic_reverse() {
                                        crate::http3::serve_connection(connection, reverse, local).await;
                                    }
                                    return;
                                }
                                let peer = connection.remote_address();
                                let mut tunnel = Tunnel::over_quic(connection);
                                let result = tokio::time::timeout(Duration::from_secs(30), async {
                                    let mut stream = tunnel.accept().await?;
                                    this.auth(&mut stream).await
                                }).await;
                                if let Ok(Ok(key)) = result { let _ = this.session(tunnel, key, peer, true).await; }
                            });
                        }
                    }
                }
            });
        }
        let this = self.clone();
        self.tasks.spawn(async move {
            let mut listener = tcp::Acceptor::new(listener);
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    () = this.cancel.cancelled() => break,
                    _ = tasks.join_next(), if !tasks.is_empty() => {},
                    (mut io, _) = listener.accept() => {
                        let this = this.clone();
                        tasks.spawn(async move {
                            let Ok(addresses) = this.proxy_accept.accept(&mut io).await else { return; };
                            let peer = addresses.source;
                            let Ok(Ok(first)) = tokio::time::timeout(Duration::from_secs(15), io.read_u8()).await else { return; };
                            let io = PrefixedConn::new(Bytes::copy_from_slice(&[first]), io);
                            let result = if first == 0x16 {
                                if let Some(reverse) = this.shared_reverse(true) {
                                    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                                    let accepted = tokio_rustls::LazyConfigAcceptor::new(
                                        rustls::server::Acceptor::default(),
                                        crate::tls_ingress::Captured::new(io),
                                    );
                                    let Ok(Ok(mut start)) = tokio::time::timeout_at(deadline, accepted).await else { return; };
                                    let bridge = start.client_hello().alpn().is_some_and(|mut protocols|
                                        protocols.any(|name| name == felucca_transport::BRIDGE_ALPN));
                                    tracing::debug!(bridge, sni = start.client_hello().server_name(), "shared TLS dispatch");
                                    if !bridge {
                                        reverse.serve_tls(start, addresses, deadline).await;
                                        return;
                                    }
                                    start.io.stop();
                                    let Ok(Ok(tls)) = tokio::time::timeout_at(deadline, start.into_stream(this.bridge_tls.load_full())).await else { return; };
                                    let _ = this.stream_transport(tls, addresses, false).await;
                                    return;
                                }
                                let acceptor = tokio_rustls::TlsAcceptor::from(this.bridge_tls.load_full());
                                match tokio::time::timeout(Duration::from_secs(15), acceptor.accept(io)).await {
                                    Ok(Ok(tls)) => this.stream_transport(tls, addresses, false).await,
                                    _ => return,
                                }
                            } else { this.stream_transport(io, addresses, true).await };
                            if let Err(e) = result { tracing::debug!(%peer, error = %e, "agent session ended"); }
                        });
                    }
                }
            }
        });
        Ok(port)
    }
    pub(super) async fn reload_bridge(
        &self,
        endpoint: Option<&quinn::Endpoint>,
    ) -> Result<(), TransportError> {
        let Some(files) = &self.config.bridge.tls else {
            return Ok(());
        };
        let cert = tokio::fs::read(&files.cert).await?;
        let key = tokio::fs::read(&files.key).await?;
        let identity = Identity::from_pem(&cert, &key)?;
        let mut tls = (*identity.server_config()?).clone();
        // Validation happens even if the chain has not changed; a half-written
        // or mismatched key must never displace the working identity.
        if identity.chain == self.identity.load().chain {
            return Ok(());
        }
        tls.alpn_protocols = vec![
            felucca_transport::BRIDGE_ALPN.to_vec(),
            b"http/1.1".to_vec(),
        ];
        let quic = endpoint.map(|_| self.quic_config(&identity)).transpose()?;
        if let (Some(endpoint), Some(config)) = (endpoint, quic) {
            endpoint.set_server_config(Some(config));
        }
        self.bridge_tls.store(Arc::new(tls));
        let fingerprint = identity.fingerprint();
        self.identity.store(Arc::new(identity));
        tracing::info!(%fingerprint, "bridge certificate reloaded");
        Ok(())
    }
    pub(super) fn shared_quic_reverse(&self) -> Option<Arc<ReverseProxy>> {
        let address = self
            .config
            .bridge
            .quic_bind
            .as_deref()
            .and_then(|bind| felucca_core::config::parse_bind(bind).ok())?;
        self.reverse
            .as_ref()
            .filter(|reverse| {
                reverse
                    .config
                    .http3_bind
                    .as_deref()
                    .and_then(|bind| felucca_core::config::parse_bind(bind).ok())
                    == Some(address)
            })
            .cloned()
    }
    pub(super) fn quic_config(
        &self,
        identity: &Identity,
    ) -> Result<quinn::ServerConfig, TransportError> {
        match self.shared_quic_reverse() {
            Some(reverse) => crate::http3::server_config(
                Arc::new(crate::http3::SharedCertificates {
                    bridge: identity.server_config()?.cert_resolver.clone(),
                    http: reverse.certificates.clone(),
                }),
                true,
            ),
            None => felucca_transport::quic::server_config(identity),
        }
    }
    pub(super) async fn stream_transport<T>(
        self: Arc<Self>,
        mut io: T,
        addresses: felucca_transport::proxy_protocol::Addresses,
        plaintext: bool,
    ) -> Result<(), TransportError>
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let peer = addresses.source;
        let first = tokio::time::timeout(Duration::from_secs(15), io.read_u8())
            .await
            .map_err(|_| TransportError::Timeout)??;
        let mut io = PrefixedConn::new(Bytes::copy_from_slice(&[first]), io);
        let (key, tunnel) = if first == b'G' {
            let (bridge, io) =
                crate::ingress::classify_get(io, &self.config.bridge.ws_path).await?;
            if !bridge
                && plaintext
                && let Some(reverse) = self.shared_reverse(false)
            {
                reverse.serve_plain(io, addresses).await;
                return Ok(());
            }
            let mut ws = tokio::time::timeout(
                Duration::from_secs(15),
                felucca_transport::WsConn::accept_at(io, &self.config.bridge.ws_path),
            )
            .await
            .map_err(|_| TransportError::Timeout)??;
            let key = self.auth(&mut ws).await?;
            (key, self.mux(ws))
        } else if first == 0 {
            let key = self.auth(&mut io).await?;
            (key, self.mux(io))
        } else if plaintext && let Some(reverse) = self.shared_reverse(false) {
            reverse.serve_plain(io, addresses).await;
            return Ok(());
        } else {
            return Err(TransportError::UnknownProtocol);
        };
        self.session(tunnel, key, peer, !plaintext).await
    }
    pub(super) fn shared_reverse(&self, secure: bool) -> Option<Arc<ReverseProxy>> {
        let reverse = self.reverse.as_ref()?;
        let bind = if secure {
            &reverse.config.tls_bind
        } else {
            &reverse.config.bind
        };
        let address = felucca_core::config::parse_bind(bind.as_deref()?).ok()?;
        (address == felucca_core::config::parse_bind(&self.config.bridge.bind).ok()?)
            .then(|| reverse.clone())
    }

    /// WSS arriving through a standard HTTP reverse proxy has http/1.1 ALPN,
    /// not our bridge ALPN. Reserve the upgrade path on a shared HTTPS ingress.
    pub(crate) fn websocket_upgrade(
        self: Arc<Self>,
        request: &mut http::Request<crate::http::Body>,
        peer: SocketAddr,
        secure: bool,
    ) -> Option<http::Response<crate::http::Body>> {
        use base64::Engine;
        use http::{HeaderValue, Method, StatusCode, Version, header};
        if request.method() != Method::GET
            || request.uri().path() != self.config.bridge.ws_path
            || !crate::http::wants_upgrade(request.headers())
        {
            return None;
        }
        let bad =
            || crate::http::response(StatusCode::BAD_REQUEST, "invalid bridge WebSocket upgrade");
        let headers = request.headers();
        let Some(key) = headers.get(header::SEC_WEBSOCKET_KEY) else {
            return Some(bad());
        };
        if request.version() != Version::HTTP_11
            || headers.get_all(header::SEC_WEBSOCKET_KEY).iter().count() != 1
            || headers
                .get(header::SEC_WEBSOCKET_VERSION)
                .map(|value| value.as_bytes())
                != Some(b"13")
            || !headers
                .get(header::UPGRADE)
                .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"))
            || base64::engine::general_purpose::STANDARD
                .decode(key.as_bytes())
                .map_or(true, |bytes| bytes.len() != 16)
        {
            return Some(bad());
        }
        let accept = felucca_transport::ws::accept_key(key.as_bytes());
        let upgraded = hyper::upgrade::on(request);
        self.tasks.clone().spawn(async move {
            tokio::select! {
                () = self.cancel.cancelled() => {},
                _ = async {
                    let handshake = async {
                        let io = upgraded.await.map_err(|e| TransportError::tls(e.to_string()))?;
                        let mut ws = felucca_transport::WsConn::upgraded(hyper_util::rt::TokioIo::new(io)).await;
                        let key = self.auth(&mut ws).await?;
                        Ok::<_, TransportError>((key, ws))
                    };
                    if let Ok(Ok((key, ws))) = tokio::time::timeout(Duration::from_secs(30), handshake).await {
                        let tunnel = self.mux(ws);
                        let _ = self.clone().session(tunnel, key, peer, secure).await;
                    }
                } => {},
            }
        });
        let mut response = crate::http::response(StatusCode::SWITCHING_PROTOCOLS, "");
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        response
            .headers_mut()
            .insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        response.headers_mut().insert(
            header::SEC_WEBSOCKET_ACCEPT,
            HeaderValue::from_str(&accept).expect("base64 accept key"),
        );
        Some(response)
    }
    pub(super) fn mux<T: felucca_transport::Conn + 'static>(&self, io: T) -> Tunnel {
        let timeout = self.config.bridge.disconnect_timeout;
        Tunnel::over_stream(
            io,
            felucca_mux::Role::Server,
            felucca_mux::Config {
                keepalive_interval: timeout / 2,
                keepalive_timeout: timeout,
                ..felucca_transport::tunnel::mux_config(&self.config.bridge.mux)
            },
        )
    }
}
