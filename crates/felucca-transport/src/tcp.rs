//! Plain TCP, and the socket tuning every TCP-based transport wants.

use std::{net::SocketAddr, time::Duration};

use tokio::net::{TcpListener, TcpStream};

use crate::error::TransportError;

mod accept;
pub use accept::Acceptor;

/// Idle time before the kernel probes a silent peer, then the probe spacing and
/// count: a peer that vanished without FIN or RST is found within two minutes.
pub const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub const KEEPALIVE_RETRIES: u32 = 4;

/// Applies the socket options a proxy needs.
///
/// `TCP_NODELAY` is the one that matters: Nagle's algorithm holds a small write
/// back waiting for more data, which on a forwarded SSH or database session adds
/// a round trip to every interaction. A proxy is exactly the case Nagle hurts,
/// since it has no idea when the application's next byte is coming.
///
/// Keepalive bounds how long an idle connection whose peer lost power, roamed or
/// sat behind an expired NAT mapping holds its descriptor, task and connection
/// quota. Without it such a relay waits for a byte that never comes, and over
/// months the leaks exhaust descriptors or `max_connections`. A live idle peer
/// answers the probes, so long-lived quiet sessions such as SSH are unaffected.
pub fn tune(stream: &TcpStream) {
    // Failure here is not worth refusing the connection over: the peer is
    // already connected and the tunnel works either way, just less crisply.
    if let Err(e) = stream.set_nodelay(true) {
        tracing::debug!(error = %e, "could not set TCP_NODELAY");
    }
    if let Err(e) = socket2::SockRef::from(stream).set_tcp_keepalive(&keepalive()) {
        tracing::debug!(error = %e, "could not enable TCP keepalive");
    }
}

fn keepalive() -> socket2::TcpKeepalive {
    probes(socket2::TcpKeepalive::new().with_time(KEEPALIVE_IDLE))
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "windows"
))]
fn probes(keepalive: socket2::TcpKeepalive) -> socket2::TcpKeepalive {
    keepalive
        .with_interval(KEEPALIVE_INTERVAL)
        .with_retries(KEEPALIVE_RETRIES)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "windows"
)))]
fn probes(keepalive: socket2::TcpKeepalive) -> socket2::TcpKeepalive {
    keepalive
}

/// Binds a TCP listener.
///
/// An unspecified IPv6 address (`[::]`) means every interface, IPv4 included.
/// Linux and macOS accept IPv4 on such a socket by default; Windows and some
/// BSDs default to `IPV6_V6ONLY`, so it is cleared explicitly.
pub async fn bind(addr: SocketAddr) -> Result<TcpListener, TransportError> {
    listen(addr).map_err(|source| TransportError::Bind {
        addr: addr.to_string(),
        source,
    })
}

/// [`bind`] without the error wrapping, for callers that report `io::Error`.
pub fn listen(addr: SocketAddr) -> std::io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if crate::udp::dual_stack(addr) {
        socket.set_only_v6(false)?;
    }
    // Match std/tokio: reuse a TIME_WAIT port on Unix. Windows SO_REUSEADDR
    // would allow two live listeners on one port, so it is left unset there.
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    TcpListener::from_std(socket.into())
}

/// Accepts one connection, tuned and ready to use.
pub async fn accept(listener: &TcpListener) -> Result<(TcpStream, SocketAddr), TransportError> {
    let (stream, peer) = listener.accept().await?;
    tune(&stream);
    Ok((stream, peer))
}

/// Connects to `addr`.
///
/// Takes the address as a string so a hostname works: an agent's config names a
/// server by domain far more often than by IP, and resolution belongs here
/// rather than in every caller.
pub async fn connect(addr: &str) -> Result<TcpStream, TransportError> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|source| TransportError::Connect {
            addr: addr.to_owned(),
            source,
        })?;
    tune(&stream);
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn loopback() -> TcpListener {
        bind("127.0.0.1:0".parse().unwrap()).await.unwrap()
    }

    /// A tunnel without a bind host listens on `[::]` and must still accept
    /// IPv4, including on Windows where IPV6_V6ONLY defaults to on.
    #[tokio::test]
    async fn an_ipv6_wildcard_listener_accepts_ipv4_and_udp_does_too() {
        let Ok(listener) = bind("[::]:0".parse().unwrap()).await else {
            return; // IPv6 disabled on this host.
        };
        let port = listener.local_addr().unwrap().port();
        let (accepted, connected) =
            tokio::join!(listener.accept(), TcpStream::connect(("127.0.0.1", port)));
        accepted.unwrap();
        connected.unwrap();

        let socket = crate::udp::bind_plain("[::]:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender.send_to(b"v4", ("127.0.0.1", port)).await.unwrap();
        let mut buffer = [0; 2];
        let (len, _) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer[..len], b"v4");
    }

    #[tokio::test]
    async fn a_connection_round_trips_bytes() {
        let listener = loopback().await;
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut s, _) = accept(&listener).await.unwrap();
            let mut got = [0u8; 5];
            s.read_exact(&mut got).await.unwrap();
            s.write_all(b"pong!").await.unwrap();
            got
        });

        let mut c = connect(&addr.to_string()).await.unwrap();
        c.write_all(b"ping!").await.unwrap();
        let mut reply = [0u8; 5];
        c.read_exact(&mut reply).await.unwrap();

        assert_eq!(&reply, b"pong!");
        assert_eq!(&server.await.unwrap(), b"ping!");
    }

    #[tokio::test]
    async fn accepted_and_dialled_sockets_are_both_tuned() {
        let listener = loopback().await;
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move { accept(&listener).await.unwrap().0 });
        let client = connect(&addr.to_string()).await.unwrap();
        let accepted = server.await.unwrap();

        assert!(client.nodelay().unwrap(), "a dialled socket needs nodelay");
        assert!(
            accepted.nodelay().unwrap(),
            "an accepted socket needs nodelay too"
        );
        for stream in [&client, &accepted] {
            let socket = socket2::SockRef::from(stream);
            assert!(socket.keepalive().unwrap(), "a silent peer must be probed");
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            assert_eq!(socket.tcp_keepalive_time().unwrap(), KEEPALIVE_IDLE);
        }
    }

    #[tokio::test]
    async fn a_hostname_is_resolved() {
        let listener = loopback().await;
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { accept(&listener).await.map(|(s, _)| s) });

        // An agent's config names its server by domain far more often than by IP.
        let c = connect(&format!("localhost:{port}")).await;
        assert!(c.is_ok(), "localhost should resolve: {:?}", c.err());
        let _ = server.await;
    }

    #[tokio::test]
    async fn a_refused_connection_names_the_address() {
        // Bind and drop, so the port is almost certainly free.
        let addr = loopback().await.local_addr().unwrap().to_string();
        let err = connect(&addr).await.expect_err("nothing is listening");

        assert!(matches!(err, TransportError::Connect { .. }), "{err}");
        assert!(
            err.to_string().contains(&addr),
            "the error should name the target: {err}"
        );
    }

    #[tokio::test]
    async fn an_unresolvable_host_fails_as_a_connect_error() {
        let err = connect("no-such-host.invalid:9")
            .await
            .expect_err("must not resolve");
        assert!(matches!(err, TransportError::Connect { .. }), "{err}");
    }

    #[tokio::test]
    async fn binding_a_taken_port_reports_a_bind_error() {
        let held = loopback().await;
        let addr = held.local_addr().unwrap();

        let err = bind(addr).await.expect_err("the port is already held");
        assert!(matches!(err, TransportError::Bind { .. }), "{err}");
        assert!(!err.is_routine(), "an operator needs to see a bind failure");
    }
}
