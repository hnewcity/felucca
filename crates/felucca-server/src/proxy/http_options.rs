use crate::http::{self, Body};
use ::http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use felucca_core::config::{HttpRouteOptions, RouteOptions};
use std::{collections::BTreeMap, net::SocketAddr};

pub struct Context {
    host: String,
    path: String,
    scheme: &'static str,
    remote_ip: String,
    remote_port: String,
    pub https_port: u16,
}
impl Context {
    pub fn new(host: &str, uri: &Uri, peer: SocketAddr, tls: bool) -> Self {
        Self {
            host: host.into(),
            path: uri.path_and_query().map_or("/", |v| v.as_str()).into(),
            scheme: if tls { "https" } else { "http" },
            remote_ip: peer.ip().to_string(),
            remote_port: peer.port().to_string(),
            https_port: 443,
        }
    }
    fn expand(&self, template: &str) -> String {
        // Never interpret placeholders inside substituted, caller-controlled values.
        let mut output = String::new();
        let mut rest = template;
        while let Some(start) = rest.find('{') {
            output.push_str(&rest[..start]);
            let Some(end) = rest[start..].find('}').map(|n| start + n) else {
                break;
            };
            output.push_str(match &rest[start..=end] {
                "{host}" => &self.host,
                "{path}" => &self.path,
                "{scheme}" => self.scheme,
                "{remote_ip}" => &self.remote_ip,
                "{remote_port}" => &self.remote_port,
                other => other,
            });
            rest = &rest[end + 1..];
        }
        output.push_str(rest);
        output
    }
}

pub fn error(options: &HttpRouteOptions, status: StatusCode, text: &str) -> Response<Body> {
    let mut response = http::response(
        status,
        options
            .error_pages
            .get(&status.as_u16())
            .cloned()
            .unwrap_or_else(|| text.into()),
    );
    if options.error_pages.contains_key(&status.as_u16()) {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
    }
    response
}

pub fn intercept<B>(
    request: &mut Request<B>,
    options: &RouteOptions,
    context: &Context,
) -> Option<Response<Body>> {
    // Browsers do not put application credentials on a CORS preflight. It only
    // advertises methods/headers; authentication still applies to the real request.
    let preflight = options.http.cors
        && request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
    let mut early = if options.http.force_https && context.scheme == "http" {
        let authority: ::http::uri::Authority = context.host.parse().ok()?;
        let host = authority.host();
        let authority = if context.https_port == 443 {
            host.to_owned()
        } else {
            format!("{host}:{}", context.https_port)
        };
        Some(redirect(
            StatusCode::MOVED_PERMANENTLY,
            &format!("https://{authority}{}", context.path),
        ))
    } else if !preflight && !http::authenticated(request.headers(), &options.auth, false) {
        Some(http::authentication_required(false))
    } else if let Some(url) = &options.http.redirect
        && !preflight
    {
        Some(redirect(
            StatusCode::TEMPORARY_REDIRECT,
            &context.expand(url),
        ))
    } else if preflight {
        let mut response = http::response(StatusCode::NO_CONTENT, "");
        let method = &request.headers()[header::ACCESS_CONTROL_REQUEST_METHOD];
        if Method::from_bytes(method.as_bytes()).is_err() {
            return Some(http::response(
                StatusCode::BAD_REQUEST,
                "invalid CORS method",
            ));
        }
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_METHODS, method.clone());
        if let Some(headers) = request
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        {
            if headers.to_str().ok().is_none_or(|value| {
                value
                    .split(',')
                    .any(|name| header::HeaderName::from_bytes(name.trim().as_bytes()).is_err())
            }) {
                return Some(http::response(
                    StatusCode::BAD_REQUEST,
                    "invalid CORS headers",
                ));
            }
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_HEADERS, headers.clone());
        }
        Some(response)
    } else {
        None
    };
    if options.auth.required() {
        request.headers_mut().remove(header::AUTHORIZATION);
    }
    if let Some(response) = &mut early {
        finish(response, &options.http, context);
    }
    early
}

fn redirect(status: StatusCode, url: &str) -> Response<Body> {
    match HeaderValue::from_str(url) {
        Ok(value) => {
            let mut response = http::response(status, "");
            response.headers_mut().insert(header::LOCATION, value);
            response
        }
        Err(_) => http::response(StatusCode::BAD_REQUEST, "invalid redirect target"),
    }
}

fn headers(map: &mut HeaderMap, configured: &BTreeMap<String, String>, context: &Context) {
    for (name, value) in configured {
        // Config is validated at registration. A runtime template can still
        // produce a non-ASCII field; omit it instead of panicking.
        let Ok(name) = header::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        if value.is_empty() {
            map.remove(name);
        } else if let Ok(value) = HeaderValue::from_str(&context.expand(value)) {
            map.insert(name, value);
        }
    }
}

pub fn rewrite<B>(
    request: &mut Request<B>,
    options: &HttpRouteOptions,
    context: &Context,
) -> Result<(), &'static str> {
    if let Some(prefix) = &options.rewrite_prefix {
        let path = request.uri().path_and_query().map_or("/", |v| v.as_str());
        let Some(suffix) = path.strip_prefix(&options.path) else {
            return Err("path does not match route");
        };
        *request.uri_mut() = format!("{prefix}{suffix}")
            .parse()
            .map_err(|_| "invalid rewritten path")?;
    }
    if let Some(host) = &options.host
        && let Ok(value) = HeaderValue::from_str(host)
    {
        request.headers_mut().insert(header::HOST, value);
    }
    headers(request.headers_mut(), &options.request_headers, context);
    Ok(())
}

pub fn finish(response: &mut Response<Body>, options: &HttpRouteOptions, context: &Context) {
    if let Some(body) = options.error_pages.get(&response.status().as_u16()) {
        *response.body_mut() = http::response(response.status(), body.clone()).into_body();
        for name in [
            header::CONTENT_LENGTH,
            header::CONTENT_ENCODING,
            header::CONTENT_RANGE,
            header::ETAG,
            header::TRANSFER_ENCODING,
        ] {
            response.headers_mut().remove(name);
        }
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
    }
    if options.cors {
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    }
    headers(response.headers_mut(), &options.response_headers, context);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn https_redirect_replaces_the_http_port_and_preserves_ipv6_and_query() {
        let mut request = Request::builder().uri("/path?q=1").body(()).unwrap();
        let mut context = Context::new(
            "[::1]:8080",
            request.uri(),
            "[::1]:9876".parse().unwrap(),
            false,
        );
        context.https_port = 8443;
        let options = RouteOptions {
            http: HttpRouteOptions {
                force_https: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let response = intercept(&mut request, &options, &context).unwrap();
        assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(
            response.headers()[header::LOCATION],
            "https://[::1]:8443/path?q=1"
        );
    }
    #[test]
    fn template_values_are_not_expanded_recursively() {
        let context = Context::new(
            "example.test",
            &"/{host}".parse().unwrap(),
            "127.0.0.1:99".parse().unwrap(),
            false,
        );
        assert_eq!(
            context.expand("{path}|{host}|{remote_port}"),
            "/{host}|example.test|99"
        );
    }
}
