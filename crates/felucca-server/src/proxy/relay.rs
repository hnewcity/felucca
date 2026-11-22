//! Admission, stream negotiation and metering shared by every public entrypoint.
use crate::registry::{Client, ConnectionGuard};
use felucca_core::Acl;
use felucca_proto::{StreamRequest, StreamResponse, read_frame, write_frame};
use felucca_transport::TunnelStream;
use felucca_transport::payload::{PayloadStream, Side};
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Debug, Clone)]
pub struct Route {
    pub cancel: tokio_util::sync::CancellationToken,
    pub trace: felucca_transport::local::RouteTrace,
    pub resources: crate::resources::MeterGroup,
    pub host_scope: Option<String>,
    pub id: u32,
    pub client: Arc<Client>,
    pub acl: Arc<Acl>,
    pub options: Arc<felucca_core::config::RouteOptions>,
    pub destination: SocketAddr,
    pub datagram: bool,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

impl Route {
    pub fn check(&self, peer: SocketAddr) -> io::Result<()> {
        if !self.acl.permits(peer.ip())
            || !self.client.permits_source(peer.ip())
            || !self.resources.permits()
            || self.cancel.is_cancelled()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "route denied",
            ));
        }
        Ok(())
    }
    pub fn acquire(&self, peer: SocketAddr) -> io::Result<ConnectionGuard> {
        self.check(peer)?;
        self.resources
            .acquire()
            .map(ConnectionGuard::new)
            .ok_or_else(|| io::Error::other("connection limit reached"))
    }

    pub async fn open(
        &self,
        peer: SocketAddr,
        target: Option<String>,
    ) -> io::Result<PayloadStream<TunnelStream>> {
        self.open_kind(peer, target, false).await
    }

    pub async fn open_kind(
        &self,
        peer: SocketAddr,
        target: Option<String>,
        datagram: bool,
    ) -> io::Result<PayloadStream<TunnelStream>> {
        let open = tokio::time::timeout(Duration::from_secs(15), async {
            let trace = self.trace.enter(&self.client.key_id, self.id)?;
            let mut stream = self
                .client
                .tunnel
                .open_context(felucca_transport::local::LocalContext { peer, trace })
                .await
                .map_err(io::Error::other)?;
            let request = StreamRequest::Routed {
                tunnel_id: self.id,
                source: peer.ip(),
                target,
                datagram,
            };
            write_frame(&mut stream, &request)
                .await
                .map_err(io::Error::other)?;
            let mut stream = PayloadStream::negotiate(
                stream,
                self.options.payload,
                &self.client.vkey,
                &request.payload_context().map_err(io::Error::other)?,
                Side::Server,
            )
            .await?;
            match read_frame::<_, StreamResponse>(&mut stream)
                .await
                .map_err(io::Error::other)?
            {
                StreamResponse::Ok => {
                    if !datagram && !self.datagram {
                        use tokio::io::AsyncWriteExt;
                        let header = felucca_transport::proxy_protocol::encode(
                            self.options.proxy_protocol,
                            felucca_transport::proxy_protocol::Addresses {
                                source: peer,
                                destination: self.destination,
                            },
                            false,
                        );
                        stream.write_all(&header).await?;
                        stream.flush().await?;
                    }
                    Ok(stream)
                }
                StreamResponse::Err(reason) => Err(io::Error::new(
                    if reason == felucca_proto::StreamReject::Forbidden {
                        io::ErrorKind::PermissionDenied
                    } else {
                        io::ErrorKind::ConnectionRefused
                    },
                    format!("backend rejected: {reason:?}"),
                )),
            }
        });
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(io::ErrorKind::Interrupted.into()),
            result = open => result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "agent response timed out"))?,
        }
    }

    /// An HTTP driver's byte stream; the copy task owns its connection permit.
    pub async fn metered(
        &self,
        peer: SocketAddr,
        target: Option<String>,
    ) -> io::Result<tokio::io::DuplexStream> {
        let slot = self.acquire(peer)?;
        let mut tunnel = self.open(peer, target).await?;
        let (io, mut public) = tokio::io::duplex(64 * 1024);
        let route = self.clone();
        tokio::spawn(async move {
            let _slot = slot;
            splice(&mut public, &mut tunnel, &route).await;
        });
        Ok(io)
    }
}

pub async fn splice<A, B>(public: &mut A, tunnel: &mut B, route: &Route)
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    tokio::select! {
        _ = crate::copy::splice_scoped(public, tunnel, &route.resources) => {},
        () = route.cancel.cancelled() => {},
        () = async {
            loop {
                if !route.resources.permits() { break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } => {},
    }
}
