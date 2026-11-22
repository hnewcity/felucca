//! Kernel original-destination lookup. No firewall rules are installed here.
use std::{io, net::SocketAddr};
use tokio::net::{TcpListener, TcpStream};

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
#[path = "transparent/pf.rs"]
mod pf;

#[derive(Debug)]
pub struct Resolver {
    tproxy: bool,
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    pf: std::fs::File,
}

impl Resolver {
    pub fn new(tproxy: bool) -> io::Result<Self> {
        if !cfg!(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "freebsd"
        )) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "transparent TCP requires Linux/Android or macOS/FreeBSD PF",
            ));
        }
        if tproxy && !cfg!(any(target_os = "linux", target_os = "android")) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "TPROXY listener options require Linux/Android",
            ));
        }
        Ok(Self {
            tproxy,
            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            pf: std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/pf")?,
        })
    }

    pub async fn bind(&self, address: SocketAddr) -> io::Result<TcpListener> {
        if self.tproxy && !cfg!(any(target_os = "linux", target_os = "android")) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "TPROXY requires Linux/Android",
            ));
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if self.tproxy {
            let socket = socket2::Socket::new(
                socket2::Domain::for_address(address),
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            socket.set_nonblocking(true)?;
            socket.set_reuse_address(true)?;
            if address.is_ipv6() {
                socket.set_ip_transparent_v6(true)?;
            } else {
                socket.set_ip_transparent_v4(true)?;
            }
            socket.bind(&address.into())?;
            socket.listen(1024)?;
            return TcpListener::from_std(socket.into());
        }
        TcpListener::bind(address).await
    }

    pub fn destination(&self, stream: &TcpStream, listener: SocketAddr) -> io::Result<SocketAddr> {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "freebsd"
        ))]
        {
            let local = stream.local_addr()?;
            #[cfg(any(target_os = "linux", target_os = "android"))]
            let destination = if self.tproxy {
                // TPROXY preserves the destination rather than installing NAT state.
                local
            } else {
                let socket = socket2::SockRef::from(stream);
                let address = if local.is_ipv6() {
                    socket
                        .original_dst_v6()
                        .or_else(|_| socket.original_dst_v4())?
                } else {
                    socket.original_dst_v4()?
                };
                address
                    .as_socket()
                    .ok_or_else(|| invalid("kernel returned a non-IP destination"))?
            };
            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            let destination = pf::lookup(&self.pf, stream.peer_addr()?, local)?;
            validate_destination(destination, local, listener, self.tproxy)
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "freebsd"
        )))]
        {
            let _ = (stream, listener);
            Err(io::ErrorKind::Unsupported.into())
        }
    }
}

#[cfg(any(
    test,
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "freebsd"
))]
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}
#[cfg(any(
    test,
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "freebsd"
))]
fn validate_destination(
    destination: SocketAddr,
    local: SocketAddr,
    listener: SocketAddr,
    tproxy: bool,
) -> io::Result<SocketAddr> {
    let ip = destination.ip().to_canonical();
    if destination.port() == 0
        || ip.is_unspecified()
        || ip.is_multicast()
        || (!tproxy && destination == local)
        || (destination.port() == listener.port()
            && (listener.ip().is_unspecified() || listener.ip().to_canonical() == ip))
    {
        return Err(invalid(
            "not a redirected destination, or points back to the listener",
        ));
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_destination_refuses_normal_listener_connections_and_invalid_kernel_addresses() {
        let listener: SocketAddr = "0.0.0.0:15080".parse().unwrap();
        let local: SocketAddr = "127.0.0.1:15080".parse().unwrap();
        for destination in [
            local,
            "127.0.0.1:0".parse().unwrap(),
            "0.0.0.0:443".parse().unwrap(),
            "224.0.0.1:80".parse().unwrap(),
        ] {
            assert!(validate_destination(destination, local, listener, false).is_err());
        }
        let target: SocketAddr = "198.51.100.8:443".parse().unwrap();
        assert_eq!(
            validate_destination(target, local, listener, false).unwrap(),
            target
        );
        assert_eq!(
            validate_destination(target, target, listener, true).unwrap(),
            target
        );
        assert!(
            validate_destination("198.51.100.8:15080".parse().unwrap(), local, listener, true)
                .is_err()
        );
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn real_linux_socket_without_redirect_is_rejected() {
        let resolver = Resolver::new(false).unwrap();
        let listener = resolver.bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address);
        let (client, accepted) = tokio::join!(client, listener.accept());
        let _client = client.unwrap();
        assert!(resolver.destination(&accepted.unwrap().0, address).is_err());
    }
}
