//! In-process service entrypoints retain admission, ownership and route history.
use crate::{Server, relay::Route};
use felucca_core::config::local::VirtualTarget;
use felucca_proto::TunnelMode;
use felucca_transport::{BoxedConn, local::LocalContext};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Weak,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};

#[derive(Debug)]
pub(crate) struct NestedDial {
    pub server: Weak<Server>,
    /// None denotes a server operator; named users can only reach their own routes.
    pub user: Option<String>,
}

impl felucca_service::VirtualDial for NestedDial {
    fn open<'a>(
        &'a self,
        target: &'a str,
        context: LocalContext,
        datagram: bool,
    ) -> Pin<Box<dyn Future<Output = io::Result<BoxedConn>> + Send + 'a>> {
        Box::pin(async move {
            let server = self.server.upgrade().ok_or(io::ErrorKind::NotConnected)?;
            let target = VirtualTarget::parse(target)
                .map_err(io::Error::other)?
                .ok_or(io::ErrorKind::InvalidInput)?;
            let (client, id, file) = match target {
                VirtualTarget::Bridge(kind) => {
                    if self.user.is_some() || datagram {
                        return Err(denied("bridge targets require an operator-owned TCP route"));
                    }
                    return server.nested_bridge(kind, context).await;
                }
                VirtualTarget::Tunnel { client, id } => (client, id, false),
                VirtualTarget::File { client, id } => (client, id, true),
            };
            let (spec, mut route) = server.nested_route(&client, id).await?;
            if self
                .user
                .as_ref()
                .is_some_and(|owner| route.client.user.as_ref() != Some(owner))
            {
                return Err(denied("nested target belongs to a different user"));
            }
            if file && !spec.file {
                return Err(denied("file target is not a registered file service"));
            }
            context.trace.enter(&client, id)?;
            route.trace = context.trace;
            route.check(context.peer)?;
            if datagram != (spec.mode == TunnelMode::Udp) {
                return Err(denied("nested target transport does not match"));
            }
            match spec.mode {
                TunnelMode::Transparent => {
                    Err(denied("transparent routes require a real kernel socket"))
                }
                TunnelMode::Tcp | TunnelMode::Udp => {
                    let slot = route.acquire(context.peer)?;
                    let mut target = route.open(context.peer, None).await?;
                    Ok(spawn(move |mut pipe| async move {
                        let _slot = slot;
                        if datagram {
                            relay_datagrams(&mut pipe, &mut target, &route).await;
                        } else {
                            crate::relay::splice(&mut pipe, &mut target, &route).await;
                        }
                    }))
                }
                TunnelMode::Http => server.nested_http(route, context.peer).await,
                TunnelMode::Socks5 | TunnelMode::HttpProxy | TunnelMode::MixedProxy => {
                    let mode = spec.mode;
                    Ok(spawn(move |pipe| async move {
                        let mut pipe = pipe;
                        let mut mode = mode;
                        let mut prefix = bytes::Bytes::new();
                        if mode == TunnelMode::MixedProxy {
                            use tokio::io::AsyncReadExt;
                            let Ok(Ok(first)) = tokio::time::timeout(
                                std::time::Duration::from_secs(15),
                                pipe.read_u8(),
                            )
                            .await
                            else {
                                return;
                            };
                            mode = if first == 5 {
                                TunnelMode::Socks5
                            } else {
                                TunnelMode::HttpProxy
                            };
                            prefix = bytes::Bytes::copy_from_slice(&[first]);
                        }
                        let pipe = felucca_transport::PrefixedConn::new(prefix, pipe);
                        if mode == TunnelMode::Socks5 {
                            crate::socks::serve(pipe, context.peer, route).await;
                        } else {
                            crate::http::serve_forward(pipe, context.peer, route).await;
                        }
                    }))
                }
            }
        })
    }
}

fn denied(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason.to_owned())
}

/// Drop cancels the service as well as closing the pipe, including idle protocols.
pub(crate) fn spawn<F, Fut>(serve: F) -> BoxedConn
where
    F: FnOnce(DuplexStream) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (io, incoming) = tokio::io::duplex(32 * 1024);
    let task = tokio::spawn(serve(incoming));
    Box::new(TaskIo {
        io,
        task: task.abort_handle(),
    })
}

struct TaskIo {
    io: DuplexStream,
    task: tokio::task::AbortHandle,
}
impl Drop for TaskIo {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl AsyncRead for TaskIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}
impl AsyncWrite for TaskIo {
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

async fn relay_datagrams<A: felucca_transport::Conn, B: felucca_transport::Conn>(
    a: &mut A,
    b: &mut B,
    route: &Route,
) {
    relay_datagrams_scoped(a, b, &route.resources, &route.cancel).await;
}
pub(crate) async fn relay_datagrams_scoped<
    A: felucca_transport::Conn,
    B: felucca_transport::Conn,
>(
    a: &mut A,
    b: &mut B,
    resources: &crate::resources::MeterGroup,
    cancel: &tokio_util::sync::CancellationToken,
) {
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    async fn copy<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        mut r: R,
        mut w: W,
        resources: &crate::resources::MeterGroup,
        direction: crate::copy::Direction,
    ) -> io::Result<()> {
        loop {
            let packet = felucca_proto::datagram::read(&mut r).await?;
            resources.pace(packet.len() as u64).await?;
            felucca_proto::datagram::write(&mut w, &packet).await?;
            resources.record(direction, packet.len() as u64);
        }
    }
    tokio::select! {
        _ = copy(ar, bw, resources, crate::copy::Direction::Inlet) => {},
        _ = copy(br, aw, resources, crate::copy::Direction::Export) => {},
        () = cancel.cancelled() => {},
        () = async {
            while resources.permits() { tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
        } => {},
    }
}

impl Server {
    pub(crate) async fn nested_http(
        &self,
        route: Route,
        peer: std::net::SocketAddr,
    ) -> io::Result<BoxedConn> {
        let reverse = self.reverse_proxy().ok_or(io::ErrorKind::NotConnected)?;
        let addresses = felucca_transport::proxy_protocol::Addresses {
            source: peer,
            destination: route.destination,
        };
        let cancel = route.cancel.clone();
        Ok(spawn(move |mut pipe| async move {
            use tokio::io::AsyncReadExt;
            let work = async move {
                let Ok(Ok(first)) =
                    tokio::time::timeout(std::time::Duration::from_secs(15), pipe.read_u8()).await
                else {
                    return;
                };
                let pipe = felucca_transport::PrefixedConn::new(
                    bytes::Bytes::copy_from_slice(&[first]),
                    pipe,
                );
                if first == 0x16 {
                    reverse
                        .accept_tls_nested(
                            pipe,
                            addresses,
                            (route.client.id, route.id),
                            route.trace,
                        )
                        .await;
                } else {
                    crate::http::serve(pipe, move |request| {
                        let reverse = reverse.clone();
                        let route = route.clone();
                        async move {
                            reverse
                                .request_nested(
                                    request.map(crate::http::incoming),
                                    addresses,
                                    Some((route.client.id, route.id)),
                                    route.trace,
                                )
                                .await
                        }
                    })
                    .await;
                }
            };
            tokio::select! {
                () = cancel.cancelled() => {},
                () = work => {},
            }
        }))
    }
}
