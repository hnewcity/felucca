//! One-use direct sessions. The bridge retains authorization and accounting.
use felucca_core::{Secret, config::peer::P2pAgentConfig};
use felucca_proto::{
    StreamResponse, datagram,
    peer::{
        BrokerMessage, Direction, ProofSide, ProviderMessage, ProviderOffer, VisitorMessage,
        VisitorOffer,
    },
    read_frame, write_frame,
};
use felucca_transport::{
    BoxedConn, QuicStream, TunnelStream, network::Network, payload::PayloadStream,
};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    sync::{mpsc, oneshot},
};

fn denied() -> io::Error {
    io::ErrorKind::PermissionDenied.into()
}
async fn send<W: AsyncWrite + Unpin, T: serde::Serialize>(
    io: &mut W,
    message: &T,
) -> io::Result<()> {
    write_frame(io, message).await.map_err(io::Error::other)
}
async fn receive<R: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
    io: &mut R,
) -> io::Result<T> {
    read_frame(io).await.map_err(io::Error::other)
}

async fn observe(
    network: &Network,
    local: Option<IpAddr>,
    id: [u8; 16],
    role: u8,
    key: &Secret,
    broker: &str,
    timeout: Duration,
) -> io::Result<std::net::UdpSocket> {
    let addresses = network.resolve(broker).await.map_err(io::Error::other)?;
    let address = addresses
        .into_iter()
        .find(|address| local.is_none_or(|ip| ip.is_ipv4() == address.is_ipv4()))
        .ok_or(io::ErrorKind::AddrNotAvailable)?;
    let local = local.unwrap_or_else(|| {
        if address.is_ipv4() {
            Ipv4Addr::UNSPECIFIED.into()
        } else {
            Ipv6Addr::UNSPECIFIED.into()
        }
    });
    Ok(
        felucca_transport::rendezvous::observe(address, local, id, role, key, timeout)
            .await?
            .0,
    )
}

struct Endpoint(quinn::Endpoint);
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.0.close(0u32.into(), b"ticket finished");
    }
}

async fn command(commands: &mut mpsc::Receiver<BrokerMessage>) -> io::Result<BrokerMessage> {
    match commands.recv().await {
        Some(BrokerMessage::Rejected) | None => Err(denied()),
        Some(value) => Ok(value),
    }
}

/// The dispatcher drops its permits before acknowledging Finished.
pub(crate) async fn provide(
    stream: &mut TunnelStream,
    offer: ProviderOffer,
    services: Arc<felucca_service::Services>,
    config: &P2pAgentConfig,
    network: &Network,
    dial_timeout: Duration,
) -> io::Result<()> {
    if !config.enabled || !offer.valid() || !services.direct_allowed(&offer.route) {
        send(stream, &ProviderMessage::Prepared { fingerprint: None }).await?;
        return Ok(());
    }
    let (mut read, mut write) = tokio::io::split(stream);
    let (commands, mut input) = mpsc::channel(8);
    // A framing read is never cancelled just because the other direction progresses.
    let reader = async {
        loop {
            let message: BrokerMessage = receive(&mut read).await?;
            commands
                .send(message)
                .await
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
        }
        #[allow(unreachable_code)]
        Ok::<(), io::Error>(())
    };
    let work = async {
        let budget = Duration::from_millis(u64::from(offer.timeout_ms));
        let timeout = tokio::time::Instant::now() + budget * 3;
        let setup = async {
            let socket = tokio::select! {
                result = observe(network, config.bind_ip.or(network.local_ip()), offer.id, 0, &offer.probe_key, &offer.broker, budget) => result?,
                _ = input.recv() => return Err(denied()),
            };
            let punch = socket.try_clone()?;
            let identity = felucca_transport::Identity::self_signed(vec!["peer.felucca".into()])
                .map_err(io::Error::other)?;
            let endpoint = Endpoint(felucca_transport::peer::endpoint(socket, Some(&identity))?);
            send(
                &mut write,
                &ProviderMessage::Prepared {
                    fingerprint: Some(identity.fingerprint()),
                },
            )
            .await?;
            let BrokerMessage::Start { peer } = command(&mut input).await? else {
                return Err(denied());
            };
            let accept = async {
                // Reject unauthenticated attempts without consuming the route ticket.
                for _ in 0..8 {
                    let incoming = endpoint.0.accept().await.ok_or(io::ErrorKind::BrokenPipe)?;
                    let attempt = async {
                        let connection = incoming.await.map_err(io::Error::other)?;
                        let (send, recv) =
                            connection.accept_bi().await.map_err(io::Error::other)?;
                        let mut stream = QuicStream::new(send, recv);
                        felucca_proto::peer::authenticate(
                            &mut stream,
                            &offer.key,
                            &offer.id,
                            &offer.route,
                            ProofSide::Provider,
                        )
                        .await?;
                        Ok::<_, io::Error>((connection, stream))
                    };
                    if let Ok(Ok(result)) =
                        tokio::time::timeout(Duration::from_secs(1), attempt).await
                    {
                        return Ok(result);
                    }
                }
                Err(denied())
            };
            let punching = async {
                felucca_transport::peer::punch(punch, peer, &offer.id, &offer.probe_key).await?;
                std::future::pending::<io::Result<()>>().await
            };
            tokio::pin!(punching);
            let (connection, stream) = tokio::select! {
                result = accept => result?,
                _ = input.recv() => return Err(denied()),
                result = &mut punching => { result?; return Err(io::ErrorKind::TimedOut.into()); },
            };
            send(&mut write, &ProviderMessage::Connected).await?;
            if command(&mut input).await? != BrokerMessage::Commit {
                return Err(denied());
            }
            let stream = PayloadStream::negotiate(
                stream,
                offer.route.payload,
                &offer.key,
                &offer
                    .route
                    .request()
                    .payload_context()
                    .map_err(io::Error::other)?,
                felucca_transport::payload::Side::Agent,
            )
            .await?;
            Ok::<_, io::Error>((endpoint, connection, stream))
        };
        let (_endpoint, _connection, mut direct) = tokio::time::timeout_at(timeout, setup)
            .await
            .map_err(|_| io::ErrorKind::TimedOut)??;
        let (mut local, service_io) = tokio::io::duplex(64 * 1024);
        let service = services.serve_direct(service_io, offer.route.clone(), dial_timeout);
        let relay = async {
            let response: StreamResponse = receive(&mut local).await?;
            send(&mut direct, &response).await?;
            if response != StreamResponse::Ok {
                return Err(io::ErrorKind::ConnectionRefused.into());
            }
            let (credit, requests) = mpsc::channel(4);
            let copy = transfer(&mut direct, &mut local, offer.route.udp, credit);
            let actor = account(&mut write, &mut input, requests);
            tokio::try_join!(copy, actor)?;
            Ok::<_, io::Error>(())
        };
        // The service can finish once its last bytes are queued; drain them before
        // completing the relay, but drop it immediately if the direct peer fails.
        tokio::pin!(service);
        tokio::pin!(relay);
        tokio::select! {
            result = &mut relay => result?,
            () = &mut service => relay.await?,
        }
        Ok(())
    };
    tokio::select! { result = reader => result, result = work => result }
}

enum Credit {
    Grant(Direction, u32, oneshot::Sender<io::Result<u64>>),
    Commit(u64, Direction, u32, oneshot::Sender<io::Result<()>>),
}
async fn account<W: AsyncWrite + Unpin>(
    write: &mut W,
    commands: &mut mpsc::Receiver<BrokerMessage>,
    mut requests: mpsc::Receiver<Credit>,
) -> io::Result<()> {
    let mut next = 1u64;
    loop {
        let request = tokio::select! {
            message = commands.recv() => { let _ = message; return Err(denied()); },
            request = requests.recv() => request,
        };
        let Some(request) = request else { break };
        match request {
            Credit::Grant(direction, bytes, reply) => {
                let sequence = next;
                next = next.checked_add(1).ok_or_else(denied)?;
                send(
                    write,
                    &ProviderMessage::Grant {
                        sequence,
                        direction,
                        bytes,
                    },
                )
                .await?;
                if command(commands).await? != (BrokerMessage::Granted { sequence }) {
                    return Err(denied());
                }
                let _ = reply.send(Ok(sequence));
            }
            Credit::Commit(sequence, direction, bytes, reply) => {
                send(
                    write,
                    &ProviderMessage::Commit {
                        sequence,
                        direction,
                        bytes,
                    },
                )
                .await?;
                if command(commands).await? != (BrokerMessage::Recorded { sequence }) {
                    return Err(denied());
                }
                let _ = reply.send(Ok(()));
            }
        }
    }
    // Finished is also the dispatcher's cleanup acknowledgement; a distinct
    // zero-traffic close phase keeps the QUIC endpoint alive until the visitor
    // has consumed the final FIN.
    send(write, &ProviderMessage::Drained).await?;
    if command(commands).await? != BrokerMessage::Finished {
        return Err(denied());
    }
    Ok(())
}

async fn transfer<A: AsyncRead + AsyncWrite + Unpin, B: AsyncRead + AsyncWrite + Unpin>(
    direct: &mut A,
    local: &mut B,
    udp: bool,
    credit: mpsc::Sender<Credit>,
) -> io::Result<()> {
    let (ar, aw) = tokio::io::split(direct);
    let (br, bw) = tokio::io::split(local);
    tokio::try_join!(
        copy(ar, bw, udp, Direction::Inlet, credit.clone()),
        copy(br, aw, udp, Direction::Export, credit)
    )?;
    Ok(())
}
async fn copy<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut read: R,
    mut write: W,
    udp: bool,
    direction: Direction,
    credit: mpsc::Sender<Credit>,
) -> io::Result<()> {
    let mut buffer = vec![0; 16 * 1024];
    loop {
        let packet;
        let data = if udp {
            packet = datagram::read(&mut read).await?;
            packet.as_slice()
        } else {
            let n = read.read(&mut buffer).await?;
            if n == 0 {
                write.shutdown().await?;
                return Ok(());
            }
            &buffer[..n]
        };
        let (reply, grant) = oneshot::channel();
        credit
            .send(Credit::Grant(direction, data.len() as u32, reply))
            .await
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        let sequence = grant.await.map_err(|_| io::ErrorKind::BrokenPipe)??;
        let mut written = 0;
        let result = if udp {
            let result = datagram::write(&mut write, data).await;
            if result.is_ok() {
                written = data.len();
            }
            result
        } else {
            async {
                while written < data.len() {
                    let n = write.write(&data[written..]).await?;
                    if n == 0 {
                        return Err(io::ErrorKind::WriteZero.into());
                    }
                    written += n;
                }
                write.flush().await
            }
            .await
        };
        let (reply, recorded) = oneshot::channel();
        credit
            .send(Credit::Commit(sequence, direction, written as u32, reply))
            .await
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        recorded.await.map_err(|_| io::ErrorKind::BrokenPipe)??;
        result?;
    }
}

/// No application bytes are written during setup, so a failed setup can relay.
pub(crate) async fn connect(
    control: &mut TunnelStream,
    offer: &VisitorOffer,
    network: &Network,
    local: Option<IpAddr>,
) -> io::Result<Direct> {
    let budget = Duration::from_millis(u64::from(offer.timeout_ms));
    let (mut read, mut write) = tokio::io::split(control);
    let (output, mut input) = mpsc::channel(8);
    let reader = async {
        loop {
            let value: BrokerMessage = receive(&mut read).await?;
            let rejected = value == BrokerMessage::Rejected;
            output
                .send(value)
                .await
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
            if rejected {
                return std::future::pending::<io::Result<()>>().await;
            }
        }
    };
    let work = async {
        let setup = async {
            let socket = observe(
                network,
                local.or(network.local_ip()),
                offer.id,
                1,
                &offer.probe_key,
                &offer.broker,
                budget,
            )
            .await?;
            let endpoint = Endpoint(felucca_transport::peer::endpoint(socket, None)?);
            send(&mut write, &VisitorMessage::Observed).await?;
            if !matches!(command(&mut input).await?, BrokerMessage::Start { .. }) {
                return Err(denied());
            }
            let connection =
                felucca_transport::peer::connect(&endpoint.0, offer.provider, &offer.fingerprint)
                    .await?;
            let (quic_send, recv) = connection.open_bi().await.map_err(io::Error::other)?;
            let mut stream = QuicStream::new(quic_send, recv);
            felucca_proto::peer::authenticate(
                &mut stream,
                &offer.key,
                &offer.id,
                &offer.route,
                ProofSide::Visitor,
            )
            .await?;
            send(&mut write, &VisitorMessage::Connected).await?;
            if command(&mut input).await? != BrokerMessage::Commit {
                return Err(denied());
            }
            let mut stream = PayloadStream::negotiate(
                stream,
                offer.route.payload,
                &offer.key,
                &offer
                    .route
                    .request()
                    .payload_context()
                    .map_err(io::Error::other)?,
                felucca_transport::payload::Side::Server,
            )
            .await?;
            if receive::<_, StreamResponse>(&mut stream).await? != StreamResponse::Ok {
                return Err(io::ErrorKind::ConnectionRefused.into());
            }
            Ok(Direct {
                endpoint,
                connection,
                stream,
            })
        };
        match tokio::time::timeout(budget * 3, setup).await {
            Ok(Ok(direct)) => Ok(direct),
            result => {
                send(&mut write, &VisitorMessage::Abort).await?;
                tokio::time::timeout(budget * 3 + Duration::from_secs(4), async {
                    loop {
                        match input.recv().await {
                            Some(BrokerMessage::Rejected) => return Ok::<_, io::Error>(()),
                            Some(_) => {}
                            None => return Err(denied()),
                        }
                    }
                })
                .await
                .map_err(|_| io::ErrorKind::TimedOut)??;
                Err(match result {
                    Ok(Err(error)) => error,
                    _ => io::ErrorKind::TimedOut.into(),
                })
            }
        }
    };
    tokio::select! { result = reader => { result?; Err(denied()) }, result = work => result }
}

pub(crate) struct Direct {
    endpoint: Endpoint,
    connection: quinn::Connection,
    stream: PayloadStream<QuicStream>,
}
impl Direct {
    pub(crate) fn pipe(self, mut control: TunnelStream) -> BoxedConn {
        let (io, mut pipe) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(async move {
            let Self {
                endpoint: _endpoint,
                connection: _connection,
                mut stream,
            } = self;
            let copy = tokio::io::copy_bidirectional(&mut stream, &mut pipe);
            tokio::select! {
                result = copy => { result?; },
                _ = receive::<_, BrokerMessage>(&mut control) => return Err(denied()),
            }
            send(&mut control, &VisitorMessage::Finished).await?;
            if receive::<_, BrokerMessage>(&mut control).await? != BrokerMessage::Finished {
                return Err(denied());
            }
            Ok::<_, io::Error>(())
        });
        Box::new(PeerIo { io, task })
    }
}
struct PeerIo {
    io: tokio::io::DuplexStream,
    task: tokio::task::JoinHandle<io::Result<()>>,
}
impl Drop for PeerIo {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl AsyncRead for PeerIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}
impl AsyncWrite for PeerIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
