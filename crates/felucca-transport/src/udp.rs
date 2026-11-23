//! UDP sockets sized to carry complete IP datagrams on every supported platform.
use tokio::net::{ToSocketAddrs, UdpSocket};

pub mod queue;
mod receive;
pub use receive::Receiver;

// Shared ingress sockets must absorb a burst before their receive task runs.
// Two hundred 1,400-byte packets already exceed 256 KiB before kernel packet
// bookkeeping. Keep this separate from the smaller per-backend socket budget.
#[cfg(not(target_os = "macos"))]
pub const INGRESS_RECEIVE_BYTES: usize = 1024 * 1024;
// The macOS process probe needs room for several bursts across a scheduling
// gap, not only one 200-packet batch. Backend sockets retain their own budget.
#[cfg(target_os = "macos")]
pub const INGRESS_RECEIVE_BYTES: usize = 2 * 1024 * 1024;

pub async fn bind(address: impl ToSocketAddrs) -> std::io::Result<UdpSocket> {
    let socket = bind_plain(address).await?;
    let reference = socket2::SockRef::from(&socket);
    reference.set_send_buffer_size(256 * 1024)?;
    reference.set_recv_buffer_size(256 * 1024)?;
    Ok(socket)
}

/// Whether `address` is the IPv6 wildcard, which felucca treats as every
/// interface including IPv4. See [`crate::tcp::bind`].
pub(crate) fn dual_stack(address: std::net::SocketAddr) -> bool {
    matches!(address, std::net::SocketAddr::V6(v6) if v6.ip().is_unspecified())
}

/// Binds a UDP socket with the OS default buffers. An IPv6 wildcard address
/// also receives IPv4, as for TCP listeners.
pub async fn bind_plain(address: impl ToSocketAddrs) -> std::io::Result<UdpSocket> {
    let mut last = None;
    for address in tokio::net::lookup_host(address).await? {
        let result = (|| {
            use socket2::{Domain, Protocol, Socket, Type};
            let socket = Socket::new(
                Domain::for_address(address),
                Type::DGRAM,
                Some(Protocol::UDP),
            )?;
            if dual_stack(address) {
                socket.set_only_v6(false)?;
            }
            socket.bind(&address.into())?;
            socket.set_nonblocking(true)?;
            UdpSocket::from_std(socket.into())
        })();
        match result {
            Ok(socket) => return Ok(socket),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "could not resolve to any address",
        )
    }))
}

/// Bind a public/visitor/SOCKS ingress, whose socket receives complete bursts.
/// The OS may cap the requested size; no system-wide socket setting is changed.
pub async fn bind_ingress(address: impl ToSocketAddrs) -> std::io::Result<UdpSocket> {
    let socket = bind(address).await?;
    socket2::SockRef::from(&socket).set_recv_buffer_size(INGRESS_RECEIVE_BYTES)?;
    Ok(socket)
}

// Regress the macOS loss observed with its 256 KiB socket. Linux may clamp
// SO_RCVBUF below one burst via rmem_max; its running receive loop is covered
// by the process probe, without changing that machine-wide policy in tests.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn ingress_buffers_two_hundred_packets_before_the_reader_runs() {
        burst(200).await;
    }

    #[tokio::test]
    async fn ingress_keeps_several_bursts_during_a_receive_scheduling_gap() {
        burst(1280).await;
    }

    async fn burst(count: u32) {
        let receiver = bind_ingress("127.0.0.1:0").await.unwrap();
        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = receiver.local_addr().unwrap();
        // Hold the application reader until a complete burst has reached the
        // kernel. This isolates socket capacity from user-space queue capacity.
        for sequence in 0..count {
            let mut packet = [7; 1400];
            packet[..4].copy_from_slice(&sequence.to_be_bytes());
            sender.send_to(&packet, target).unwrap();
        }
        let mut received = Vec::new();
        let mut packet = [0; 1401];
        while received.len() < count as usize {
            let Ok(result) =
                tokio::time::timeout(Duration::from_millis(150), receiver.recv_from(&mut packet))
                    .await
            else {
                break;
            };
            let (length, peer) = result.unwrap();
            assert_eq!(peer, sender.local_addr().unwrap());
            assert_eq!(length, 1400);
            assert!(packet[4..length].iter().all(|byte| *byte == 7));
            received.push(u32::from_be_bytes(packet[..4].try_into().unwrap()));
        }
        assert_eq!(received, (0..count).collect::<Vec<u32>>());
    }
}
