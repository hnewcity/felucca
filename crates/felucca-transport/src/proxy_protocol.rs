//! HAProxy PROXY v1/v2, bounded parsing and explicit trust at the listener.
use felucca_core::{
    Acl,
    config::{ProxyAccept, ProxyMode, ProxyVersion},
};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use tokio::{io::AsyncReadExt, net::TcpStream};

const SIGNATURE: &[u8; 12] = b"\r\n\r\n\0\r\nQUIT\n";
const MAX_V2: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Addresses {
    pub source: SocketAddr,
    pub destination: SocketAddr,
}
#[derive(Debug, Clone)]
pub struct Acceptor {
    mode: ProxyMode,
    trusted: Acl,
}
impl Acceptor {
    pub fn new(config: &ProxyAccept) -> Result<Self, felucca_core::ConfigError> {
        Ok(Self {
            mode: config.mode,
            trusted: config.acl()?,
        })
    }
    /// Leave application bytes in the socket, so the following protocol driver
    /// sees exactly its own preface. A partial header has a single 5s deadline.
    pub async fn accept(&self, io: &mut TcpStream) -> io::Result<Addresses> {
        let actual = Addresses {
            source: io.peer_addr()?,
            destination: io.local_addr()?,
        };
        if self.mode == ProxyMode::Off {
            return Ok(actual);
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let kind = loop {
                let mut prefix = [0; 12];
                let n = io.peek(&mut prefix).await?;
                if n == 0 {
                    return Err(invalid("EOF before PROXY/application preface"));
                }
                let bytes = &prefix[..n];
                if bytes.starts_with(b"PROXY ") {
                    break Some(ProxyVersion::V1);
                }
                if bytes.starts_with(SIGNATURE) {
                    break Some(ProxyVersion::V2);
                }
                if !b"PROXY ".starts_with(bytes) && !SIGNATURE.starts_with(bytes) {
                    break None;
                }
                // peek remains readable while a fragment is buffered. A bounded
                // delay avoids spinning on that same fragment.
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            let Some(kind) = kind else {
                return if self.mode == ProxyMode::Required {
                    Err(invalid("required PROXY header missing"))
                } else {
                    Ok(actual)
                };
            };
            if !self.trusted.permits(actual.source.ip()) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "untrusted PROXY sender",
                ));
            }
            let mut header = Vec::new();
            if kind == ProxyVersion::V1 {
                loop {
                    header.push(io.read_u8().await?);
                    if header.ends_with(b"\r\n") {
                        break;
                    }
                    if header.len() >= 108 {
                        return Err(invalid("PROXY v1 header exceeds 108 bytes"));
                    }
                }
            } else {
                header.resize(16, 0);
                io.read_exact(&mut header).await?;
                let size = usize::from(u16::from_be_bytes([header[14], header[15]]));
                if size > MAX_V2 {
                    return Err(invalid("PROXY v2 header exceeds 4096 bytes"));
                }
                header.resize(16 + size, 0);
                io.read_exact(&mut header[16..]).await?;
                if header[12] == 0x21 && header[13] & 15 == 2 {
                    return Err(invalid("datagram PROXY header on a TCP listener"));
                }
            }
            decode(&header).map(|(addresses, _)| addresses.unwrap_or(actual))
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "PROXY header timed out"))?
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn encode(version: ProxyVersion, addresses: Addresses, datagram: bool) -> Vec<u8> {
    let source = addresses.source.ip().to_canonical();
    let destination = addresses.destination.ip().to_canonical();
    if version == ProxyVersion::Off {
        return vec![];
    }
    if version == ProxyVersion::V1 {
        let family = match (source, destination, datagram) {
            (IpAddr::V4(_), IpAddr::V4(_), false) => "TCP4",
            (IpAddr::V6(_), IpAddr::V6(_), false) => "TCP6",
            _ => return b"PROXY UNKNOWN\r\n".to_vec(),
        };
        return format!(
            "PROXY {family} {source} {destination} {} {}\r\n",
            addresses.source.port(),
            addresses.destination.port()
        )
        .into_bytes();
    }
    let mut out = SIGNATURE.to_vec();
    out.extend_from_slice(&[0x21, 0, 0, 0]);
    let family = match (source, destination) {
        (IpAddr::V4(source), IpAddr::V4(destination)) => {
            out.extend_from_slice(&source.octets());
            out.extend_from_slice(&destination.octets());
            0x10
        }
        (IpAddr::V6(source), IpAddr::V6(destination)) => {
            out.extend_from_slice(&source.octets());
            out.extend_from_slice(&destination.octets());
            0x20
        }
        _ => {
            out[12] = 0x20;
            return out;
        }
    };
    out[13] = family | if datagram { 2 } else { 1 };
    out.extend_from_slice(&addresses.source.port().to_be_bytes());
    out.extend_from_slice(&addresses.destination.port().to_be_bytes());
    let size = (out.len() - 16) as u16;
    out[14..16].copy_from_slice(&size.to_be_bytes());
    out
}

/// Parse one complete header, returning the consumed length. UNKNOWN/LOCAL
/// retains the real socket addresses rather than trusting unspecified fields.
pub fn decode(data: &[u8]) -> io::Result<(Option<Addresses>, usize)> {
    if data.starts_with(b"PROXY ") {
        let end = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| invalid("incomplete PROXY v1"))?
            + 2;
        if end > 108 {
            return Err(invalid("oversized PROXY v1"));
        }
        let text =
            std::str::from_utf8(&data[..end - 2]).map_err(|_| invalid("invalid PROXY v1 text"))?;
        let parts: Vec<_> = text.split(' ').collect();
        if parts.get(1) == Some(&"UNKNOWN") {
            return Ok((None, end));
        }
        if parts.len() != 6 {
            return Err(invalid("invalid PROXY v1 fields"));
        }
        let source: IpAddr = parts[2].parse().map_err(|_| invalid("invalid source IP"))?;
        let destination: IpAddr = parts[3]
            .parse()
            .map_err(|_| invalid("invalid destination IP"))?;
        if !matches!(
            (parts[1], source, destination),
            ("TCP4", IpAddr::V4(_), IpAddr::V4(_)) | ("TCP6", IpAddr::V6(_), IpAddr::V6(_))
        ) {
            return Err(invalid("PROXY address family mismatch"));
        }
        let source = SocketAddr::new(
            source,
            parts[4]
                .parse()
                .map_err(|_| invalid("invalid source port"))?,
        );
        let destination = SocketAddr::new(
            destination,
            parts[5]
                .parse()
                .map_err(|_| invalid("invalid destination port"))?,
        );
        return Ok((
            Some(Addresses {
                source,
                destination,
            }),
            end,
        ));
    }
    if data.len() < 16 || &data[..12] != SIGNATURE || data[12] >> 4 != 2 {
        return Err(invalid("invalid PROXY v2 signature/version"));
    }
    let size = usize::from(u16::from_be_bytes([data[14], data[15]]));
    if size > MAX_V2 || data.len() < 16 + size {
        return Err(invalid("invalid PROXY v2 length"));
    }
    let end = 16 + size;
    if data[12] & 15 == 0 {
        return Ok((None, end));
    }
    if data[12] & 15 != 1 {
        return Err(invalid("invalid PROXY v2 command"));
    }
    if data[13] == 0 {
        return Ok((None, end));
    }
    let (source, destination, offset) = match data[13] {
        0x11 | 0x12 if size >= 12 => (
            IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&data[16..20]).unwrap())),
            IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&data[20..24]).unwrap())),
            24,
        ),
        0x21 | 0x22 if size >= 36 => (
            IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&data[16..32]).unwrap())),
            IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&data[32..48]).unwrap())),
            48,
        ),
        _ => return Err(invalid("unsupported PROXY v2 family/protocol")),
    };
    let source = SocketAddr::new(source, u16::from_be_bytes([data[offset], data[offset + 1]]));
    let destination = SocketAddr::new(
        destination,
        u16::from_be_bytes([data[offset + 2], data[offset + 3]]),
    );
    let mut cursor = offset + 4;
    while cursor < end {
        if end - cursor < 3 {
            return Err(invalid("truncated PROXY v2 TLV"));
        }
        let len = usize::from(u16::from_be_bytes([data[cursor + 1], data[cursor + 2]]));
        cursor += 3 + len;
        if cursor > end {
            return Err(invalid("truncated PROXY v2 TLV value"));
        }
    }
    Ok((
        Some(Addresses {
            source,
            destination,
        }),
        end,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn addresses() -> Addresses {
        Addresses {
            source: "192.0.2.1:1234".parse().unwrap(),
            destination: "198.51.100.2:443".parse().unwrap(),
        }
    }
    #[test]
    fn published_wire_layouts_and_ipv6_are_preserved() {
        let v1 = b"PROXY TCP4 192.0.2.1 198.51.100.2 1234 443\r\n";
        let v2 =
            b"\r\n\r\n\0\r\nQUIT\n\x21\x11\0\x0c\xc0\0\x02\x01\xc6\x33\x64\x02\x04\xd2\x01\xbb";
        assert_eq!(encode(ProxyVersion::V1, addresses(), false), v1);
        assert_eq!(encode(ProxyVersion::V2, addresses(), false), v2);
        assert_eq!(decode(v2).unwrap(), (Some(addresses()), v2.len()));
        for version in [ProxyVersion::V1, ProxyVersion::V2] {
            let v6 = Addresses {
                source: "[2001:db8::1]:1234".parse().unwrap(),
                destination: "[2001:db8::2]:443".parse().unwrap(),
            };
            assert_eq!(decode(&encode(version, v6, false)).unwrap().0, Some(v6));
        }
        let mut tlv = v2.to_vec();
        tlv[15] += 5;
        tlv.extend_from_slice(&[0x04, 0, 2, 0, 0]);
        assert_eq!(decode(&tlv).unwrap(), (Some(addresses()), tlv.len()));
        let mut local = v2.to_vec();
        local[12] = 0x20;
        assert_eq!(decode(&local).unwrap().0, None);
        assert_eq!(decode(b"PROXY UNKNOWN\r\npayload").unwrap(), (None, 15));
    }
    #[test]
    fn truncated_headers_bad_families_and_malformed_tlvs_are_rejected() {
        let good = encode(ProxyVersion::V2, addresses(), false);
        for size in 0..good.len() {
            assert!(decode(&good[..size]).is_err());
        }
        for bad in [
            b"PROXY TCP4 ::1 127.0.0.1 1 2\r\n".as_slice(),
            b"PROXY TCP6 ::1 ::2 65536 2\r\n",
            b"PROXY TCP4 1.1.1.1 2.2.2.2 1\r\n",
        ] {
            assert!(decode(bad).is_err());
        }
        let mut malformed = good;
        malformed[15] += 3;
        malformed.extend_from_slice(&[1, 0, 99]);
        assert!(decode(&malformed).is_err());
    }
    #[tokio::test]
    async fn fragmented_headers_leave_all_application_bytes_and_preserve_real_unknown_peers() {
        for version in [ProxyVersion::V1, ProxyVersion::V2] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut client = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (mut server, _) = listener.accept().await.unwrap();
            let acceptor = Acceptor::new(&ProxyAccept {
                mode: ProxyMode::Required,
                trusted: vec!["127.0.0.1".into()],
            })
            .unwrap();
            let receiver = tokio::spawn(async move {
                let metadata = acceptor.accept(&mut server).await.unwrap();
                assert_eq!(metadata, addresses());
                let mut payload = [0; 7];
                server.read_exact(&mut payload).await.unwrap();
                assert_eq!(&payload, b"payload");
            });
            for byte in encode(version, addresses(), false) {
                client.write_all(&[byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
            client.write_all(b"payload").await.unwrap();
            receiver.await.unwrap();
        }
    }
    #[tokio::test]
    async fn optional_post_is_untouched_but_untrusted_and_missing_required_headers_fail() {
        for (mode, trusted, payload, succeeds) in [
            (
                ProxyMode::Optional,
                "127.0.0.1",
                b"POST / HTTP/1.1\r\n\r\n".as_slice(),
                true,
            ),
            (
                ProxyMode::Optional,
                "192.0.2.1",
                b"PROXY UNKNOWN\r\n",
                false,
            ),
            (
                ProxyMode::Required,
                "127.0.0.1",
                b"GET / HTTP/1.1\r\n\r\n",
                false,
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut client = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (mut server, real) = listener.accept().await.unwrap();
            client.write_all(payload).await.unwrap();
            let acceptor = Acceptor::new(&ProxyAccept {
                mode,
                trusted: vec![trusted.into()],
            })
            .unwrap();
            let result = acceptor.accept(&mut server).await;
            assert_eq!(result.is_ok(), succeeds);
            if succeeds {
                assert_eq!(result.unwrap().source, real);
                let mut got = vec![0; payload.len()];
                server.read_exact(&mut got).await.unwrap();
                assert_eq!(got, payload);
            }
        }
    }
}
