//! Server-owned routes and nested bridge endpoints.
use super::*;
impl Server {
    pub(crate) async fn nested_route(
        &self,
        key: &str,
        id: u32,
    ) -> std::io::Result<(TunnelSpec, Route)> {
        self.registrations.route(key, id).await
    }
    pub(crate) async fn nested_bridge(
        self: Arc<Self>,
        kind: felucca_core::config::local::BridgeTarget,
        context: felucca_transport::local::LocalContext,
    ) -> std::io::Result<felucca_transport::BoxedConn> {
        use felucca_core::config::local::BridgeTarget;
        let addresses = felucca_transport::proxy_protocol::Addresses {
            source: context.peer,
            destination: felucca_core::config::parse_bind(&self.config.bridge.bind)
                .map_err(std::io::Error::other)?,
        };
        if kind == BridgeTarget::Web {
            let admin = self
                .admin
                .read()
                .upgrade()
                .ok_or(std::io::ErrorKind::NotConnected)?;
            return Ok(crate::nested::spawn(move |pipe| async move {
                admin.serve(pipe, context.peer.ip()).await;
            }));
        }
        if kind == BridgeTarget::Http {
            let reverse = self
                .reverse
                .clone()
                .ok_or(std::io::ErrorKind::NotConnected)?;
            return Ok(crate::nested::spawn(move |pipe| async move {
                let cancel = self.cancel.clone();
                tokio::select! {
                    () = cancel.cancelled() => {},
                    () = crate::http::serve(pipe, move |request| {
                        let reverse = reverse.clone();
                        let trace = context.trace.clone();
                        async move { reverse.request_nested(request.map(crate::http::incoming), addresses, None, trace).await }
                    }) => {},
                }
            }));
        }
        Ok(crate::nested::spawn(move |mut pipe| async move {
            let cancel = self.cancel.clone();
            let work = async move {
                let ws = matches!(kind, BridgeTarget::Ws | BridgeTarget::Wss);
                if matches!(kind, BridgeTarget::Tls | BridgeTarget::Wss) {
                    let acceptor = tokio_rustls::TlsAcceptor::from(self.bridge_tls.load_full());
                    let Ok(Ok(mut io)) =
                        tokio::time::timeout(Duration::from_secs(15), acceptor.accept(pipe)).await
                    else {
                        return;
                    };
                    if ws {
                        let Ok(Ok(mut io)) = tokio::time::timeout(
                            Duration::from_secs(15),
                            felucca_transport::WsConn::accept_at(io, &self.config.bridge.ws_path),
                        )
                        .await
                        else {
                            return;
                        };
                        if let Ok(key) = self.auth(&mut io).await {
                            let tunnel = self.mux(io);
                            let _ = self.session(tunnel, key, context.peer, true).await;
                        }
                    } else if let Ok(key) = self.auth(&mut io).await {
                        let tunnel = self.mux(io);
                        let _ = self.session(tunnel, key, context.peer, true).await;
                    }
                } else if ws {
                    let Ok(Ok(mut io)) = tokio::time::timeout(
                        Duration::from_secs(15),
                        felucca_transport::WsConn::accept_at(pipe, &self.config.bridge.ws_path),
                    )
                    .await
                    else {
                        return;
                    };
                    if let Ok(key) = self.auth(&mut io).await {
                        let tunnel = self.mux(io);
                        let _ = self.session(tunnel, key, context.peer, false).await;
                    }
                } else if let Ok(key) = self.auth(&mut pipe).await {
                    let tunnel = self.mux(pipe);
                    let _ = self.session(tunnel, key, context.peer, false).await;
                }
            };
            tokio::select! {
                () = cancel.cancelled() => {},
                () = work => {},
            }
        }))
    }
    pub(super) async fn start_locals(self: &Arc<Self>) -> Result<(), ServerError> {
        for (name, config) in &self.config.locals {
            let key = format!("local.{name}");
            let (handle, mut listener) = felucca_transport::local::channel(128);
            let entry = felucca_core::config::ClientEntry {
                enabled: true,
                user: config.user.clone(),
                vkey: felucca_core::Secret::new(uuid::Uuid::new_v4().to_string()),
                remark: "server local service".into(),
                quota: config.limits.quota,
                rate_limit: config.limits.rate_limit,
                max_connections: config.limits.max_connections,
                max_tunnels: None,
                acl_mode: config.acl_mode,
                acl_rules: config.acl_rules.clone(),
                allow_config_push: true,
            };
            let client = self
                .registrations
                .attach(
                    &key,
                    &entry,
                    felucca_transport::TunnelHandle::Local(handle),
                    self.source_acls[&key].clone(),
                    false,
                )
                .await;
            let Control::ConfigResult { rejected, .. } = self
                .registrations
                .register(&felucca_service::specs(&config.tunnels), &client)
                .await
            else {
                unreachable!()
            };
            if !rejected.is_empty() {
                return Err(std::io::Error::other(format!(
                    "local {name:?} registration failed: {rejected:?}"
                ))
                .into());
            }
            let services = self.local_services[name].clone();
            let dial_timeout = config.dial_timeout;
            let cancel = self.cancel.clone();
            let dial = Arc::new(crate::nested::NestedDial {
                server: Arc::downgrade(self),
                user: client.user.clone(),
            });
            self.tasks.spawn(async move {
                let mut tasks = tokio::task::JoinSet::new();
                services.spawn_probes(&mut tasks);
                let slots = Arc::new(tokio::sync::Semaphore::new(4096));
                loop {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => break,
                        () = client.cancel.cancelled() => break,
                        _ = tasks.join_next(), if !tasks.is_empty() => {},
                        incoming = listener.accept() => {
                            let Some((stream, context)) = incoming else { break };
                            let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
                            let services = services.clone();
                            let vkey = client.vkey.clone();
                            let dial = dial.clone();
                            tasks.spawn(async move {
                                let _slot = slot;
                                services.serve_local(stream, dial_timeout, &vkey, Some((&*dial, context))).await;
                            });
                        }
                    }
                }
                tasks.shutdown().await;
            });
        }
        Ok(())
    }
}
