//! Backends and probes shared by agents and server-owned local routes.
use crate::pool::Pool;
use felucca_core::{
    Secret, TrustPolicy,
    config::{TunnelConfig, TunnelMode},
};
use felucca_proto::{StreamReject, StreamRequest, StreamResponse, read_frame, write_frame};
use felucca_transport::{Conn, TransportError, network::Network};
use std::{collections::HashMap, sync::Arc, time::Duration};

pub trait VirtualDial: std::fmt::Debug + Send + Sync {
    fn open<'a>(
        &'a self,
        target: &'a str,
        context: felucca_transport::local::LocalContext,
        datagram: bool,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::io::Result<felucca_transport::BoxedConn>>
                + Send
                + 'a,
        >,
    >;
}

#[derive(Debug)]
pub struct Services {
    network: Arc<Network>,
    pools: HashMap<u32, Arc<Pool>>,
    routes: HashMap<u32, PreparedRoute>,
}
#[derive(Debug)]
struct PreparedRoute {
    config: felucca_core::config::TunnelConfig,
    target_acl: Arc<felucca_core::access::TargetAcl>,
    source_acl: felucca_core::Acl,
    file: Option<crate::file_server::FileServer>,
}
impl std::ops::Deref for PreparedRoute {
    type Target = felucca_core::config::TunnelConfig;
    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl Services {
    pub fn diagnostics(&self, id: u32) -> Option<felucca_proto::diagnostics::BackendReport> {
        diagnostic(&self.pools, &self.routes, id)
    }
    pub fn new(
        tunnels: &[TunnelConfig],
        geo_files: &felucca_core::geodata::GeoFiles,
        network: Arc<Network>,
    ) -> Result<Self, felucca_core::ConfigError> {
        Self::with_authority(tunnels, geo_files, network, None)
    }
    pub fn with_authority(
        tunnels: &[TunnelConfig],
        geo_files: &felucca_core::geodata::GeoFiles,
        network: Arc<Network>,
        authority: Option<&crate::remote::RemoteAuthority>,
    ) -> Result<Self, felucca_core::ConfigError> {
        let mut pools = HashMap::new();
        let mut routes = HashMap::new();
        let geo = felucca_core::geodata::GeoResolver::new(geo_files)?;
        for (index, tunnel) in tunnels.iter().enumerate().filter(|(_, t)| t.enabled) {
            let id = tunnel.id.unwrap_or(index as u32 + 1);
            pools.insert(id, Arc::new(Pool::new(&tunnel.backends, tunnel.balance)));
            routes.insert(
                id,
                PreparedRoute {
                    file: tunnel
                        .file
                        .as_ref()
                        .map(|file| match authority {
                            Some(authority) => authority.file(file),
                            None => crate::file_server::FileServer::new(file).map_err(|e| {
                                felucca_core::ConfigError::invalid(format!("file service: {e}"))
                            }),
                        })
                        .transpose()?,
                    config: tunnel.clone(),
                    target_acl: Arc::new({
                        let acl = felucca_core::access::TargetAcl::compile_with_geo(
                            &tunnel.target_policy,
                            &geo,
                        )?;
                        match authority {
                            Some(authority) => acl.intersect(authority.target_acl.clone()),
                            None => acl,
                        }
                    }),
                    source_acl: felucca_core::Acl::parse_with_geo(
                        tunnel.acl_mode,
                        &tunnel.acl_rules,
                        &geo,
                    )
                    .map_err(|error| felucca_core::ConfigError::invalid(error.to_string()))?,
                },
            );
        }
        Ok(Self {
            network,
            pools,
            routes,
        })
    }
    pub fn spawn_probes(&self, tasks: &mut tokio::task::JoinSet<()>) {
        for (id, route) in self.routes.iter() {
            if let Some(health) = route
                .health
                .as_ref()
                .filter(|h| h.kind != felucca_core::config::HealthCheckKind::None)
            {
                for member in self.pools[id].members() {
                    tasks.spawn(crate::health::monitor(
                        self.network.clone(),
                        member.clone(),
                        health.clone(),
                        route.target_acl.clone(),
                        route.options.http.backend_http2,
                    ));
                }
            }
        }
    }
    pub async fn serve<T: Conn + 'static>(&self, stream: T, dial_timeout: Duration, vkey: &Secret) {
        self.serve_local(stream, dial_timeout, vkey, None).await;
    }
    pub async fn serve_local<T: Conn + 'static>(
        &self,
        stream: T,
        dial_timeout: Duration,
        vkey: &Secret,
        virtual_dial: Option<(&dyn VirtualDial, felucca_transport::local::LocalContext)>,
    ) {
        serve_stream(
            stream,
            &self.pools,
            &self.routes,
            dial_timeout,
            &self.network,
            vkey,
            ServeContext {
                request: None,
                direct: None,
                virtual_dial,
            },
        )
        .await;
    }
    /// A bridge dispatcher has already decoded the first bounded frame.
    pub async fn serve_request<T: Conn + 'static>(
        &self,
        stream: T,
        request: StreamRequest,
        dial_timeout: Duration,
        vkey: &Secret,
    ) {
        serve_stream(
            stream,
            &self.pools,
            &self.routes,
            dial_timeout,
            &self.network,
            vkey,
            ServeContext {
                request: Some(request),
                direct: None,
                virtual_dial: None,
            },
        )
        .await;
    }
    pub fn direct_allowed(&self, ticket: &felucca_proto::peer::PeerRoute) -> bool {
        let Some(route) = self.routes.get(&ticket.tunnel_id) else {
            return false;
        };
        if !ticket.valid()
            || ticket.payload != route.options.payload
            || !route.source_acl.permits(ticket.source.ip())
            || route.backends.iter().any(|b| b.addr.contains("://"))
        {
            return false;
        }
        let allowed = match route.mode {
            TunnelMode::Tcp | TunnelMode::File => {
                ticket.target.is_none() && !ticket.udp && !ticket.association
            }
            TunnelMode::Udp => ticket.target.is_none() && ticket.udp && !ticket.association,
            TunnelMode::Socks5 | TunnelMode::MixedProxy | TunnelMode::HttpProxy => {
                ticket.target.is_some()
                    && !route.options.auth.required()
                    && ticket.udp == ticket.association
                    && (!ticket.udp || route.mode != TunnelMode::HttpProxy)
            }
            _ => false,
        };
        let header = felucca_transport::proxy_protocol::encode(
            route.options.proxy_protocol,
            felucca_transport::proxy_protocol::Addresses {
                source: ticket.source,
                destination: ticket.destination,
            },
            false,
        );
        allowed
            && if ticket.udp {
                header.is_empty()
            } else {
                header == ticket.proxy_header
            }
    }
    /// The outer peer stream already authenticates and transforms the payload.
    pub async fn serve_direct<T: Conn + 'static>(
        &self,
        mut stream: T,
        ticket: felucca_proto::peer::PeerRoute,
        dial_timeout: Duration,
    ) {
        if !self.direct_allowed(&ticket) {
            let _ = write_frame(&mut stream, &StreamResponse::Err(StreamReject::Forbidden)).await;
            return;
        }
        let request = ticket.request();
        serve_stream(
            stream,
            &self.pools,
            &self.routes,
            dial_timeout,
            &self.network,
            &Secret::from(""),
            ServeContext {
                request: Some(request),
                direct: Some(ticket.proxy_header),
                virtual_dial: None,
            },
        )
        .await;
    }
}
struct ServeContext<'a> {
    request: Option<StreamRequest>,
    direct: Option<Vec<u8>>,
    virtual_dial: Option<(&'a dyn VirtualDial, felucca_transport::local::LocalContext)>,
}
/// Serves one stream, retaining the backend lease until forwarding ends.
async fn serve_stream<T: Conn + 'static>(
    mut stream: T,
    pools: &HashMap<u32, Arc<Pool>>,
    routes: &HashMap<u32, PreparedRoute>,
    dial_timeout: Duration,
    network: &Network,
    vkey: &felucca_core::Secret,
    settings: ServeContext<'_>,
) {
    let request = match settings.request {
        Some(request) => request,
        None => {
            match tokio::time::timeout(dial_timeout, read_frame::<_, StreamRequest>(&mut stream))
                .await
            {
                Ok(Ok(request)) => request,
                _ => return,
            }
        }
    };
    if let StreamRequest::Diagnostics { tunnel_id } = request {
        if let Some(report) = diagnostic(pools, routes, tunnel_id) {
            let _ = tokio::time::timeout(Duration::from_secs(5), write_frame(&mut stream, &report))
                .await;
        }
        return;
    }
    let Ok(context) = request.payload_context() else {
        return;
    };
    let (id, source, named_target, datagram) = match request {
        StreamRequest::Routed {
            tunnel_id,
            source,
            target,
            datagram,
        } => (tunnel_id, Some(source), target, datagram),
        _ => {
            let _ = write_frame(&mut stream, &StreamResponse::Err(StreamReject::Forbidden)).await;
            return;
        }
    };
    let Some(route) = routes.get(&id) else {
        let _ = write_frame(&mut stream, &StreamResponse::Err(StreamReject::Forbidden)).await;
        return;
    };
    let mut stream = match felucca_transport::payload::PayloadStream::negotiate(
        stream,
        if settings.direct.is_some() {
            Default::default()
        } else {
            route.options.payload
        },
        vkey,
        &context,
        felucca_transport::payload::Side::Agent,
    )
    .await
    {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(%error, "payload negotiation failed");
            return;
        }
    };
    if (datagram && !matches!(route.mode, TunnelMode::Socks5 | TunnelMode::MixedProxy))
        || source.is_some_and(|ip| !route.source_acl.permits(ip))
    {
        let _ = write_frame(&mut stream, &StreamResponse::Err(StreamReject::Forbidden)).await;
        return;
    }
    let lease;
    if let Some(file) = &route.file {
        if named_target.is_some() {
            return;
        }
        if write_frame(&mut stream, &StreamResponse::Ok).await.is_ok() {
            file.serve(stream).await;
        }
        return;
    }
    let (target, encrypted, trust) = if route.mode.needs_backends() {
        if named_target.is_some() {
            return;
        }
        lease = pools.get(&id).and_then(|p| p.select(source));
        let Some(selected) = lease.as_ref() else {
            let _ = write_frame(&mut stream, &StreamResponse::Err(StreamReject::UnknownHost)).await;
            return;
        };
        (
            selected.addr().to_owned(),
            selected.tls(),
            selected.trust().clone(),
        )
    } else {
        let Some(target) = named_target else {
            return;
        };
        (target, false, TrustPolicy::Webpki)
    };
    if target.contains("://") {
        // Dynamic proxy destinations can never select server services.
        let Some((dial, context)) = settings
            .virtual_dial
            .filter(|_| route.mode.needs_backends())
        else {
            let _ = write_frame(&mut stream, &StreamResponse::Err(StreamReject::Forbidden)).await;
            return;
        };
        let result = tokio::time::timeout(
            dial_timeout,
            dial.open(&target, context, route.mode == TunnelMode::Udp),
        )
        .await;
        match result {
            Ok(Ok(mut local)) => {
                if write_frame(&mut stream, &StreamResponse::Ok).await.is_ok() {
                    let _ = tokio::io::copy_bidirectional(&mut local, &mut stream).await;
                }
            }
            result => {
                let reason = match result {
                    Err(_) => StreamReject::Timeout,
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                        StreamReject::Forbidden
                    }
                    _ => StreamReject::ConnectionRefused,
                };
                let _ = write_frame(&mut stream, &StreamResponse::Err(reason)).await;
            }
        }
        return;
    }
    if route.mode == TunnelMode::Udp || datagram {
        let outcome = tokio::time::timeout(dial_timeout, async {
            let address = crate::dial::resolve(network, &target, &route.target_acl)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| std::io::Error::other("no UDP backend address"))?;
            let bind = network
                .backend_local_ip()
                .map(|ip| std::net::SocketAddr::new(ip, 0).to_string())
                .unwrap_or_else(|| {
                    if address.is_ipv6() {
                        "[::]:0".into()
                    } else {
                        "0.0.0.0:0".into()
                    }
                });
            let socket = felucca_transport::udp::bind(&bind).await?;
            socket.connect(address).await?;
            Ok::<_, std::io::Error>(socket)
        })
        .await;
        match outcome {
            Ok(Ok(socket)) => {
                if write_frame(&mut stream, &StreamResponse::Ok).await.is_ok() {
                    serve_udp(stream, socket).await;
                }
            }
            result => {
                let _ = write_frame(
                    &mut stream,
                    &StreamResponse::Err(match result {
                        Err(_) => StreamReject::Timeout,
                        Ok(Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                            StreamReject::Forbidden
                        }
                        _ => StreamReject::ConnectionRefused,
                    }),
                )
                .await;
            }
        }
        return;
    }
    let mut local = match tokio::time::timeout(
        dial_timeout,
        crate::dial::connect_protocol(
            network,
            &target,
            encrypted,
            &trust,
            &route.target_acl,
            route.options.http.backend_http2,
        ),
    )
    .await
    {
        Ok(Ok(io)) => io,
        result => {
            let reason = match result {
                Err(_) => StreamReject::Timeout,
                Ok(Err(TransportError::Io(e)))
                    if e.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    StreamReject::Forbidden
                }
                _ => StreamReject::ConnectionRefused,
            };
            let _ = write_frame(&mut stream, &StreamResponse::Err(reason)).await;
            return;
        }
    };
    if let Some(header) = settings.direct {
        use tokio::io::AsyncWriteExt;
        if local.write_all(&header).await.is_err() || local.flush().await.is_err() {
            let _ = write_frame(
                &mut stream,
                &StreamResponse::Err(StreamReject::ConnectionRefused),
            )
            .await;
            return;
        }
    }
    if write_frame(&mut stream, &StreamResponse::Ok).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut local, &mut stream).await;
}

fn diagnostic(
    pools: &HashMap<u32, Arc<Pool>>,
    routes: &HashMap<u32, PreparedRoute>,
    id: u32,
) -> Option<felucca_proto::diagnostics::BackendReport> {
    let pool = pools.get(&id)?;
    let route = routes.get(&id)?;
    Some(felucca_proto::diagnostics::BackendReport {
        total: pool.members().len(),
        probing: route
            .health
            .as_ref()
            .is_some_and(|h| h.kind != felucca_core::config::HealthCheckKind::None),
        backends: pool
            .members()
            .iter()
            .take(felucca_proto::diagnostics::MAX_BACKENDS)
            .map(|member| member.diagnostic())
            .collect(),
    })
}

async fn serve_udp<T: Conn + 'static>(
    stream: felucca_transport::payload::PayloadStream<T>,
    socket: tokio::net::UdpSocket,
) {
    let (mut read, mut write) = tokio::io::split(stream);
    let outbound = async {
        loop {
            let packet = felucca_proto::datagram::read(&mut read).await?;
            socket.send(&packet).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    };
    let inbound = async {
        let mut buffer = vec![0; 65_536];
        loop {
            let n = socket.recv(&mut buffer).await?;
            felucca_proto::datagram::write(&mut write, &buffer[..n]).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    };
    tokio::select! { _ = outbound => {}, _ = inbound => {} }
}
