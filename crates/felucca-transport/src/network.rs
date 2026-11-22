//! Per-agent resolution and egress. The proxy is used only for bridge sockets.
use crate::{PrefixedConn, TransportError};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use felucca_core::config::network::{NetworkConfig, ProxyKind, UpstreamProxy};
use hickory_resolver::{
    TokioResolver,
    config::{LookupIpStrategy, NameServerConfig, ResolverConfig},
    net::runtime::TokioRuntimeProvider,
};
use std::{
    io,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpSocket, TcpStream},
};

/// How long a resolved address may take to connect while another remains.
pub const ADDRESS_ATTEMPT: Duration = Duration::from_secs(3);

#[derive(Clone, Default)]
pub struct Network {
    config: NetworkConfig,
    resolver: Option<TokioResolver>,
}
impl std::fmt::Debug for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Network")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Network {
    pub fn new(config: &NetworkConfig, timeout: Duration) -> Result<Self, TransportError> {
        config
            .validate()
            .map_err(|error| TransportError::Io(io::Error::other(error)))?;
        let resolver = if config.dns.is_empty() {
            None
        } else {
            let servers = config
                .dns
                .iter()
                .map(|address| {
                    let mut server = NameServerConfig::udp_and_tcp(address.ip());
                    for connection in &mut server.connections {
                        connection.port = address.port();
                        connection.bind_addr = config.local_ip.map(|ip| SocketAddr::new(ip, 0));
                    }
                    server
                })
                .collect();
            let mut builder = TokioResolver::builder_with_config(
                ResolverConfig::from_name_servers(servers),
                TokioRuntimeProvider::default(),
            );
            let options = builder.options_mut();
            options.timeout = timeout.min(Duration::from_secs(5));
            options.cache_size = 256;
            options.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
            options.try_tcp_on_error = true;
            Some(
                builder
                    .build()
                    .map_err(|error| TransportError::Io(io::Error::other(error)))?,
            )
        };
        Ok(Self {
            config: config.clone(),
            resolver,
        })
    }
    pub fn local_ip(&self) -> Option<IpAddr> {
        self.config.local_ip
    }
    pub fn backend_local_ip(&self) -> Option<IpAddr> {
        self.config
            .forward_local_ip
            .then_some(self.config.local_ip)
            .flatten()
    }

    /// Resolve once; callers apply ACLs and connect to these exact addresses.
    pub async fn resolve(&self, endpoint: &str) -> io::Result<Vec<SocketAddr>> {
        felucca_core::config::validate_endpoint(endpoint).map_err(io::Error::other)?;
        if let Ok(address) = endpoint.parse() {
            return Ok(vec![address]);
        }
        let (host, port) = endpoint.rsplit_once(':').expect("validated endpoint");
        let port = port.parse::<u16>().expect("validated port");
        let mut addresses = Vec::new();
        if let Some(resolver) = &self.resolver {
            let lookup = resolver.lookup_ip(host).await.map_err(io::Error::other)?;
            for ip in lookup.iter().take(64) {
                let address = SocketAddr::new(ip, port);
                if !addresses.contains(&address) {
                    addresses.push(address);
                }
            }
        } else {
            addresses.extend(tokio::net::lookup_host(endpoint).await?.take(64));
            addresses.dedup();
        }
        if addresses.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no resolved addresses",
            ));
        }
        Ok(addresses)
    }

    pub async fn connect_to(
        address: SocketAddr,
        local_ip: Option<IpAddr>,
    ) -> io::Result<TcpStream> {
        if local_ip.is_some_and(|local| local.is_ipv4() != address.is_ipv4()) {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "destination and local_ip have different address families",
            ));
        }
        let socket = if address.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        if let Some(ip) = local_ip {
            socket.bind(SocketAddr::new(ip, 0))?;
        }
        let io = socket.connect(address).await?;
        crate::tcp::tune(&io);
        Ok(io)
    }

    /// Connects to one of several resolved addresses, bounding all but the last.
    ///
    /// A silently dropped address (broken IPv6 on a Wi-Fi network, one dead node
    /// in a round-robin set) only fails after the OS connect timeout of 75 to
    /// 127 s, far past any caller's dial deadline, so the working address after
    /// it would never be tried. Give every address but the last
    /// [`ADDRESS_ATTEMPT`]; the last keeps whatever the caller's deadline leaves.
    pub async fn connect_candidate(
        address: SocketAddr,
        local_ip: Option<IpAddr>,
        last: bool,
    ) -> io::Result<TcpStream> {
        let attempt = Self::connect_to(address, local_ip);
        if last {
            return attempt.await;
        }
        tokio::time::timeout(ADDRESS_ATTEMPT, attempt)
            .await
            .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
    }

    async fn direct(&self, endpoint: &str) -> io::Result<TcpStream> {
        let mut error = io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no source-compatible destination",
        );
        let addresses = self.resolve(endpoint).await?;
        let count = addresses.len();
        for (index, address) in addresses.into_iter().enumerate() {
            match Self::connect_candidate(address, self.config.local_ip, index + 1 == count).await {
                Ok(io) => return Ok(io),
                Err(last) => error = last,
            }
        }
        Err(error)
    }

    pub async fn bridge_tcp(&self, endpoint: &str) -> io::Result<PrefixedConn<TcpStream>> {
        felucca_core::config::validate_endpoint(endpoint).map_err(io::Error::other)?;
        let Some(proxy) = &self.config.proxy else {
            return Ok(PrefixedConn::new(
                Bytes::new(),
                self.direct(endpoint).await?,
            ));
        };
        let io = self.direct(&proxy.server).await?;
        let destination = if proxy.remote_dns {
            let (host, port) = endpoint.rsplit_once(':').expect("validated endpoint");
            if endpoint.parse::<SocketAddr>().is_ok() {
                endpoint.to_owned()
            } else {
                let name =
                    hickory_resolver::proto::rr::Name::from_utf8(host).map_err(io::Error::other)?;
                format!("{}:{port}", name.to_ascii().trim_end_matches('.'))
            }
        } else {
            self.resolve(endpoint)
                .await?
                .into_iter()
                .next()
                .expect("nonempty")
                .to_string()
        };
        match proxy.kind {
            ProxyKind::Http => http_connect(io, &destination, proxy).await,
            ProxyKind::Socks5 => socks_connect(io, &destination, proxy).await,
        }
    }
}

fn rejected(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

async fn http_connect(
    mut io: TcpStream,
    endpoint: &str,
    proxy: &UpstreamProxy,
) -> io::Result<PrefixedConn<TcpStream>> {
    let mut request = format!("CONNECT {endpoint} HTTP/1.1\r\nHost: {endpoint}\r\n");
    if let Some(auth) = &proxy.auth {
        let encoded = STANDARD.encode(format!("{}:{}", auth.username, auth.password.expose()));
        request.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    request.push_str("\r\n");
    io.write_all(request.as_bytes()).await?;
    let mut reader = BufReader::new(io);
    // Accommodate bounded informational responses without consuming tunnel bytes.
    for _ in 0..5 {
        let mut head = Vec::new();
        loop {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "proxy closed before CONNECT response",
                ));
            }
            let take = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |i| i + 1);
            if head.len() + take > 32768 {
                return Err(invalid("proxy response headers exceed 32 KiB"));
            }
            head.extend_from_slice(&available[..take]);
            reader.consume(take);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let mut headers = [httparse::EMPTY_HEADER; 128];
        let mut response = httparse::Response::new(&mut headers);
        if !response
            .parse(&head)
            .is_ok_and(|status| status.is_complete())
        {
            return Err(invalid("invalid upstream HTTP response"));
        }
        let code = response
            .code
            .ok_or_else(|| invalid("missing proxy status"))?;
        if (100..200).contains(&code) && code != 101 {
            continue;
        }
        if !(200..300).contains(&code) {
            return Err(rejected("upstream proxy refused CONNECT"));
        }
        let prefix = Bytes::copy_from_slice(reader.buffer());
        return Ok(PrefixedConn::new(prefix, reader.into_inner()));
    }
    Err(invalid("too many proxy informational responses"))
}

async fn socks_connect(
    mut io: TcpStream,
    endpoint: &str,
    proxy: &UpstreamProxy,
) -> io::Result<PrefixedConn<TcpStream>> {
    let method = if proxy.auth.is_some() { 2 } else { 0 };
    io.write_all(&[5, 1, method]).await?;
    let mut selected = [0; 2];
    io.read_exact(&mut selected).await?;
    if selected != [5, method] {
        return Err(rejected("upstream SOCKS authentication method refused"));
    }
    if let Some(auth) = &proxy.auth {
        let mut request = vec![1, auth.username.len() as u8];
        request.extend_from_slice(auth.username.as_bytes());
        request.push(auth.password.len() as u8);
        request.extend_from_slice(auth.password.as_bytes());
        io.write_all(&request).await?;
        io.read_exact(&mut selected).await?;
        if selected != [1, 0] {
            return Err(rejected("upstream SOCKS credentials refused"));
        }
    }
    let mut request = vec![5, 1, 0];
    if let Ok(address) = endpoint.parse::<SocketAddr>() {
        match address.ip() {
            IpAddr::V4(ip) => {
                request.push(1);
                request.extend_from_slice(&ip.octets());
            }
            IpAddr::V6(ip) => {
                request.push(4);
                request.extend_from_slice(&ip.octets());
            }
        }
        request.extend_from_slice(&address.port().to_be_bytes());
    } else {
        let (host, port) = endpoint.rsplit_once(':').expect("validated endpoint");
        if host.is_empty() || host.len() > 255 {
            return Err(invalid("SOCKS target hostname exceeds 255 bytes"));
        }
        request.extend_from_slice(&[3, host.len() as u8]);
        request.extend_from_slice(host.as_bytes());
        request.extend_from_slice(&port.parse::<u16>().expect("validated port").to_be_bytes());
    }
    io.write_all(&request).await?;
    let mut response = [0; 4];
    io.read_exact(&mut response).await?;
    if response[0] != 5 || response[2] != 0 {
        return Err(invalid("invalid SOCKS CONNECT response"));
    }
    if response[1] != 0 {
        return Err(rejected("upstream SOCKS proxy refused CONNECT"));
    }
    let size = match response[3] {
        1 => 6,
        4 => 18,
        3 => {
            let size = io.read_u8().await?;
            if size == 0 {
                return Err(invalid("empty SOCKS bound address"));
            }
            usize::from(size) + 2
        }
        _ => return Err(invalid("invalid SOCKS bound address type")),
    };
    io.read_exact(&mut vec![0; size]).await?;
    Ok(PrefixedConn::new(Bytes::new(), io))
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
