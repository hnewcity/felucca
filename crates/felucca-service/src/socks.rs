//! Shared SOCKS5 parsing, authentication and datagram headers.
use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn reply(stream: &mut impl felucca_transport::Conn, code: u8) -> io::Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}

pub async fn target(
    stream: &mut impl felucca_transport::Conn,
    auth: &felucca_core::access::Authentication,
) -> io::Result<(u8, String)> {
    if stream.read_u8().await? != 5 {
        return Err(io::Error::other("not SOCKS5"));
    }
    let n = stream.read_u8().await?;
    let mut methods = vec![0; n as usize];
    stream.read_exact(&mut methods).await?;
    let method = if auth.required() { 2 } else { 0 };
    if !methods.contains(&method) {
        stream.write_all(&[5, 255]).await?;
        return Err(io::Error::other("no acceptable SOCKS5 method"));
    }
    stream.write_all(&[5, method]).await?;
    if method == 2 {
        if stream.read_u8().await? != 1 {
            stream.write_all(&[1, 1]).await?;
            return Err(io::Error::other("bad authentication version"));
        }
        let mut name = vec![0; stream.read_u8().await? as usize];
        stream.read_exact(&mut name).await?;
        let mut password = vec![0; stream.read_u8().await? as usize];
        stream.read_exact(&mut password).await?;
        let valid = !name.is_empty() && !password.is_empty() && auth.verify(&name, &password);
        stream.write_all(&[1, u8::from(!valid)]).await?;
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "authentication failed",
            ));
        }
    }
    let mut request = [0; 4];
    stream.read_exact(&mut request).await?;
    if request[0] != 5 || request[2] != 0 {
        reply(stream, 1).await?;
        return Err(io::Error::other("bad SOCKS request"));
    }
    if !matches!(request[1], 1 | 3) {
        reply(stream, 7).await?;
        return Err(io::Error::other("unsupported SOCKS5 command"));
    }
    let host = match request[3] {
        1 => {
            let mut addr = [0; 4];
            stream.read_exact(&mut addr).await?;
            Ipv4Addr::from(addr).to_string()
        }
        4 => {
            let mut addr = [0; 16];
            stream.read_exact(&mut addr).await?;
            format!("[{}]", Ipv6Addr::from(addr))
        }
        3 => {
            let n = stream.read_u8().await? as usize;
            let mut name = vec![0; n];
            stream.read_exact(&mut name).await?;
            let name = String::from_utf8(name).map_err(io::Error::other)?;
            if name.is_empty()
                || name
                    .bytes()
                    .any(|c| c <= 32 || c == b':' || c == b'/' || c == 127)
            {
                reply(stream, 8).await?;
                return Err(io::Error::other("bad target name"));
            }
            name
        }
        _ => {
            reply(stream, 8).await?;
            return Err(io::Error::other("bad address type"));
        }
    };
    let port = stream.read_u16().await?;
    if port == 0 && request[1] != 3 {
        reply(stream, 8).await?;
        return Err(io::Error::other("zero target port"));
    }
    Ok((request[1], format!("{host}:{port}")))
}

pub fn encode_address(address: SocketAddr, output: &mut Vec<u8>) {
    match address.ip() {
        std::net::IpAddr::V4(ip) => {
            output.push(1);
            output.extend(ip.octets());
        }
        std::net::IpAddr::V6(ip) => {
            output.push(4);
            output.extend(ip.octets());
        }
    }
    output.extend(address.port().to_be_bytes());
}

pub fn datagram_target(packet: &[u8]) -> Option<(String, usize)> {
    // Fragmentation is unsupported: RFC 1928 requires dropping nonzero FRAG.
    if packet.get(..3)? != [0, 0, 0] || packet.len() > felucca_proto::datagram::MAX_DATAGRAM {
        return None;
    }
    let (host, offset) = match packet.get(3)? {
        1 => (
            Ipv4Addr::from(<[u8; 4]>::try_from(packet.get(4..8)?).ok()?).to_string(),
            8,
        ),
        4 => (
            format!(
                "[{}]",
                Ipv6Addr::from(<[u8; 16]>::try_from(packet.get(4..20)?).ok()?)
            ),
            20,
        ),
        3 => {
            let n = *packet.get(4)? as usize;
            let host = std::str::from_utf8(packet.get(5..5 + n)?).ok()?.to_owned();
            (host, 5 + n)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(packet.get(offset..offset + 2)?.try_into().ok()?);
    let target = format!("{host}:{port}");
    felucca_core::config::validate_endpoint(&target).ok()?;
    Some((target, offset + 2))
}
