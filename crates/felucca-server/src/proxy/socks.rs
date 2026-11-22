//! SOCKS5 CONNECT and UDP ASSOCIATE, with optional RFC 1929 authentication.
use crate::relay::Route;
use felucca_service::socks::{datagram_target, encode_address, reply, target};
use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn serve(
    mut stream: impl felucca_transport::Conn + 'static,
    peer: SocketAddr,
    route: Route,
) {
    let Ok(_slot) = route.acquire(peer) else {
        return;
    };
    let (command, target) = match tokio::time::timeout(
        Duration::from_secs(15),
        target(&mut stream, &route.options.auth),
    )
    .await
    {
        Ok(Ok(t)) => t,
        _ => return,
    };
    if command == 3 {
        let _ = associate(stream, peer, route, &target).await;
        return;
    }
    match route.open(peer, Some(target)).await {
        Ok(mut tunnel) => {
            if reply(&mut stream, 0).await.is_ok() {
                crate::relay::splice(&mut stream, &mut tunnel, &route).await;
            }
        }
        Err(error) => {
            let _ = reply(
                &mut stream,
                if error.kind() == io::ErrorKind::PermissionDenied {
                    2
                } else {
                    5
                },
            )
            .await;
        }
    }
}

async fn associate(
    mut control: impl felucca_transport::Conn,
    peer: SocketAddr,
    route: Route,
    target: &str,
) -> io::Result<()> {
    let source: SocketAddr = match target.parse() {
        Ok(source) => source,
        Err(_) => {
            reply(&mut control, 8).await?;
            return Ok(());
        }
    };
    if !source.ip().is_unspecified() && source.ip().to_canonical() != peer.ip().to_canonical() {
        reply(&mut control, 2).await?;
        return Ok(());
    }
    let socket = Arc::new(
        felucca_transport::udp::bind_ingress(SocketAddr::new(route.destination.ip(), 0)).await?,
    );
    let bound = socket.local_addr()?;
    let mut answer = vec![5, 0, 0];
    encode_address(bound, &mut answer);
    control.write_all(&answer).await?;
    let mut sources = if source.port() == 0 {
        None
    } else {
        Some(SocketAddr::new(peer.ip(), source.port()))
    };
    let mut targets = HashMap::<String, felucca_transport::udp::queue::Sender>::new();
    let budget = Arc::new(tokio::sync::Semaphore::new(crate::udp::SHARED_QUEUE_BYTES));
    let mut tasks = tokio::task::JoinSet::new();
    let mut buffer = vec![0; 65536];
    let mut receiver = felucca_transport::udp::Receiver::new();
    let mut control_byte = [0];
    loop {
        tokio::select! {
            biased;
            () = route.cancel.cancelled() => break,
            _ = control.read(&mut control_byte) => break,
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            (n, from) = receiver.recv_from(&socket, &mut buffer) => {
                if from.ip().to_canonical() != peer.ip().to_canonical()
                    || sources.is_some_and(|source| source != from) { continue; }
                let Some((target, header_len)) = datagram_target(&buffer[..n]) else { continue; };
                sources = Some(from);
                targets.retain(|_, tx| !tx.is_closed());
                if !targets.contains_key(&target) {
                    if targets.len() >= 64 { continue; }
                    let (tx, rx) = crate::udp::packet_queue(budget.clone());
                    targets.insert(target.clone(), tx);
                    let socket = socket.clone();
                    let route = route.clone();
                    let header = buffer[..header_len].to_vec();
                    let destination = target.clone();
                    tasks.spawn(async move {
                        let _ = crate::udp::association(socket, route, from, rx, Some(destination), header).await;
                    });
                }
                let _ = targets[&target].try_send(&buffer[header_len..n]);
            }
        }
    }
    tasks.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn udp_headers_reject_fragments_and_truncation() {
        let mut packet = vec![0, 0, 0];
        encode_address("[::1]:53".parse().unwrap(), &mut packet);
        assert_eq!(datagram_target(&packet), Some(("[::1]:53".into(), 22)));
        for n in 0..packet.len() {
            assert!(datagram_target(&packet[..n]).is_none());
        }
        packet[2] = 1;
        assert!(datagram_target(&packet).is_none());
        assert!(datagram_target(&[0, 0, 0, 3, 0, 0, 53]).is_none());
        assert!(datagram_target(&[0, 0, 0, 1, 127, 0, 0, 1, 0, 0]).is_none());
    }
}
