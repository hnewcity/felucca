//! HTTP/3 on native QUIC streams, sharing HTTP routing, policy and backend pools.
use crate::{http::BoxError, reverse::ReverseProxy};
use bytes::{Buf, Bytes};
use felucca_transport::{TransportError, proxy_protocol::Addresses};
use h3::error::Code;
use http::{HeaderMap, Request, Version, header};
use http_body_util::BodyExt;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub fn bind(proxy: &ReverseProxy, address: SocketAddr) -> Result<quinn::Endpoint, TransportError> {
    quinn::Endpoint::server(server_config(proxy.certificates.clone(), false)?, address).map_err(
        |source| TransportError::Bind {
            addr: address.to_string(),
            source,
        },
    )
}

pub(crate) fn server_config(
    certificates: Arc<dyn rustls::server::ResolvesServerCert>,
    bridge: bool,
) -> Result<quinn::ServerConfig, TransportError> {
    let mut tls = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_cert_resolver(certificates);
    tls.alpn_protocols = vec![b"h3".to_vec()];
    if bridge {
        // Keep this preference consistent with SharedCertificates::resolve.
        tls.alpn_protocols
            .insert(0, felucca_transport::quic::ALPN.to_vec());
    }
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|error| TransportError::tls(error.to_string()))?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_concurrent_bidi_streams(128_u32.into())
        .max_concurrent_uni_streams(16_u32.into())
        .stream_receive_window(1_048_576_u32.into())
        .receive_window(16_777_216_u32.into())
        .send_window(16_777_216)
        .max_idle_timeout(Some(
            Duration::from_secs(30)
                .try_into()
                .expect("30s idle timeout"),
        ));
    if bridge {
        transport.keep_alive_interval(Some(Duration::from_secs(10)));
    }
    config.transport_config(Arc::new(transport));
    Ok(config)
}

#[derive(Debug)]
pub(crate) struct SharedCertificates {
    pub bridge: Arc<dyn rustls::server::ResolvesServerCert>,
    pub http: Arc<dyn rustls::server::ResolvesServerCert>,
}
impl rustls::server::ResolvesServerCert for SharedCertificates {
    fn resolve(
        &self,
        hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        if hello.alpn().is_some_and(|mut protocols| {
            protocols.any(|name| name == felucca_transport::quic::ALPN)
        }) {
            self.bridge.resolve(hello)
        } else {
            self.http.resolve(hello)
        }
    }
}

pub(crate) async fn serve_connection(
    connection: quinn::Connection,
    proxy: Arc<ReverseProxy>,
    local: SocketAddr,
) {
    let sni = connection
        .handshake_data()
        .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
        .and_then(|data| data.server_name)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let destination = SocketAddr::new(connection.local_ip().unwrap_or(local.ip()), local.port());
    let Ok(mut h3) = h3::server::builder()
        .max_field_section_size(32768)
        .enable_extended_connect(true)
        .build(h3_quinn::Connection::new(connection.clone()))
        .await
    else {
        return;
    };
    let mut requests = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = requests.join_next(), if !requests.is_empty() => {},
            accepted = h3.accept() => {
                let Ok(Some(resolver)) = accepted else { break; };
                let proxy = proxy.clone();
                let sni = sni.clone();
                let addresses = Addresses { source: connection.remote_address(), destination };
                requests.spawn(async move {
                    let Ok(Ok((request, stream))) = tokio::time::timeout(
                        Duration::from_secs(15), resolver.resolve_request(),
                    ).await else { return; };
                    respond(proxy, request, stream, addresses, sni).await;
                });
            }
        }
    }
    // GOAWAY drains accepted requests; transport closure cancels idle bodies too.
    tokio::select! {
        _ = connection.closed() => requests.shutdown().await,
        () = async { while requests.join_next().await.is_some() {} } => {},
    }
}

pub async fn serve(endpoint: quinn::Endpoint, proxy: Arc<ReverseProxy>, cancel: CancellationToken) {
    let Ok(local) = endpoint.local_addr() else {
        return;
    };
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            _ = connections.join_next(), if !connections.is_empty() => {},
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break; };
                let proxy = proxy.clone();
                connections.spawn(async move {
                    let Ok(Ok(connection)) = tokio::time::timeout(Duration::from_secs(15), incoming).await else { return; };
                    serve_connection(connection, proxy, local).await;
                });
            }
        }
    }
    endpoint.close(0_u32.into(), b"shutdown");
    connections.shutdown().await;
}

fn content_length(headers: &HeaderMap) -> io::Result<Option<u64>> {
    if [
        "connection",
        "proxy-connection",
        "keep-alive",
        "upgrade",
        "transfer-encoding",
    ]
    .iter()
    .any(|name| headers.contains_key(*name))
        || headers.get_all(header::CONTENT_LENGTH).iter().count() > 1
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid HTTP/3 framing headers",
        ));
    }
    headers
        .get(header::CONTENT_LENGTH)
        .map(|value| {
            let text = value.to_str().map_err(io::Error::other)?;
            if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid Content-Length",
                ));
            }
            text.parse().map_err(io::Error::other)
        })
        .transpose()
}

async fn respond(
    proxy: Arc<ReverseProxy>,
    request: Request<()>,
    mut stream: h3::server::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
    addresses: Addresses,
    sni: String,
) {
    let Ok(length) = content_length(request.headers()) else {
        stream.stop_sending(Code::H3_MESSAGE_ERROR);
        stream.stop_stream(Code::H3_MESSAGE_ERROR);
        return;
    };
    let (mut send, receive) = stream.split();
    let body = RequestBody {
        stream: receive,
        length,
        read: 0,
        trailers: false,
        done: false,
    }
    .boxed_unsync();
    let request = request.map(|_| body);
    let response = proxy.request(request, addresses, Some(sni)).await;
    let (mut parts, mut body) = response.into_parts();
    parts.version = Version::HTTP_3;
    crate::http::strip_hop_headers(&mut parts.headers, false);
    if send
        .send_response(http::Response::from_parts(parts, ()))
        .await
        .is_err()
    {
        return;
    }
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            send.stop_stream(Code::H3_INTERNAL_ERROR);
            return;
        };
        let result = match frame.into_data() {
            Ok(data) => send.send_data(data).await,
            Err(frame) => match frame.into_trailers() {
                Ok(mut trailers) => {
                    crate::http::strip_hop_headers(&mut trailers, false);
                    send.send_trailers(trailers).await
                }
                Err(_) => continue,
            },
        };
        if result.is_err() {
            return;
        }
    }
    let _ = send.finish().await;
}

struct RequestBody {
    stream: h3::server::RequestStream<h3_quinn::RecvStream, Bytes>,
    length: Option<u64>,
    read: u64,
    trailers: bool,
    done: bool,
}
impl hyper::body::Body for RequestBody {
    type Data = Bytes;
    type Error = BoxError;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        if !this.trailers {
            match ready!(this.stream.poll_recv_data(cx)) {
                Ok(Some(mut data)) => {
                    this.read = this.read.saturating_add(data.remaining() as u64);
                    if this.length.is_some_and(|length| this.read > length) {
                        this.done = true;
                        this.stream.stop_sending(Code::H3_MESSAGE_ERROR);
                        return Poll::Ready(Some(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "HTTP/3 body exceeds Content-Length",
                        )
                        .into())));
                    }
                    return Poll::Ready(Some(Ok(hyper::body::Frame::data(
                        data.copy_to_bytes(data.remaining()),
                    ))));
                }
                Ok(None) => {
                    if this.length.is_some_and(|length| this.read != length) {
                        this.done = true;
                        return Poll::Ready(Some(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "HTTP/3 body shorter than Content-Length",
                        )
                        .into())));
                    }
                    this.trailers = true;
                }
                Err(error) => {
                    this.done = true;
                    return Poll::Ready(Some(Err(Box::new(error))));
                }
            }
        }
        let trailers = ready!(this.stream.poll_recv_trailers(cx));
        this.done = true;
        match trailers {
            Ok(Some(trailers))
                if ["content-length", "host", "transfer-encoding", "connection"]
                    .iter()
                    .any(|header| trailers.contains_key(*header)) =>
            {
                Poll::Ready(Some(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid HTTP/3 trailers",
                )
                .into())))
            }
            Ok(Some(trailers)) => Poll::Ready(Some(Ok(hyper::body::Frame::trailers(trailers)))),
            Ok(None) => Poll::Ready(None),
            Err(error) => Poll::Ready(Some(Err(Box::new(error)))),
        }
    }
    fn is_end_stream(&self) -> bool {
        self.done
    }
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.length
            .map_or_else(hyper::body::SizeHint::default, |length| {
                hyper::body::SizeHint::with_exact(length.saturating_sub(self.read))
            })
    }
}
impl Drop for RequestBody {
    fn drop(&mut self) {
        if !self.done {
            self.stream.stop_sending(Code::H3_REQUEST_CANCELLED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn http3_rejects_ambiguous_framing_before_opening_a_backend() {
        let mut headers = HeaderMap::new();
        assert_eq!(content_length(&headers).unwrap(), None);
        headers.insert(header::CONTENT_LENGTH, "123".parse().unwrap());
        assert_eq!(content_length(&headers).unwrap(), Some(123));
        headers.append(header::CONTENT_LENGTH, "123".parse().unwrap());
        assert!(content_length(&headers).is_err());
        for (name, value) in [
            ("content-length", "+1"),
            ("transfer-encoding", "chunked"),
            ("connection", "keep-alive"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
            assert!(content_length(&headers).is_err());
        }
    }
}
