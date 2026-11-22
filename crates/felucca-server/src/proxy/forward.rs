//! Public TCP/UDP listeners and their lifecycle.
use crate::{registry::Client, relay::Route};
use felucca_proto::{TunnelMode, TunnelSpec};
use felucca_transport::{TransportError, tcp};
use std::{net::SocketAddr, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct Tunnel {
    pub id: u32,
    pub port: u16,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Tunnel {
    pub fn stop(&self) {
        self.cancel.cancel();
    }
    pub async fn stop_and_wait(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub async fn start_tcp(
    id: u32,
    bind: SocketAddr,
    client: Arc<Client>,
) -> Result<Tunnel, TransportError> {
    start(
        &TunnelSpec {
            public: true,
            file: false,
            tunnel_id: id,
            mode: TunnelMode::Tcp,
            port: bind.port(),
            bind: Some(bind.ip().to_string()),
            target: None,
            remark: String::new(),
            hosts: vec![],
            acl_mode: felucca_core::AclMode::Off,
            acl_rules: String::new(),
            options: Default::default(),
        },
        bind,
        client,
    )
    .await
}

pub async fn start(
    spec: &TunnelSpec,
    bind: SocketAddr,
    client: Arc<Client>,
) -> Result<Tunnel, TransportError> {
    let acl = felucca_core::Acl::parse(spec.acl_mode, &spec.acl_rules)
        .map_err(|e| TransportError::tls(e.to_string()))?;
    start_with_acl(spec, bind, client, acl).await
}

pub async fn start_with_acl(
    spec: &TunnelSpec,
    bind: SocketAddr,
    client: Arc<Client>,
    acl: felucca_core::Acl,
) -> Result<Tunnel, TransportError> {
    if spec.mode == TunnelMode::Udp
        && (spec.options.proxy_protocol == felucca_core::config::ProxyVersion::V1
            || spec.options.proxy_accept.mode != felucca_core::config::ProxyMode::Off)
    {
        return Err(TransportError::tls("UDP only supports sending PROXY v2"));
    }
    let original = if spec.mode == TunnelMode::Transparent {
        if spec.options.auth.required()
            || spec.options.proxy_accept.mode != felucca_core::config::ProxyMode::Off
        {
            return Err(TransportError::tls(
                "transparent TCP cannot parse proxy authentication or ingress PROXY headers",
            ));
        }
        Some(Arc::new(felucca_transport::transparent::Resolver::new(
            spec.options.transparent_tproxy,
        )?))
    } else {
        if spec.options.transparent_tproxy {
            return Err(TransportError::tls("TPROXY requires transparent mode"));
        }
        None
    };
    let route = Route {
        cancel: client.route_cancel(spec.tunnel_id),
        trace: Default::default(),
        resources: client.route_resources(spec.tunnel_id, spec.options.limits),
        host_scope: None,
        id: spec.tunnel_id,
        client,
        acl: Arc::new(acl),
        options: Arc::new(spec.options.clone()),
        destination: bind,
        datagram: spec.mode == TunnelMode::Udp,
    };
    let proxy_accept = felucca_transport::proxy_protocol::Acceptor::new(&spec.options.proxy_accept)
        .map_err(|e| TransportError::tls(e.to_string()))?;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let (port, task) = if spec.mode == TunnelMode::Udp {
        let socket = Arc::new(felucca_transport::udp::bind_ingress(bind).await?);
        (
            socket.local_addr()?.port(),
            tokio::spawn(crate::udp::serve(socket, route, stop)),
        )
    } else {
        let listener = if let Some(resolver) = &original {
            resolver.bind(bind).await?
        } else {
            tcp::bind(bind).await?
        };
        let port = listener.local_addr()?.port();
        let mode = spec.mode.clone();
        let task = tokio::spawn(async move {
            let mut listener = tcp::Acceptor::new(listener);
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => break,
                    () = route.cancel.cancelled() => break,
                    _ = tasks.join_next(), if !tasks.is_empty() => {},
                    (mut stream, _) = listener.accept() => {
                        let mut route = route.clone(); let mode = mode.clone();
                        let proxy_accept = proxy_accept.clone();
                        let original = original.clone();
                        tasks.spawn(async move {
                            let Ok(addresses) = proxy_accept.accept(&mut stream).await else { return; };
                            let peer = addresses.source;
                            route.destination = addresses.destination;
                            // Admission before even parsing a public proxy handshake.
                            if route.check(peer).is_err() { return; }
                            let mode = if mode == TunnelMode::MixedProxy {
                                let mut byte = [0];
                                match tokio::time::timeout(std::time::Duration::from_secs(15), stream.peek(&mut byte)).await {
                                    Ok(Ok(1)) if byte[0] == 5 => TunnelMode::Socks5,
                                    Ok(Ok(1)) => TunnelMode::HttpProxy,
                                    _ => return,
                                }
                            } else { mode };
                            match mode {
                                TunnelMode::Transparent => {
                                    let Ok(_slot) = route.acquire(peer) else { return; };
                                    let Some(resolver) = original else { return; };
                                    let target = match resolver.destination(&stream, bind) {
                                        Ok(target) => target,
                                        Err(error) => { tracing::debug!(%peer, %error, "transparent destination rejected"); return; }
                                    };
                                    route.destination = target;
                                    let Ok(mut tunnel) = route.open(peer, Some(target.to_string())).await else { return; };
                                    crate::relay::splice(&mut stream, &mut tunnel, &route).await;
                                },
                                TunnelMode::Socks5 => crate::socks::serve(stream, peer, route).await,
                                TunnelMode::HttpProxy => crate::http::serve_forward(stream, peer, route).await,
                                TunnelMode::Tcp => {
                                    let Ok(_slot) = route.acquire(peer) else { return; };
                                    let Ok(mut tunnel) = route.open(peer, None).await else { return; };
                                    let mut stream = stream;
                                    crate::relay::splice(&mut stream, &mut tunnel, &route).await;
                                },
                                _ => {},
                            }
                        });
                    }
                }
            }
        });
        (port, task)
    };
    Ok(Tunnel {
        id: spec.tunnel_id,
        port,
        cancel,
        task: Some(task),
    })
}
