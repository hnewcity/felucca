//! One bounded association per public UDP peer, with idle expiration.
use crate::relay::Route;
use felucca_transport::udp::queue::{self, Packet, Sender};
use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::UdpSocket,
    sync::{Semaphore, mpsc},
};
use tokio_util::sync::CancellationToken;

const IDLE: Duration = Duration::from_secs(60);
const MAX_ASSOCIATIONS: usize = 256;
const QUEUE_PACKETS: usize = 1024;
const PEER_QUEUE_BYTES: usize = 512 * 1024;
pub(crate) const SHARED_QUEUE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn packet_queue(budget: Arc<Semaphore>) -> (Sender, mpsc::Receiver<Packet>) {
    queue::channel(QUEUE_PACKETS, PEER_QUEUE_BYTES, budget)
}

pub async fn serve(socket: Arc<UdpSocket>, route: Route, cancel: CancellationToken) {
    let mut peers = HashMap::<SocketAddr, Sender>::new();
    let budget = Arc::new(Semaphore::new(SHARED_QUEUE_BYTES));
    let mut tasks = tokio::task::JoinSet::new();
    let mut buffer = vec![0; 65_536];
    let mut receiver = felucca_transport::udp::Receiver::new();
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = route.cancel.cancelled() => break,
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            (n, peer) = receiver.recv_from(&socket, &mut buffer) => {
                if n > felucca_proto::datagram::MAX_DATAGRAM { continue; }
                peers.retain(|_, tx| !tx.is_closed());
                if !peers.contains_key(&peer) {
                    if peers.len() >= MAX_ASSOCIATIONS { continue; }
                    let Ok(slot) = route.acquire(peer) else { continue; };
                    let (tx, rx) = packet_queue(budget.clone());
                    peers.insert(peer, tx);
                    let route = route.clone();
                    let socket = socket.clone();
                    tasks.spawn(async move {
                        let _slot = slot;
                        let _ = association(socket, route, peer, rx, None, Vec::new()).await;
                    });
                }
                // Dropping a packet under pressure preserves UDP semantics and bounds memory.
                let _ = peers[&peer].try_send(&buffer[..n]);
            }
        }
    }
    // Associations also own the socket. Wait for them so reconnect can rebind.
    tasks.shutdown().await;
}

pub(crate) async fn association(
    socket: Arc<UdpSocket>,
    route: Route,
    peer: SocketAddr,
    mut packets: mpsc::Receiver<Packet>,
    target: Option<String>,
    response_prefix: Vec<u8>,
) -> io::Result<()> {
    let stream = route
        .open_kind(peer, target.clone(), target.is_some())
        .await?;
    let (mut read, mut write) = tokio::io::split(stream);
    let touched = Arc::new(parking_lot::Mutex::new(tokio::time::Instant::now()));
    let send = async {
        while let Some(packet) = packets.recv().await {
            *touched.lock() = tokio::time::Instant::now();
            let mut wrapped = felucca_transport::proxy_protocol::encode(
                route.options.proxy_protocol,
                felucca_transport::proxy_protocol::Addresses {
                    source: peer,
                    destination: route.destination,
                },
                true,
            );
            if wrapped.len() + packet.len() > felucca_proto::datagram::MAX_DATAGRAM {
                continue;
            }
            wrapped.extend_from_slice(&packet);
            route.resources.pace(packet.len() as u64).await?;
            felucca_proto::datagram::write(&mut write, &wrapped).await?;
            route
                .resources
                .record(crate::copy::Direction::Inlet, packet.len() as u64);
        }
        Ok::<_, io::Error>(())
    };
    let receive = async {
        loop {
            let packet = felucca_proto::datagram::read(&mut read).await?;
            *touched.lock() = tokio::time::Instant::now();
            if response_prefix.len() + packet.len() > felucca_proto::datagram::MAX_DATAGRAM {
                continue;
            }
            let mut response = response_prefix.clone();
            response.extend_from_slice(&packet);
            route.resources.pace(packet.len() as u64).await?;
            socket.send_to(&response, peer).await?;
            route
                .resources
                .record(crate::copy::Direction::Export, packet.len() as u64);
        }
        #[allow(unreachable_code)]
        Ok::<_, io::Error>(())
    };
    let expiry = async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if touched.lock().elapsed() >= IDLE || !route.resources.permits() {
                break;
            }
        }
    };
    tokio::select! {
        r = send => r, r = receive => r,
        () = expiry => Ok(()),
        () = route.cancel.cancelled() => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn public_udp_queue_keeps_a_fifty_packet_burst_without_growing_memory_limits() {
        let budget = Arc::new(Semaphore::new(SHARED_QUEUE_BYTES));
        let (sender, mut receiver) = packet_queue(budget.clone());
        for i in 0..50u8 {
            sender.try_send(&[i; 512]).expect("short burst must fit");
        }
        for i in 0..50u8 {
            assert_eq!(&*receiver.recv().await.unwrap(), &[i; 512]);
        }
        assert_eq!(budget.available_permits(), SHARED_QUEUE_BYTES);
    }
}
