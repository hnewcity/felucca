//! Bounded classification for a socket shared by bridge and public HTTP.
use bytes::Bytes;
use felucca_transport::{PrefixedConn, TransportError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

/// Only a real WebSocket upgrade at the reserved bridge path goes to the
/// bridge. Preserve all read bytes, including any pipelined request body.
pub async fn classify_get<T>(
    mut io: T,
    path: &str,
) -> Result<(bool, PrefixedConn<T>), TransportError>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut prefix = Vec::new();
    let bridge = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let mut headers = [httparse::EMPTY_HEADER; 128];
            let mut request = httparse::Request::new(&mut headers);
            match request.parse(&prefix) {
                Ok(httparse::Status::Complete(_)) => {
                    let upgrade = request.headers.iter().any(|header| {
                        header.name.eq_ignore_ascii_case("upgrade")
                            && header.value.eq_ignore_ascii_case(b"websocket")
                    });
                    let connection = request.headers.iter().any(|header| {
                        header.name.eq_ignore_ascii_case("connection")
                            && header
                                .value
                                .split(|b| *b == b',')
                                .any(|part| part.trim_ascii().eq_ignore_ascii_case(b"upgrade"))
                    });
                    let target = request.path.unwrap_or("").split('?').next().unwrap_or("");
                    return Ok(upgrade
                        && connection
                        && request.method == Some("GET")
                        && target == path);
                }
                Err(_) => return Err(TransportError::UnknownProtocol),
                Ok(httparse::Status::Partial) => {}
            }
            if prefix.len() >= 32768 {
                return Err(TransportError::UnknownProtocol);
            }
            let mut chunk = [0; 2048];
            let n = io.read(&mut chunk).await?;
            if n == 0 {
                return Err(TransportError::UnknownProtocol);
            }
            prefix.extend_from_slice(&chunk[..n]);
        }
    })
    .await
    .map_err(|_| TransportError::Timeout)??;
    Ok((bridge, PrefixedConn::new(Bytes::from(prefix), io)))
}
