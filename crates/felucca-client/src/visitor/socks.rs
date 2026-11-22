//! A SOCKS visitor terminates UDP ASSOCIATE locally, so its returned address is
//! reachable on the visitor's network rather than the relay server's network.
use super::{Packet, Runtime};
use felucca_service::socks::{datagram_target, encode_address, reply, target};
use std::{collections::HashMap, io, net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    task::JoinSet,
};

pub(super) async fn serve(
    runtime: Arc<Runtime>,
    mut stream: TcpStream,
    peer: SocketAddr,
) -> io::Result<()> {
    let (command, destination) =
        tokio::time::timeout(runtime.timeout, target(&mut stream, &runtime.config.auth))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    if command == 3 {
        return associate(runtime, stream, peer, &destination).await;
    }
    match runtime.open_target(Some(destination), false).await {
        Ok(mut remote) => {
            reply(&mut stream, 0).await?;
            tokio::io::copy_bidirectional(&mut stream, &mut remote).await?;
            Ok(())
        }
        Err(error) => {
            reply(
                &mut stream,
                if error.kind() == io::ErrorKind::PermissionDenied {
                    2
                } else {
                    5
                },
            )
            .await?;
            Err(error)
        }
    }
}

async fn associate(
    runtime: Arc<Runtime>,
    mut control: TcpStream,
    peer: SocketAddr,
    destination: &str,
) -> io::Result<()> {
    let source: SocketAddr = match destination.parse() {
        Ok(source) => source,
        Err(_) => return reply(&mut control, 8).await,
    };
    if !source.ip().is_unspecified() && source.ip().to_canonical() != peer.ip().to_canonical() {
        return reply(&mut control, 2).await;
    }
    let socket = Arc::new(
        felucca_transport::udp::bind_ingress(SocketAddr::new(control.local_addr()?.ip(), 0))
            .await?,
    );
    let mut answer = vec![5, 0, 0];
    encode_address(socket.local_addr()?, &mut answer);
    control.write_all(&answer).await?;
    let mut source = (source.port() != 0).then(|| SocketAddr::new(peer.ip(), source.port()));
    let mut targets = HashMap::<String, mpsc::Sender<Packet>>::new();
    let mut tasks = JoinSet::new();
    let mut buffer = vec![0; 65536];
    let mut receiver = felucca_transport::udp::Receiver::new();
    let mut control_byte = [0];
    loop {
        tokio::select! {
            biased;
            _ = control.read(&mut control_byte) => return Ok(()),
            _ = tasks.join_next(), if !tasks.is_empty() => {}
            (n, from) = receiver.recv_from(&socket, &mut buffer) => {
                if from.ip().to_canonical() != peer.ip().to_canonical()
                    || source.is_some_and(|source| source != from) { continue; }
                let Some((destination, offset)) = datagram_target(&buffer[..n]) else { continue };
                source = Some(from);
                targets.retain(|_, send| !send.is_closed());
                let Ok(budget) = runtime.bytes.clone().try_acquire_many_owned((n - offset).max(1) as u32) else { continue };
                if !targets.contains_key(&destination) {
                    if targets.len() >= 64 { continue; }
                    let Some(slots) = runtime.admit(from) else { continue };
                    let (send, receive) = mpsc::channel(8);
                    targets.insert(destination.clone(), send);
                    let runtime = runtime.clone();
                    let socket = socket.clone();
                    let destination = destination.clone();
                    let header = buffer[..offset].to_vec();
                    tasks.spawn(async move {
                        let _slots = slots;
                        let _ = runtime.udp_target(socket, from, receive, Some(destination), header).await;
                    });
                }
                let _ = targets[&destination].try_send(Packet { data: buffer[offset..n].to_vec(), _budget: budget });
            }
        }
    }
}
