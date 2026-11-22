//! The broker authorizes a route but never carries its direct application data.
use crate::{Server, registry::Client, resources::MeterGroup};
use felucca_core::{Secret, config::ShareConfig};
use felucca_proto::{
    StreamRequest,
    peer::{
        BrokerMessage, CreditLedger, Direction, PeerRoute, ProviderMessage, ProviderOffer,
        VisitorMessage, VisitorOffer,
    },
    read_frame,
    visitor::{VisitChallenge, VisitRequest, VisitResult},
    write_frame,
};
use felucca_transport::TunnelStream;
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
};

fn rejected() -> io::Error {
    io::ErrorKind::PermissionDenied.into()
}
async fn send<W: AsyncWrite + Unpin, T: serde::Serialize>(io: &mut W, value: &T) -> io::Result<()> {
    write_frame(io, value).await.map_err(io::Error::other)
}
async fn message<T>(input: &mut mpsc::Receiver<T>) -> io::Result<T> {
    input
        .recv()
        .await
        .ok_or_else(|| io::ErrorKind::BrokenPipe.into())
}
async fn pump<R, T>(mut read: R, send: mpsc::Sender<T>, terminal: fn(&T) -> bool) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    loop {
        let value: T = read_frame(&mut read).await.map_err(io::Error::other)?;
        let done = terminal(&value);
        send.send(value)
            .await
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        if done {
            // Keep the reader parked at this exact frame boundary. On fallback
            // the original visitor stream is resumed by its owner.
            return std::future::pending().await;
        }
    }
}
fn secret() -> Secret {
    let nonce = felucca_proto::handshake::generate_nonce();
    let mut value = String::with_capacity(64);
    for byte in nonce {
        use std::fmt::Write;
        write!(value, "{byte:02x}").expect("string");
    }
    value.into()
}
pub(crate) struct Visit<'a> {
    pub share: &'a ShareConfig,
    pub request: &'a VisitRequest,
    pub challenge: &'a VisitChallenge,
    pub visitor: &'a Arc<Client>,
    pub peer: SocketAddr,
}
/// Returns true if the direct session was served; false permits a fresh relay.
/// An error closes the visit, so a broken control channel cannot cause replay.
pub(crate) async fn attempt(
    server: &Arc<Server>,
    stream: &mut TunnelStream,
    visit: Visit<'_>,
) -> io::Result<bool> {
    let record = |state| {
        server.peer_history.record(
            &visit.share.client,
            visit.share.tunnel,
            &visit.visitor.key_id,
            state,
        )
    };
    let Some(config) = server.p2p_config().filter(|_| visit.share.direct) else {
        record("disabled");
        return Ok(false);
    };
    let Some(broker) = server.peer_broker.read().clone() else {
        record("broker_unavailable");
        return Ok(false);
    };
    let (spec, route) = server
        .nested_route(&visit.share.client, visit.share.tunnel)
        .await?;
    let dynamic = visit.request.target.is_some();
    let allowed = if dynamic {
        visit.share.dynamic
            && !spec.options.auth.required()
            && matches!(
                spec.mode,
                felucca_proto::TunnelMode::Socks5
                    | felucca_proto::TunnelMode::MixedProxy
                    | felucca_proto::TunnelMode::HttpProxy
            )
            && (!visit.request.datagram || spec.mode != felucca_proto::TunnelMode::HttpProxy)
    } else {
        matches!(
            spec.mode,
            felucca_proto::TunnelMode::Tcp | felucca_proto::TunnelMode::Udp
        ) && visit.request.datagram == (spec.mode == felucca_proto::TunnelMode::Udp)
    };
    if !allowed
        || !route
            .client
            .secure
            .load(std::sync::atomic::Ordering::Acquire)
        || visit.share.client.starts_with("local.")
        || (visit.request.datagram
            && spec.options.proxy_protocol != felucca_core::config::ProxyVersion::Off)
    {
        record("unsupported");
        return Ok(false);
    }
    let mut slot = Some(route.acquire(visit.peer)?);
    let mut provider = route.client.tunnel.open().await.map_err(io::Error::other)?;
    let key = secret();
    let probes = [secret(), secret()];
    let nonce = felucca_proto::handshake::generate_nonce();
    let id: [u8; 16] = nonce[..16].try_into().expect("nonce");
    let registration = match broker.register(id, [&probes[0], &probes[1]], config.timeout * 3) {
        Ok(registration) => registration,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            record("broker_busy");
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let ticket = PeerRoute {
        tunnel_id: visit.share.tunnel,
        source: visit.peer,
        destination: route.destination,
        target: visit.request.target.clone(),
        udp: visit.request.datagram,
        association: dynamic && visit.request.datagram,
        payload: route.options.payload,
        proxy_header: if visit.request.datagram {
            Vec::new()
        } else {
            felucca_transport::proxy_protocol::encode(
                route.options.proxy_protocol,
                felucca_transport::proxy_protocol::Addresses {
                    source: visit.peer,
                    destination: route.destination,
                },
                false,
            )
        },
    };
    let offer = ProviderOffer {
        id,
        key: key.clone(),
        probe_key: probes[0].clone(),
        broker: config.advertise.clone(),
        route: ticket.clone(),
        timeout_ms: config.timeout.as_millis() as u16,
    };
    send(&mut provider, &StreamRequest::Peer(Box::new(offer))).await?;
    let (pr, mut pw) = tokio::io::split(&mut provider);
    let (vr, mut vw) = tokio::io::split(stream);
    let (ps, mut pi) = mpsc::channel(8);
    let (vs, mut vi) = mpsc::channel(4);
    let readers = async {
        tokio::try_join!(
            pump(pr, ps, |m| *m == ProviderMessage::Finished),
            pump(vr, vs, |m| matches!(
                m,
                VisitorMessage::Abort | VisitorMessage::Finished
            ))
        )?;
        Ok::<_, io::Error>(())
    };
    let work = async {
        let mut offered = false;
        let mut aborted = false;
        let setup = async {
            let ProviderMessage::Prepared {
                fingerprint: Some(fingerprint),
            } = message(&mut pi).await?
            else {
                return Err(rejected());
            };
            let observed = registration.observed(0).await?;
            let offer = VisitorOffer {
                id,
                key,
                probe_key: probes[1].clone(),
                broker: config.advertise.clone(),
                route: ticket,
                timeout_ms: config.timeout.as_millis() as u16,
                provider: observed,
                fingerprint,
            };
            let proof = felucca_proto::visitor::prove_offer(
                visit.share.secret.as_bytes(),
                visit.request,
                visit.challenge,
                &visit.visitor.key_id,
                &offer,
            );
            send(
                &mut vw,
                &VisitResult::Direct {
                    offer: Box::new(offer),
                    proof,
                },
            )
            .await?;
            offered = true;
            let value = message(&mut vi).await?;
            aborted = value == VisitorMessage::Abort;
            if value != VisitorMessage::Observed {
                return Err(rejected());
            }
            let peer = registration.observed(1).await?;
            send(&mut pw, &BrokerMessage::Start { peer }).await?;
            send(&mut vw, &BrokerMessage::Start { peer: observed }).await?;
            let value = message(&mut vi).await?;
            aborted = value == VisitorMessage::Abort;
            if value != VisitorMessage::Connected {
                return Err(rejected());
            }
            if message(&mut pi).await? != ProviderMessage::Connected {
                return Err(rejected());
            }
            send(&mut pw, &BrokerMessage::Commit).await?;
            send(&mut vw, &BrokerMessage::Commit).await?;
            Ok::<_, io::Error>(())
        };
        let established = tokio::time::timeout(config.timeout * 3, setup)
            .await
            .is_ok_and(|result| result.is_ok());
        drop(registration);
        if established {
            let _diagnostic = server.peer_history.session(
                &visit.share.client,
                visit.share.tunnel,
                &visit.visitor.key_id,
            );
            let resources = MeterGroup(
                route
                    .resources
                    .0
                    .iter()
                    .chain(&visit.visitor.resources.0)
                    .cloned()
                    .collect(),
            );
            match accounting(
                &mut pw,
                &mut vw,
                &mut pi,
                &mut vi,
                &resources,
                visit.request.datagram,
            )
            .await?
            {
                End::Finished => {
                    // Finished means the provider has dropped its connection permits.
                    if message(&mut pi).await? != ProviderMessage::Finished {
                        return Err(rejected());
                    }
                    return Ok(true);
                }
                End::Abort => aborted = true,
            }
        }
        if !established {
            record("setup_failed");
        }
        let _ = send(&mut pw, &BrokerMessage::Rejected).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if message(&mut pi).await? == ProviderMessage::Finished {
                    return Ok::<_, io::Error>(());
                }
            }
        })
        .await
        .map_err(|_| io::ErrorKind::TimedOut)??;
        // Free the route and provider-side slots before opening the relay.
        drop(slot.take());
        if offered {
            if !aborted {
                tokio::time::timeout(config.timeout * 3 + Duration::from_secs(1), async {
                    loop {
                        if message(&mut vi).await? == VisitorMessage::Abort {
                            return Ok::<_, io::Error>(());
                        }
                    }
                })
                .await
                .map_err(|_| io::ErrorKind::TimedOut)??;
            }
            send(&mut vw, &BrokerMessage::Rejected).await?;
        }
        Ok(false)
    };
    tokio::select! {
        result = readers => { result?; Err(rejected()) },
        result = work => result,
        () = visit.visitor.cancel.cancelled() => Err(rejected()),
        () = route.cancel.cancelled() => Err(rejected()),
    }
}

enum End {
    Finished,
    Abort,
}
async fn accounting<P: AsyncWrite + Unpin, V: AsyncWrite + Unpin>(
    provider: &mut P,
    visitor: &mut V,
    pi: &mut mpsc::Receiver<ProviderMessage>,
    vi: &mut mpsc::Receiver<VisitorMessage>,
    resources: &MeterGroup,
    udp: bool,
) -> io::Result<End> {
    let mut ledger = CreditLedger::default();
    let mut visitor_done = false;
    let mut provider_done = false;
    let mut traffic = false;
    let expiry = async {
        while resources.permits() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::pin!(expiry);
    loop {
        let value = tokio::select! {
            () = &mut expiry => return Err(rejected()),
            value = message(vi), if !visitor_done => {
                match value? {
                    VisitorMessage::Finished => visitor_done = true,
                    VisitorMessage::Abort if !traffic => return Ok(End::Abort),
                    _ => return Err(rejected()),
                }
                None
            }
            value = message(pi), if !provider_done => Some(value?),
        };
        if let Some(value) = value {
            match value {
                ProviderMessage::Grant {
                    sequence,
                    direction,
                    bytes,
                } => {
                    ledger
                        .grant(
                            sequence,
                            direction,
                            bytes,
                            if udp { datagram_max() } else { 16 * 1024 },
                        )
                        .map_err(io::Error::other)?;
                    traffic = true;
                    tokio::select! {
                        () = &mut expiry => return Err(rejected()),
                        result = resources.pace(u64::from(bytes)) => result?,
                    }
                    send(provider, &BrokerMessage::Granted { sequence }).await?;
                }
                ProviderMessage::Commit {
                    sequence,
                    direction,
                    bytes,
                } => {
                    ledger
                        .commit(sequence, direction, bytes)
                        .map_err(io::Error::other)?;
                    resources.record(
                        match direction {
                            Direction::Inlet => crate::copy::Direction::Inlet,
                            Direction::Export => crate::copy::Direction::Export,
                        },
                        u64::from(bytes),
                    );
                    send(provider, &BrokerMessage::Recorded { sequence }).await?;
                }
                ProviderMessage::Drained if ledger.idle() => provider_done = true,
                _ => return Err(rejected()),
            }
        }
        if visitor_done && provider_done {
            send(provider, &BrokerMessage::Finished).await?;
            send(visitor, &BrokerMessage::Finished).await?;
            return Ok(End::Finished);
        }
    }
}
fn datagram_max() -> u32 {
    felucca_proto::datagram::MAX_DATAGRAM as u32
}
