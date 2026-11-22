//! RFC 6455 / 8441 / 9220 handshakes over the existing metered backend pool.
use crate::http::{Body, BoxError, incoming, response, strip_hop_headers, wants_upgrade};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use http::{HeaderMap, Method, Request, Response, StatusCode, Version, header};
use http_body_util::BodyExt;
use hyper::{body::Incoming, upgrade::OnUpgrade};
use hyper_util::rt::TokioIo;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

enum Downstream {
    Upgrade(OnUpgrade),
    Http3(Body),
}

pub(crate) struct Handshake {
    downstream: Downstream,
    // Only HTTP/1 uses the key/accept challenge.
    downstream_accept: Option<String>,
    backend_accept: Option<String>,
}

pub(crate) fn extended(request: &Request<impl Sized>) -> Option<&str> {
    request
        .extensions()
        .get::<hyper::ext::Protocol>()
        .map(|p| p.as_str())
        .or_else(|| {
            request
                .extensions()
                .get::<h3::ext::Protocol>()
                .map(|p| p.as_str())
        })
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

pub(crate) fn prepare(
    request: &mut Request<Body>,
    backend_http2: bool,
) -> Result<Option<Handshake>, (StatusCode, &'static str)> {
    let protocol = extended(request);
    let is_extended = protocol.is_some();
    if protocol.is_some_and(|p| p != "websocket") {
        return Err((StatusCode::NOT_IMPLEMENTED, "unsupported CONNECT protocol"));
    }
    let is_h1 = wants_upgrade(request.headers())
        && single(request.headers(), "upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if !is_extended && !is_h1 {
        // Plain CONNECT is supported by the forward proxy, not site routes.
        if request.method() == Method::CONNECT {
            return Err((StatusCode::BAD_REQUEST, "site CONNECT requires :protocol"));
        }
        return Ok(None);
    }
    let bad = || (StatusCode::BAD_REQUEST, "invalid WebSocket handshake");
    if single(request.headers(), "sec-websocket-version") != Some("13")
        || request.headers().contains_key("sec-websocket-accept")
        || request
            .headers()
            .get_all(header::CONTENT_LENGTH)
            .iter()
            .count()
            > 1
        || request
            .headers()
            .get(header::CONTENT_LENGTH)
            .is_some_and(|v| v != "0")
    {
        return Err(bad());
    }
    let downstream_accept = if is_extended {
        if request.method() != Method::CONNECT
            || !matches!(request.version(), Version::HTTP_2 | Version::HTTP_3)
            || request.headers().contains_key("sec-websocket-key")
            || request.headers().contains_key(header::UPGRADE)
            || request.headers().contains_key(header::CONNECTION)
        {
            return Err(bad());
        }
        None
    } else {
        if request.method() != Method::GET || request.version() != Version::HTTP_11 {
            return Err(bad());
        }
        let key = single(request.headers(), "sec-websocket-key").ok_or_else(bad)?;
        if !STANDARD
            .decode(key)
            .is_ok_and(|decoded| decoded.len() == 16)
        {
            return Err(bad());
        }
        Some(felucca_transport::ws::accept_key(key.as_bytes()))
    };
    let downstream = if request.version() == Version::HTTP_3 {
        Downstream::Http3(std::mem::replace(
            request.body_mut(),
            response(StatusCode::OK, "").into_body(),
        ))
    } else {
        let upgrade = hyper::upgrade::on(&mut *request);
        *request.body_mut() = response(StatusCode::OK, "").into_body();
        Downstream::Upgrade(upgrade)
    };
    request.extensions_mut().remove::<hyper::ext::Protocol>();
    request.extensions_mut().remove::<h3::ext::Protocol>();
    strip_hop_headers(request.headers_mut(), false);
    request.headers_mut().remove(header::CONTENT_LENGTH);
    let backend_accept = if backend_http2 {
        *request.method_mut() = Method::CONNECT;
        request
            .extensions_mut()
            .insert(hyper::ext::Protocol::from_static("websocket"));
        request.headers_mut().remove("sec-websocket-key");
        None
    } else {
        *request.method_mut() = Method::GET;
        let key = STANDARD.encode(&felucca_proto::handshake::generate_nonce()[..16]);
        let accept = felucca_transport::ws::accept_key(key.as_bytes());
        request
            .headers_mut()
            .insert("sec-websocket-key", key.parse().expect("base64"));
        request
            .headers_mut()
            .insert(header::CONNECTION, "upgrade".parse().unwrap());
        request
            .headers_mut()
            .insert(header::UPGRADE, "websocket".parse().unwrap());
        Some(accept)
    };
    Ok(Some(Handshake {
        downstream,
        downstream_accept,
        backend_accept,
    }))
}

impl Handshake {
    pub(crate) fn finish(
        self,
        mut upstream: Response<Incoming>,
        cancel: CancellationToken,
        timeout: Duration,
    ) -> Response<Body> {
        let established = if self.backend_accept.is_some() {
            upstream.status() == StatusCode::SWITCHING_PROTOCOLS
        } else {
            upstream.status().is_success()
        };
        if !established {
            if upstream.status() == StatusCode::SWITCHING_PROTOCOLS {
                return response(
                    StatusCode::BAD_GATEWAY,
                    "invalid WebSocket backend response",
                );
            }
            strip_hop_headers(upstream.headers_mut(), false);
            return upstream.map(incoming);
        }
        if let Some(expected) = &self.backend_accept {
            if !wants_upgrade(upstream.headers())
                || !single(upstream.headers(), "upgrade")
                    .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
                || single(upstream.headers(), "sec-websocket-accept") != Some(expected.as_str())
            {
                return response(
                    StatusCode::BAD_GATEWAY,
                    "invalid WebSocket backend handshake",
                );
            }
        } else if upstream.headers().contains_key("sec-websocket-accept") {
            return response(
                StatusCode::BAD_GATEWAY,
                "invalid extended WebSocket handshake",
            );
        }
        let backend = hyper::upgrade::on(&mut upstream);
        let (mut parts, _) = upstream.into_parts();
        strip_hop_headers(&mut parts.headers, false);
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.headers.remove("sec-websocket-accept");
        parts.status = if let Some(accept) = self.downstream_accept {
            parts
                .headers
                .insert(header::CONNECTION, "upgrade".parse().unwrap());
            parts
                .headers
                .insert(header::UPGRADE, "websocket".parse().unwrap());
            parts
                .headers
                .insert("sec-websocket-accept", accept.parse().expect("base64"));
            StatusCode::SWITCHING_PROTOCOLS
        } else {
            StatusCode::OK
        };
        let body = match self.downstream {
            Downstream::Upgrade(downstream) => {
                tokio::spawn(async move {
                    tokio::select! {
                        () = cancel.cancelled() => {},
                        _ = async {
                            if let Ok((Ok(a), Ok(b))) = tokio::time::timeout(timeout, async {
                                tokio::join!(downstream, backend)
                            }).await {
                                let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(a), &mut TokioIo::new(b)).await;
                            }
                        } => {},
                    }
                });
                response(StatusCode::OK, "").into_body()
            }
            Downstream::Http3(mut input) => {
                // Four bounded data chunks propagate QUIC backpressure to the backend.
                let (tx, rx) = tokio::sync::mpsc::channel(4);
                let stopped = cancel.child_token();
                let task_cancel = stopped.clone();
                tokio::spawn(async move {
                    let relay = async {
                        let backend = tokio::time::timeout(timeout, backend)
                            .await
                            .map_err(io::Error::other)?
                            .map_err(io::Error::other)?;
                        let (mut reader, mut writer) = tokio::io::split(TokioIo::new(backend));
                        let upload = async {
                            while let Some(frame) = input.frame().await {
                                let frame = frame.map_err(io::Error::other)?;
                                if let Ok(data) = frame.into_data() {
                                    writer.write_all(&data).await?;
                                } else {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "WebSocket trailers",
                                    ));
                                }
                            }
                            writer.shutdown().await
                        };
                        let download = async {
                            let mut buffer = [0; 16 * 1024];
                            loop {
                                let n = reader.read(&mut buffer).await?;
                                if n == 0 {
                                    return Ok::<_, io::Error>(());
                                }
                                tx.send(Ok(hyper::body::Frame::data(Bytes::copy_from_slice(
                                    &buffer[..n],
                                ))))
                                .await
                                .map_err(io::Error::other)?;
                            }
                        };
                        tokio::try_join!(upload, download).map(|_| ())
                    };
                    tokio::select! {
                        () = task_cancel.cancelled() => {},
                        result = relay => {
                            if let Err(error) = result {
                                let _ = tx.send(Err(Box::new(error) as BoxError)).await;
                            }
                        },
                    }
                });
                StreamBody { rx, stopped }.boxed_unsync()
            }
        };
        Response::from_parts(parts, body)
    }
}

struct StreamBody {
    rx: tokio::sync::mpsc::Receiver<Result<hyper::body::Frame<Bytes>, BoxError>>,
    stopped: CancellationToken,
}
impl hyper::body::Body for StreamBody {
    type Data = Bytes;
    type Error = BoxError;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, BoxError>>> {
        self.get_mut().rx.poll_recv(cx)
    }
}
impl Drop for StreamBody {
    fn drop(&mut self) {
        self.stopped.cancel();
    }
}
