//! Secret-gated access to ordinary service entrypoints, including private ones.
use crate::{Server, registry::Client};
use felucca_proto::{
    StreamReject, read_frame,
    visitor::{Side, VisitChallenge, VisitRequest, VisitResult},
    write_frame,
};
use felucca_service::VirtualDial;
use felucca_transport::{TunnelStream, local::LocalContext};
use std::{net::SocketAddr, sync::Arc, time::Duration};

impl Server {
    pub(crate) async fn visit(
        self: Arc<Self>,
        stream: TunnelStream,
        visitor: Arc<Client>,
        peer: SocketAddr,
        secure: bool,
        request: VisitRequest,
    ) {
        if !felucca_core::config::visitor::valid_name(&request.share) {
            return;
        }
        let policy = self.live.load_full();
        // Missing shares still receive the same challenge and coarse rejection.
        let cancel = policy
            .shares
            .get(&request.share)
            .map(|(_, cancel)| cancel.clone())
            .unwrap_or_default();
        tokio::select! {
            biased;
            () = cancel.cancelled() => {},
            () = visitor.cancel.cancelled() => {},
            () = self.clone().visit_authorized(stream, visitor.clone(), peer, secure, request, policy.clone()) => {},
        }
    }
    async fn visit_authorized(
        self: Arc<Self>,
        mut stream: TunnelStream,
        visitor: Arc<Client>,
        peer: SocketAddr,
        secure: bool,
        request: VisitRequest,
        policy: Arc<crate::runtime::live::LivePolicy>,
    ) {
        let handshake = async {
            let challenge = VisitChallenge {
                nonce: felucca_proto::handshake::generate_nonce(),
            };
            write_frame(&mut stream, &challenge)
                .await
                .map_err(|_| StreamReject::Internal)?;
            let proof: [u8; 32] = read_frame(&mut stream)
                .await
                .map_err(|_| StreamReject::Forbidden)?;
            let share = policy.config.shares.get(&request.share);
            let key = share.map_or(b"felucca-missing-share-placeholder".as_slice(), |s| {
                s.secret.as_bytes()
            });
            let valid = felucca_proto::visitor::verify(
                key,
                &request,
                &challenge,
                &visitor.key_id,
                Side::Visitor,
                &proof,
            );
            let allowed = secure
                && visitor.permits_source(peer.ip())
                && share.is_some_and(|s| {
                    (s.allow_clients.is_empty() || s.allow_clients.contains(&visitor.key_id))
                        && policy
                            .shares
                            .get(&request.share)
                            .is_some_and(|(acl, _)| acl.permits(peer.ip()))
                });
            if !valid || !allowed {
                return Err(StreamReject::Forbidden);
            }
            Ok(challenge)
        };
        let challenge = match tokio::time::timeout(Duration::from_secs(15), handshake).await {
            Ok(Ok(challenge)) => challenge,
            result => {
                let reason = match result {
                    Ok(Err(reason)) => reason,
                    _ => StreamReject::Timeout,
                };
                let _ = write_frame(&mut stream, &VisitResult::Err(reason)).await;
                return;
            }
        };
        let share = policy
            .config
            .shares
            .get(&request.share)
            .expect("authenticated share");
        let key = share.secret.as_bytes();
        let Some(_slot) = visitor.try_acquire() else {
            let _ = write_frame(
                &mut stream,
                &VisitResult::Err(StreamReject::TooManyConnections),
            )
            .await;
            return;
        };
        if request.connection != felucca_core::config::peer::VisitorConnection::Relay {
            match crate::peer::attempt(
                &self,
                &mut stream,
                crate::peer::Visit {
                    share,
                    request: &request,
                    challenge: &challenge,
                    visitor: &visitor,
                    peer,
                },
            )
            .await
            {
                Ok(true) => return,
                Ok(false)
                    if request.connection
                        == felucca_core::config::peer::VisitorConnection::PreferDirect => {}
                _ => {
                    let _ = write_frame(
                        &mut stream,
                        &VisitResult::Err(StreamReject::ConnectionRefused),
                    )
                    .await;
                    return;
                }
            }
        }
        self.peer_history
            .record(&share.client, share.tunnel, &visitor.key_id, "relay");
        let handshake = async {
            let dial = crate::nested::NestedDial {
                server: Arc::downgrade(&self),
                user: None,
            };
            let target = format!("tunnel://{}/{}", share.client, share.tunnel);
            let backend = if let Some(destination) = &request.target {
                if !share.dynamic
                    || destination.len() > 4096
                    || felucca_core::config::validate_endpoint(destination).is_err()
                {
                    return Err(StreamReject::Forbidden);
                }
                let (spec, route) = self
                    .nested_route(&share.client, share.tunnel)
                    .await
                    .map_err(|_| StreamReject::ConnectionRefused)?;
                // A dynamic share provides its own authentication, but must not
                // silently bypass a separately configured proxy account.
                if spec.options.auth.required()
                    || !matches!(
                        spec.mode,
                        felucca_proto::TunnelMode::Socks5
                            | felucca_proto::TunnelMode::HttpProxy
                            | felucca_proto::TunnelMode::MixedProxy
                    )
                    || (request.datagram && spec.mode == felucca_proto::TunnelMode::HttpProxy)
                {
                    return Err(StreamReject::Forbidden);
                }
                let slot = route
                    .acquire(peer)
                    .map_err(|_| StreamReject::TooManyConnections)?;
                let mut backend = route
                    .open_kind(peer, Some(destination.clone()), request.datagram)
                    .await
                    .map_err(|_| StreamReject::ConnectionRefused)?;
                let datagram = request.datagram;
                Ok(crate::nested::spawn(move |mut pipe| async move {
                    let _slot = slot;
                    if datagram {
                        crate::nested::relay_datagrams_scoped(
                            &mut pipe,
                            &mut backend,
                            &route.resources,
                            &route.cancel,
                        )
                        .await;
                    } else {
                        crate::relay::splice(&mut pipe, &mut backend, &route).await;
                    }
                }))
            } else {
                dial.open(
                    &target,
                    LocalContext {
                        peer,
                        trace: Default::default(),
                    },
                    request.datagram,
                )
                .await
            }
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::PermissionDenied => StreamReject::Forbidden,
                std::io::ErrorKind::TimedOut => StreamReject::Timeout,
                _ => StreamReject::ConnectionRefused,
            })?;
            let proof = felucca_proto::visitor::prove(
                key,
                &request,
                &challenge,
                &visitor.key_id,
                Side::Server,
            );
            write_frame(&mut stream, &VisitResult::Ok { proof })
                .await
                .map_err(|_| StreamReject::Internal)?;
            Ok(backend)
        };
        let mut backend = match tokio::time::timeout(Duration::from_secs(15), handshake).await {
            Ok(Ok(connected)) => connected,
            result => {
                let reason = match result {
                    Ok(Err(reason)) => reason,
                    Err(_) => StreamReject::Timeout,
                    _ => unreachable!(),
                };
                // Authentication failures are deliberately coarse. No share
                // identity, target address or credential appears in the reply.
                let _ = write_frame(&mut stream, &VisitResult::Err(reason)).await;
                return;
            }
        };
        if request.datagram {
            crate::nested::relay_datagrams_scoped(
                &mut stream,
                &mut backend,
                &visitor.resources,
                &visitor.cancel,
            )
            .await;
        } else {
            tokio::select! {
                _ = crate::copy::splice_scoped(&mut stream, &mut backend, &visitor.resources) => {},
                () = visitor.cancel.cancelled() => {},
                () = async {
                    while visitor.resources.permits() { tokio::time::sleep(Duration::from_millis(100)).await; }
                } => {},
            }
        }
    }
}
