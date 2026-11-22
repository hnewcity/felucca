//! Options transported with a public route registration.
use crate::{ConfigError, access::Authentication};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteOptions {
    /// Enable Linux TPROXY listener options; false uses NAT REDIRECT / PF.
    #[serde(default)]
    pub transparent_tproxy: bool,
    #[serde(default)]
    pub payload: super::payload::PayloadOptions,
    #[serde(default)]
    pub limits: crate::traffic::ResourceLimits,
    #[serde(default)]
    pub proxy_protocol: super::ProxyVersion,
    #[serde(default)]
    pub proxy_accept: super::ProxyAccept,
    #[serde(default)]
    pub auth: Authentication,
    #[serde(default)]
    pub http: HttpRouteOptions,
}
impl RouteOptions {
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.proxy_accept.acl()?;
        self.auth.validate()?;
        if self.http.tls_mode != TlsMode::Http && self.auth.required() {
            return Err(ConfigError::invalid(
                "raw TLS routes cannot perform HTTP authentication",
            ));
        }
        self.http.validate()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpScheme {
    #[default]
    Any,
    Http,
    Https,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    #[default]
    Http,
    Passthrough,
    Offload,
}
impl HttpScheme {
    pub fn matches(self, tls: bool) -> bool {
        self == Self::Any
            || if tls {
                self == Self::Https
            } else {
                self == Self::Http
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRouteOptions {
    #[serde(default)]
    pub tls_mode: TlsMode,
    /// HTTP/2 prior knowledge, with h2 ALPN for TLS backends.
    #[serde(default)]
    pub backend_http2: bool,
    #[serde(default = "root")]
    pub path: String,
    #[serde(default)]
    pub scheme: HttpScheme,
    /// Replace the matching prefix, retaining the suffix and query.
    #[serde(default)]
    pub rewrite_prefix: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    /// A value sets a header; an empty string removes it.
    #[serde(default)]
    pub request_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub response_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub redirect: Option<String>,
    #[serde(default)]
    pub force_https: bool,
    #[serde(default)]
    pub cors: bool,
    /// Inline UTF-8 bodies for proxy-generated error responses.
    #[serde(default)]
    pub error_pages: BTreeMap<u16, String>,
}
fn root() -> String {
    "/".into()
}
impl Default for HttpRouteOptions {
    fn default() -> Self {
        Self {
            tls_mode: TlsMode::Http,
            backend_http2: false,
            path: root(),
            scheme: HttpScheme::Any,
            rewrite_prefix: None,
            host: None,
            request_headers: BTreeMap::new(),
            response_headers: BTreeMap::new(),
            redirect: None,
            force_https: false,
            cors: false,
            error_pages: BTreeMap::new(),
        }
    }
}
impl HttpRouteOptions {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.tls_mode != TlsMode::Http {
            let mut plain = self.clone();
            plain.tls_mode = TlsMode::Http;
            plain.scheme = HttpScheme::Any;
            if plain != Self::default() || self.scheme == HttpScheme::Http {
                return Err(ConfigError::invalid(
                    "raw SNI routes own the whole TLS connection and cannot use HTTP path/header options",
                ));
            }
        }
        for path in std::iter::once(&self.path).chain(self.rewrite_prefix.iter()) {
            if !path.starts_with('/')
                || path.starts_with("//")
                || path
                    .bytes()
                    .any(|b| b <= 32 || b == 127 || b == b'?' || b == b'#')
            {
                return Err(ConfigError::invalid(
                    "HTTP prefixes need absolute paths without query or fragment",
                ));
            }
        }
        if let Some(host) = &self.host {
            let authority = host
                .parse::<http::uri::Authority>()
                .map_err(|_| ConfigError::invalid("invalid backend Host"))?;
            if authority.as_str().contains('@') {
                return Err(ConfigError::invalid("backend Host cannot contain userinfo"));
            }
        }
        for (name, value) in self.request_headers.iter().chain(&self.response_headers) {
            let name = http::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ConfigError::invalid("invalid configured HTTP header name"))?;
            if matches!(
                name.as_str(),
                "host"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "upgrade"
                    | "trailer"
                    | "te"
                    | "keep-alive"
                    | "proxy-connection"
            ) {
                return Err(ConfigError::invalid(
                    "framing/hop headers cannot be rewritten; use http.host for Host",
                ));
            }
            http::HeaderValue::from_str(value)
                .map_err(|_| ConfigError::invalid("invalid configured HTTP header value"))?;
            validate_template(value)?;
        }
        if let Some(url) = &self.redirect {
            validate_template(url)?;
            let probe = url
                .replace("{host}", "example.com")
                .replace("{path}", "/test")
                .replace("{scheme}", "https")
                .replace("{remote_ip}", "127.0.0.1")
                .replace("{remote_port}", "1234");
            let uri: http::Uri = probe
                .parse()
                .map_err(|_| ConfigError::invalid("invalid redirect URL"))?;
            if !matches!(uri.scheme_str(), Some("http" | "https"))
                || uri.authority().is_none_or(|a| a.as_str().contains('@'))
            {
                return Err(ConfigError::invalid(
                    "redirect must be an absolute HTTP or HTTPS URL",
                ));
            }
        }
        if self
            .error_pages
            .iter()
            .any(|(status, body)| !(400..600).contains(status) || body.len() > 65536)
        {
            return Err(ConfigError::invalid(
                "error pages need status 400..599 and at most 64 KiB each",
            ));
        }
        Ok(())
    }
}
fn validate_template(value: &str) -> Result<(), ConfigError> {
    let remaining = [
        "{host}",
        "{path}",
        "{scheme}",
        "{remote_ip}",
        "{remote_port}",
    ]
    .iter()
    .fold(value.to_owned(), |s, token| s.replace(token, ""));
    if remaining.contains(['{', '}']) {
        return Err(ConfigError::invalid("unknown HTTP template variable"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn route_configuration_rejects_typos_and_http_smuggling_fields() {
        assert!(toml::from_str::<RouteOptions>("[http]\nrewite_prefix='/x/'").is_err());
        for (name, value) in [
            ("Content-Length", "9"),
            ("X-Test", "a\r\nInjected: value"),
            ("X-Tag", "{misspelled}"),
        ] {
            let options = HttpRouteOptions {
                request_headers: [(name.into(), value.into())].into(),
                ..Default::default()
            };
            assert!(options.validate().is_err(), "{name}");
        }
        for prefix in [
            "relative",
            "//other.test/",
            "/path?query=1",
            "/path#fragment",
        ] {
            assert!(
                HttpRouteOptions {
                    path: prefix.into(),
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }
    #[test]
    fn proxy_accounts_and_inline_error_pages_load_and_roundtrip() {
        let config: RouteOptions = toml::from_str(
            "[auth.users]\nalice='secret'\n[http.error_pages]\n'502'='<h1>offline</h1>'",
        )
        .unwrap();
        config.validate().unwrap();
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<RouteOptions>(&encoded).unwrap(), config);
    }
}
