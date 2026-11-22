//! Streaming HTTP/1 proxying and upgrades, shared by forward and reverse proxies.
pub(crate) use crate::http_support::{Body, BoxError, incoming, response, serve};
use crate::relay::Route;
use base64::Engine;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use std::{net::SocketAddr, sync::Arc, time::Duration};

#[derive(Debug, Clone, Copy)]
pub struct OriginScheme(pub &'static str);

pub fn authenticated(
    headers: &HeaderMap,
    auth: &felucca_core::access::Authentication,
    proxy: bool,
) -> bool {
    if !auth.required() {
        return true;
    }
    let name = if proxy {
        header::PROXY_AUTHORIZATION
    } else {
        header::AUTHORIZATION
    };
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next().and_then(|v| v.to_str().ok()) else {
        return false;
    };
    if values.next().is_some() || value.len() > 1024 {
        return false;
    }
    let Some((scheme, encoded)) = value.split_once(' ') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return false;
    }
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
        return false;
    };
    let Some(colon) = decoded.iter().position(|b| *b == b':') else {
        return false;
    };
    auth.verify(&decoded[..colon], &decoded[colon + 1..])
}

pub fn authentication_required(proxy: bool) -> Response<Body> {
    let mut response = response(
        if proxy {
            StatusCode::PROXY_AUTHENTICATION_REQUIRED
        } else {
            StatusCode::UNAUTHORIZED
        },
        "authentication required",
    );
    response.headers_mut().insert(
        if proxy {
            header::PROXY_AUTHENTICATE
        } else {
            header::WWW_AUTHENTICATE
        },
        HeaderValue::from_static("Basic realm=\"felucca\", charset=\"UTF-8\""),
    );
    response
}

fn target_error(error: &std::io::Error) -> Response<Body> {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        response(StatusCode::FORBIDDEN, "destination denied")
    } else {
        response(StatusCode::BAD_GATEWAY, "proxy target unavailable")
    }
}

pub fn strip_hop_headers(headers: &mut HeaderMap, upgrade: bool) {
    let nominated: Vec<String> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    for name in nominated {
        if !upgrade || name != "upgrade" {
            headers.remove(name);
        }
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        headers.remove(name);
    }
    if upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    } else {
        headers.remove(header::UPGRADE);
    }
}

pub fn origin_uri(uri: &Uri) -> Uri {
    // An absolute-form `GET http://host?x` keeps the path-and-query `?x`,
    // which does not parse as an origin-form target by itself.
    let target = uri.path_and_query().map_or("/", |p| p.as_str());
    target
        .parse()
        .or_else(|_| format!("/{target}").parse())
        .unwrap_or_else(|_| Uri::from_static("/"))
}

pub fn wants_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE)
        && headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|h| h.to_str().ok())
            .any(|v| {
                v.split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            })
}

pub async fn send(
    mut request: Request<Body>,
    route: &Route,
    peer: SocketAddr,
    target: Option<String>,
    timeout: Duration,
    pool: &crate::http_pool::HttpPool,
) -> Response<Body> {
    if let Err(error) = route.check(peer) {
        return target_error(&error);
    }
    let http2 = route.options.http.backend_http2;
    let websocket = match crate::websocket::prepare(&mut request, http2) {
        Ok(handshake) => handshake,
        Err((status, message)) => return response(status, message),
    };
    let wants_upgrade = wants_upgrade(request.headers());
    if http2 && wants_upgrade {
        return response(
            StatusCode::NOT_IMPLEMENTED,
            "HTTP/1 upgrade requires an HTTP/1 backend route",
        );
    }
    let downstream =
        (wants_upgrade && websocket.is_none()).then(|| hyper::upgrade::on(&mut request));
    strip_hop_headers(request.headers_mut(), wants_upgrade);
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let scheme = request
        .extensions()
        .get::<OriginScheme>()
        .map_or("http", |scheme| scheme.0);
    let Ok(uri) = Uri::builder()
        .scheme(scheme)
        .authority(host)
        .path_and_query(
            request
                .uri()
                .path_and_query()
                .map_or("/", |value| value.as_str()),
        )
        .build()
    else {
        return response(StatusCode::BAD_REQUEST, "invalid backend request URI");
    };
    *request.uri_mut() = uri;
    *request.version_mut() = if http2 {
        ::http::Version::HTTP_2
    } else {
        ::http::Version::HTTP_11
    };
    let result = tokio::time::timeout(timeout, pool.request(request, route, peer, target)).await;
    let mut upstream = match result {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            let mut cause: &(dyn std::error::Error + 'static) = &error;
            loop {
                if let Some(error) = cause.downcast_ref::<std::io::Error>()
                    && error.kind() == std::io::ErrorKind::PermissionDenied
                {
                    return target_error(error);
                }
                match cause.source() {
                    Some(source) => cause = source,
                    None => break,
                }
            }
            return response(StatusCode::BAD_GATEWAY, "backend connection failed");
        }
        Err(_) => return response(StatusCode::GATEWAY_TIMEOUT, "backend response timed out"),
    };
    if let Some(handshake) = websocket {
        return handshake.finish(upstream, route.cancel.clone(), timeout);
    }
    let upgraded = upstream.status() == StatusCode::SWITCHING_PROTOCOLS;
    if upgraded {
        let Some(downstream) = downstream else {
            return response(StatusCode::BAD_GATEWAY, "unexpected protocol upgrade");
        };
        let backend = hyper::upgrade::on(&mut upstream);
        tokio::spawn(async move {
            if let (Ok(a), Ok(b)) = tokio::join!(downstream, backend) {
                let _ =
                    tokio::io::copy_bidirectional(&mut TokioIo::new(a), &mut TokioIo::new(b)).await;
            }
        });
    }
    strip_hop_headers(upstream.headers_mut(), upgraded);
    upstream.map(|body| body.map_err(|e| -> BoxError { Box::new(e) }).boxed_unsync())
}

pub async fn serve_forward(
    io: impl felucca_transport::Conn + 'static,
    peer: SocketAddr,
    route: Route,
) {
    let cancel = route.cancel.clone();
    let pool = Arc::new(crate::http_pool::HttpPool::default());
    tokio::select! {
        () = cancel.cancelled() => {},
        () = serve(io, move |mut request| {
            let route = route.clone();
            let pool = pool.clone();
            async move {
                if !authenticated(request.headers(), &route.options.auth, true) {
                    return authentication_required(true);
                }
                request.headers_mut().remove(header::PROXY_AUTHORIZATION);
                if request.method() == Method::CONNECT && crate::websocket::extended(&request).is_none() {
                    let Some(authority) = request.uri().authority() else {
                        return response(StatusCode::BAD_REQUEST, "CONNECT requires host:port");
                    };
                    if authority.port_u16().is_none_or(|port| port == 0) || authority.as_str().contains('@') {
                        return response(StatusCode::BAD_REQUEST, "invalid CONNECT target");
                    }
                    let mut tunnel = match route.metered(peer, Some(authority.to_string())).await {
                        Ok(io) => io,
                        Err(error) => return target_error(&error),
                    };
                    let upgrade = hyper::upgrade::on(&mut request);
                    tokio::spawn(async move {
                        if let Ok(io) = upgrade.await {
                            let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(io), &mut tunnel).await;
                        }
                    });
                    return response(StatusCode::OK, "");
                }
                if request.uri().scheme_str() != Some("http") {
                    return response(StatusCode::BAD_REQUEST, "use an absolute http URL or CONNECT");
                }
                let Some(authority) = request.uri().authority().cloned() else {
                    return response(StatusCode::BAD_REQUEST, "missing target host");
                };
                if authority.as_str().contains('@') || authority.port_u16() == Some(0) {
                    return response(StatusCode::BAD_REQUEST, "invalid proxy target");
                }
                let target = if authority.port().is_some() { authority.to_string() } else { format!("{authority}:80") };
                *request.uri_mut() = origin_uri(request.uri());
                let upgrade = wants_upgrade(request.headers());
                strip_hop_headers(request.headers_mut(), upgrade);
                request.headers_mut().insert(header::HOST, HeaderValue::from_str(authority.as_str()).expect("parsed authority"));
                send(request.map(incoming), &route, peer, Some(target), Duration::from_secs(100), &pool).await
            }
        }) => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_target_with_only_a_query_becomes_an_origin_form_target() {
        let uri: Uri = "http://host?x=1".parse().unwrap();
        assert_eq!(origin_uri(&uri), "/?x=1");
        let uri: Uri = "http://host/a/b?c".parse().unwrap();
        assert_eq!(origin_uri(&uri), "/a/b?c");
        let uri: Uri = "http://host".parse().unwrap();
        assert_eq!(origin_uri(&uri), "/");
    }
}
