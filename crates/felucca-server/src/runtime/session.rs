//! Authenticated agent sessions and control messages.
use super::*;
/// Cancelling a transport future must retire the authenticated session as well.
struct SessionGuard {
    server: Arc<Server>,
    client: Arc<Client>,
    armed: bool,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.client.cancel.cancel();
        self.client.tunnel.close(felucca_mux::CloseReason::Shutdown);
        let server = self.server.clone();
        let client = self.client.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let tasks = server.tasks.clone();
            tasks.spawn_on(
                async move {
                    server.retire_session(&client).await;
                },
                &runtime,
            );
        }
    }
}

impl Server {
    pub(super) async fn auth<T: AsyncRead + AsyncWrite + Unpin>(
        &self,
        io: &mut T,
    ) -> Result<(String, felucca_core::Secret), TransportError> {
        let policy = self.live.load_full();
        felucca_proto::session::accept_with_clock(
            io,
            AcceptPolicy {
                clients: &policy.config.clients,
                strict_clock: self.config.bridge.strict_clock,
                keepalive_secs: felucca_proto::session::keepalive_for(
                    self.config.bridge.disconnect_timeout,
                ),
            },
            || self.clock.now_unix(),
        )
        .await
        .map(|auth| {
            let secret = policy.config.clients[&auth.key_id].vkey.clone();
            (auth.key_id, secret)
        })
        .map_err(|e| TransportError::tls(e.to_string()))
    }
    pub(super) async fn session(
        self: Arc<Self>,
        mut tunnel: Tunnel,
        identity: (String, felucca_core::Secret),
        peer: SocketAddr,
        secure: bool,
    ) -> Result<(), TransportError> {
        let (key, secret) = identity;
        let gate = self.policy_gate.read().await;
        let policy = self.live.load_full();
        // Authentication may have started before a key was revoked or rotated.
        let entry = policy
            .config
            .clients
            .get(&key)
            .filter(|entry| entry.enabled && entry.vkey == secret)
            .ok_or_else(|| TransportError::Io(std::io::ErrorKind::PermissionDenied.into()))?;
        // The client ACL controls public traffic; it must not also deny its agent's bridge address.
        let client = self
            .registrations
            .attach(
                &key,
                entry,
                tunnel.handle(),
                policy.sources[&key].clone(),
                secure,
            )
            .await;
        drop(gate);
        let mut guard = SessionGuard {
            server: self.clone(),
            client: client.clone(),
            armed: true,
        };
        tracing::info!(%peer, key_id = %key, "agent connected");
        let mut tasks = tokio::task::JoinSet::new();
        let result = loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => break Ok(()),
                () = client.cancel.cancelled() => break Ok(()),
                _ = tasks.join_next(), if !tasks.is_empty() => {},
                accepted = tunnel.accept() => {
                    match accepted {
                        Ok(stream) => { let this = self.clone(); let client = client.clone(); tasks.spawn(async move { this.control(stream, client, peer, secure).await; }); },
                        Err(e) => break Err(e),
                    }
                }
            }
        };
        tasks.abort_all();
        client.cancel.cancel();
        tunnel.close(felucca_mux::CloseReason::Shutdown);
        self.retire_session(&client).await;
        guard.armed = false;
        result
    }
    pub(super) async fn retire_session(&self, client: &Arc<Client>) {
        self.registrations.retire(client).await;
    }
    pub(super) async fn control(
        self: Arc<Self>,
        mut stream: TunnelStream,
        client: Arc<Client>,
        peer: SocketAddr,
        secure: bool,
    ) {
        match tokio::time::timeout(
            Duration::from_secs(15),
            read_frame::<_, StreamRequest>(&mut stream),
        )
        .await
        {
            Ok(Ok(StreamRequest::Control)) => {}
            Ok(Ok(StreamRequest::Visit(request))) => {
                self.visit(stream, client, peer, secure, request).await;
                return;
            }
            _ => return,
        }
        loop {
            match read_frame::<_, Control>(&mut stream).await {
                Ok(Control::WatchConfig) => {
                    client
                        .live_config
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                Ok(Control::ConfigResult { rejected, .. }) => {
                    // Agent-side preparation failed. Only IDs cross the display
                    // boundary; detailed paths and errors stay in its own log.
                    client.config_state.write().rejected = rejected
                        .into_iter()
                        .take(4096)
                        .map(|entry| entry.tunnel_id)
                        .collect();
                }
                Ok(Control::PushConfig {
                    tunnels,
                    remote_config,
                    revision,
                }) => {
                    let reply = self
                        .configure(&tunnels, &client, remote_config, revision, secure)
                        .await;
                    if write_frame(&mut stream, &reply).await.is_err() {
                        break;
                    }
                }
                Ok(Control::Ping { nonce }) => {
                    let changed = {
                        let policy = self.live.load();
                        let state = client.config_state.read();
                        state.dirty
                            || state.revision
                                != policy.managed_revisions.get(&client.key_id).copied()
                            || !state.rejected.is_empty()
                    };
                    let reply = if nonce == felucca_proto::message::LIVE_CONFIG_QUERY
                        || (changed
                            && client
                                .live_config
                                .load(std::sync::atomic::Ordering::Acquire))
                    {
                        Control::ConfigChanged
                    } else {
                        Control::Pong { nonce }
                    };
                    if write_frame(&mut stream, &reply).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
    pub(super) async fn configure(
        &self,
        tunnels: &[TunnelSpec],
        client: &Arc<Client>,
        remote_config: bool,
        revision: Option<[u8; 32]>,
        secure: bool,
    ) -> Control {
        let _gate = self.policy_gate.read().await;
        let reply = self
            .configure_current(tunnels, client, remote_config, revision, secure)
            .await;
        if let Control::ConfigResult { rejected, .. } = &reply {
            *client.config_state.write() = crate::registry::ConfigState {
                revision,
                rejected: rejected.iter().map(|entry| entry.tunnel_id).collect(),
                dirty: false,
            };
        }
        reply
    }
    async fn configure_current(
        &self,
        tunnels: &[TunnelSpec],
        client: &Arc<Client>,
        remote_config: bool,
        revision: Option<[u8; 32]>,
        secure: bool,
    ) -> Control {
        let policy = self.live.load_full();
        let reject = |reason: &str| Control::ConfigResult {
            accepted: Vec::new(),
            rejected: vec![felucca_proto::ConfigRejection {
                tunnel_id: 0,
                reason: reason.into(),
            }],
        };
        if client.cancel.is_cancelled() {
            return reject("client may not register tunnels");
        }
        if let Some(desired) = policy.config.managed.get(&client.key_id) {
            if !secure {
                return reject("managed configuration requires an encrypted bridge");
            }
            if !remote_config {
                return reject("this client requires remote configuration opt-in");
            }
            let Ok((config, digest)) = desired.encoded() else {
                return reject("invalid managed configuration");
            };
            if revision != Some(digest) {
                return Control::Configure {
                    revision: digest,
                    config,
                };
            }
            if felucca_service::specs(&desired.tunnels) != tunnels {
                return reject("registered tunnels do not match managed configuration");
            }
            self.registrations.reconcile(client, tunnels).await;
        } else {
            if revision.is_some() {
                return Control::RestoreConfig;
            }
            if !client.allow_config_push && !tunnels.is_empty() {
                return reject("client may not register tunnels");
            }
        }
        self.registrations.register(tunnels, client).await
    }
}
