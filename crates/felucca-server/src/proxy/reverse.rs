use crate::{
    http::{self, Body},
    relay::Route,
};
use ::http::{HeaderValue, Request, StatusCode, header};
use felucca_core::config::route::TlsMode;
use felucca_core::config::{HttpProxyConfig, host_matches, normalize_host_pattern};
use felucca_transport::proxy_protocol::{Acceptor, Addresses};
use felucca_transport::{Identity, TransportError, tcp};
use parking_lot::RwLock;
use rustls::{
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct Certificates {
    keys: RwLock<HashMap<String, Arc<CertifiedKey>>>,
    default: RwLock<Option<Arc<CertifiedKey>>>,
}

impl Certificates {
    pub(crate) fn snapshot(&self) -> Vec<serde_json::Value> {
        let mut result: Vec<_> = self
            .keys
            .read()
            .iter()
            .filter_map(|(host, key)| {
                key.cert
                    .first()
                    .map(|cert| crate::certificates::status::certificate(host, cert))
            })
            .collect();
        if let Some(cert) = self
            .default
            .read()
            .as_ref()
            .and_then(|key| key.cert.first())
        {
            result.push(crate::certificates::status::certificate("default", cert));
        }
        result
    }
    pub fn key(identity: &Identity) -> Result<Arc<CertifiedKey>, TransportError> {
        let provider = rustls::crypto::ring::default_provider();
        let key =
            CertifiedKey::from_der(identity.chain.clone(), identity.key.clone_key(), &provider)
                .map_err(|e| TransportError::certificate(e.to_string()))?;
        Ok(Arc::new(key))
    }
    pub fn insert(&self, host: &str, identity: &Identity) -> Result<(), TransportError> {
        self.keys
            .write()
            .insert(host.to_owned(), Self::key(identity)?);
        Ok(())
    }
}

impl ResolvesServerCert for Certificates {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if let Some(host) = hello.server_name().map(str::to_ascii_lowercase) {
            let keys = self.keys.read();
            if let Some(key) = keys.get(&host) {
                return Some(key.clone());
            }
            if let Some((_, key)) = keys.iter().find(|(name, _)| host_matches(name, &host)) {
                return Some(key.clone());
            }
        }
        self.default.read().clone()
    }
}

#[derive(Debug)]
pub struct ReverseProxy {
    pub(crate) certificate_status: crate::certificates::status::Status,
    tasks: tokio_util::task::TaskTracker,
    pub(crate) bridge: RwLock<std::sync::Weak<crate::Server>>,
    pub config: HttpProxyConfig,
    routes: RwLock<HashMap<String, Vec<Route>>>,
    pub certificates: Arc<Certificates>,
    /// Keyed by (DNS name, challenge token), never by arbitrary request hosts.
    pub challenges: RwLock<HashMap<(String, String), String>>,
    pub tls_challenges: RwLock<HashMap<String, Arc<rustls::ServerConfig>>>,
    pool: crate::http_pool::HttpPool,
    proxy_accept: Acceptor,
}

impl ReverseProxy {
    pub(crate) async fn stopped(&self) {
        self.tasks.close();
        self.tasks.wait().await;
    }
    fn scope_host(&self, route: &mut Route, host: &str) {
        if let Some((pattern, limits)) = self
            .config
            .host_limits
            .iter()
            .filter(|(pattern, _)| host_matches(pattern, host))
            .max_by_key(|(pattern, _)| (*pattern == host, pattern.len()))
        {
            route.resources.0.push(route.client.store.meter(
                crate::resources::ResourceKey::Host {
                    pattern: pattern.clone(),
                },
                *limits,
            ));
            route.host_scope = Some(pattern.clone());
        }
    }
    pub fn new(config: HttpProxyConfig) -> Result<Self, TransportError> {
        let certificates = Arc::new(Certificates::default());
        if let Some(tls) = &config.default_tls {
            *certificates.default.write() = Some(Certificates::key(&Identity::from_pem_files(
                &tls.cert, &tls.key,
            )?)?);
        }
        for (host, tls) in &config.certificates {
            certificates.insert(host, &Identity::from_pem_files(&tls.cert, &tls.key)?)?;
        }
        Ok(Self {
            certificate_status: Default::default(),
            tasks: tokio_util::task::TaskTracker::new(),
            bridge: RwLock::new(std::sync::Weak::new()),
            proxy_accept: Acceptor::new(&config.proxy_accept)
                .map_err(|e| TransportError::tls(e.to_string()))?,
            config,
            routes: RwLock::new(HashMap::new()),
            certificates,
            challenges: RwLock::new(HashMap::new()),
            tls_challenges: RwLock::new(HashMap::new()),
            pool: Default::default(),
        })
    }

    pub fn register(&self, hosts: &[String], route: Route) -> Result<(), String> {
        route.options.validate().map_err(|e| e.to_string())?;
        if route.options.http.force_https && self.config.tls_bind.is_none() {
            return Err("force_https requires an HTTPS listener".into());
        }
        if route.options.http.tls_mode != TlsMode::Http && self.config.tls_bind.is_none() {
            return Err("raw SNI routes require an HTTPS listener".into());
        }
        let hosts: Vec<_> = hosts
            .iter()
            .map(|h| normalize_host_pattern(h).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        if hosts.is_empty() {
            return Err("HTTP route needs hosts".into());
        }
        let mut routes = self.routes.write();
        for host in &hosts {
            if routes.get(host).is_some_and(|old| {
                old.iter().any(|old| {
                    (old.options.http.tls_mode != TlsMode::Http
                        || route.options.http.tls_mode != TlsMode::Http)
                        && (old.client.id != route.client.id || old.id != route.id)
                })
            }) {
                return Err(format!("raw TLS route must own SNI {host}"));
            }
            if routes.get(host).is_some_and(|old| {
                old.iter().any(|old| {
                    old.options.http.path == route.options.http.path
                        && old.options.http.scheme == route.options.http.scheme
                        && (old.client.id != route.client.id || old.id != route.id)
                })
            }) {
                return Err(format!(
                    "host {host} path {} is already registered",
                    route.options.http.path
                ));
            }
        }
        for host in hosts {
            let entries = routes.entry(host).or_default();
            entries.retain(|old| old.client.id != route.client.id || old.id != route.id);
            entries.push(route.clone());
        }
        Ok(())
    }
    pub fn remove(&self, client: felucca_core::ClientId) {
        self.pool.remove(client);
        self.routes.write().retain(|_, routes| {
            routes.retain(|route| route.client.id != client);
            !routes.is_empty()
        });
    }
    pub(crate) fn remove_tunnel(&self, client: felucca_core::ClientId, id: u32) {
        // Evict idle backend connections; active requests own their streams.
        self.pool.remove(client);
        self.routes.write().retain(|_, routes| {
            routes.retain(|route| route.client.id != client || route.id != id);
            !routes.is_empty()
        });
    }
    pub fn domains(&self) -> Vec<String> {
        self.routes
            .read()
            .keys()
            .filter(|h| !h.starts_with("*."))
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub async fn start(self: &Arc<Self>, cancel: CancellationToken) -> Result<(), TransportError> {
        self.start_except(cancel, None, None).await
    }

    pub(crate) async fn start_except(
        self: &Arc<Self>,
        cancel: CancellationToken,
        bridge: Option<SocketAddr>,
        bridge_quic: Option<SocketAddr>,
    ) -> Result<(), TransportError> {
        // Bind all sockets before starting any tasks, so startup failure does not leak a listener.
        let parse = |bind: &Option<String>| {
            bind.as_deref()
                .map(felucca_core::config::parse_bind)
                .transpose()
                .map_err(|e| TransportError::tls(e.to_string()))
        };
        let plain_addr = parse(&self.config.bind)?;
        let secure_addr = parse(&self.config.tls_bind)?;
        let combined = plain_addr.is_some() && plain_addr == secure_addr;
        let plain = match plain_addr.filter(|addr| Some(*addr) != bridge) {
            Some(addr) => Some(tcp::bind(addr).await?),
            None => None,
        };
        let secure = match secure_addr.filter(|addr| Some(*addr) != bridge && !combined) {
            Some(addr) => Some(tcp::bind(addr).await?),
            None => None,
        };
        let http3 = match parse(&self.config.http3_bind)?.filter(|addr| Some(*addr) != bridge_quic)
        {
            Some(address) => Some(crate::http3::bind(self, address)?),
            None => None,
        };
        if let Some(endpoint) = http3 {
            self.tasks
                .spawn(crate::http3::serve(endpoint, self.clone(), cancel.clone()));
        }
        let this = self.clone();
        let reload_cancel = cancel.clone();
        self.tasks.spawn(async move {
            let mut interval = tokio::time::interval(this.config.cert_reload_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                tokio::select! {
                    () = reload_cancel.cancelled() => break,
                    _ = interval.tick() => this.reload_static().await,
                }
            }
        });
        if let Some(listener) = plain {
            let mut listener = tcp::Acceptor::new(listener);
            let this = self.clone();
            let stop = cancel.clone();
            self.tasks.spawn(async move {
                let mut tasks = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        biased;
                        () = stop.cancelled() => break,
                        _ = tasks.join_next(), if !tasks.is_empty() => {},
                        (mut io, _) = listener.accept() => {
                            let this = this.clone();
                            tasks.spawn(async move {
                                let Ok(addresses) = this.proxy_accept.accept(&mut io).await else { return; };
                                if combined {
                                    use tokio::io::AsyncReadExt;
                                    let Ok(Ok(first)) = tokio::time::timeout(
                                        std::time::Duration::from_secs(15), io.read_u8(),
                                    ).await else { return; };
                                    let io = felucca_transport::PrefixedConn::new(
                                        bytes::Bytes::copy_from_slice(&[first]), io,
                                    );
                                    if first == 0x16 {
                                        this.accept_tls(io, addresses).await;
                                    } else {
                                        this.serve_plain(io, addresses).await;
                                    }
                                } else {
                                    this.serve_plain(io, addresses).await;
                                }
                            });
                        }
                    }
                }
            });
        }
        if let Some(listener) = secure {
            let mut listener = tcp::Acceptor::new(listener);
            let this = self.clone();
            self.tasks.spawn(async move {
                let mut tasks = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => break,
                        _ = tasks.join_next(), if !tasks.is_empty() => {},
                        (mut io, _) = listener.accept() => {
                            let this = this.clone();
                            tasks.spawn(async move {
                                let Ok(addresses) = this.proxy_accept.accept(&mut io).await else { return; };
                                this.accept_tls(io, addresses).await;
                            });
                        }
                    }
                }
            });
        }
        Ok(())
    }

    pub async fn accept_tls<T>(self: Arc<Self>, io: T, addresses: Addresses)
    where
        T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        let accepted = tokio_rustls::LazyConfigAcceptor::new(
            rustls::server::Acceptor::default(),
            crate::tls_ingress::Captured::new(io),
        );
        let Ok(Ok(start)) = tokio::time::timeout_at(deadline, accepted).await else {
            return;
        };
        self.serve_tls(start, addresses, deadline).await;
    }

    pub(crate) async fn serve_plain<T>(self: Arc<Self>, io: T, addresses: Addresses)
    where
        T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        http::serve(io, move |request| {
            let this = self.clone();
            async move {
                this.request(request.map(http::incoming), addresses, None)
                    .await
            }
        })
        .await;
    }

    pub(crate) async fn accept_tls_nested<T: felucca_transport::Conn + 'static>(
        self: Arc<Self>,
        io: T,
        addresses: Addresses,
        scope: (felucca_core::ClientId, u32),
        trace: felucca_transport::local::RouteTrace,
    ) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        let accepted = tokio_rustls::LazyConfigAcceptor::new(
            rustls::server::Acceptor::default(),
            crate::tls_ingress::Captured::new(io),
        );
        let Ok(Ok(start)) = tokio::time::timeout_at(deadline, accepted).await else {
            return;
        };
        self.serve_tls_impl(start, addresses, deadline, Some(scope), trace, true)
            .await;
    }
    pub(crate) async fn serve_tls<T: felucca_transport::Conn + 'static>(
        self: Arc<Self>,
        start: tokio_rustls::StartHandshake<crate::tls_ingress::Captured<T>>,
        addresses: Addresses,
        deadline: tokio::time::Instant,
    ) {
        self.serve_tls_impl(start, addresses, deadline, None, Default::default(), false)
            .await;
    }
    async fn serve_tls_impl<T: felucca_transport::Conn + 'static>(
        self: Arc<Self>,
        mut start: tokio_rustls::StartHandshake<crate::tls_ingress::Captured<T>>,
        addresses: Addresses,
        deadline: tokio::time::Instant,
        scope: Option<(felucca_core::ClientId, u32)>,
        trace: felucca_transport::local::RouteTrace,
        nested: bool,
    ) {
        let peer = addresses.source;
        let sni = start
            .client_hello()
            .server_name()
            .unwrap_or("")
            .to_ascii_lowercase();
        let acme = start.client_hello().alpn().is_some_and(|mut protocols| {
            protocols.next() == Some(b"acme-tls/1") && protocols.next().is_none()
        });
        if acme {
            let config = self.tls_challenges.read().get(&sni).cloned();
            if let Some(config) = config {
                start.io.stop();
                if let Ok(Ok(mut tls)) =
                    tokio::time::timeout_at(deadline, start.into_stream(config)).await
                {
                    let _ = tokio::io::AsyncWriteExt::shutdown(&mut tls).await;
                }
            }
            return;
        }
        let raw = self
            .routes
            .read()
            .iter()
            .filter(|(name, routes)| {
                host_matches(name, &sni)
                    && routes.iter().any(|route| {
                        route.options.http.scheme.matches(true)
                            && scope.is_none_or(|(client, id)| {
                                route.client.id == client && route.id == id
                            })
                    })
            })
            .max_by_key(|(name, _)| *name == &sni)
            .and_then(|(_, routes)| {
                routes.iter().find(|route| {
                    route.options.http.tls_mode != TlsMode::Http
                        && route.options.http.scheme.matches(true)
                        && scope
                            .is_none_or(|(client, id)| route.client.id == client && route.id == id)
                })
            })
            .cloned();
        if let Some(mut route) = raw {
            route.trace = trace;
            self.scope_host(&mut route, &sni);
            route.destination = addresses.destination;
            let Ok(_slot) = route.acquire(peer) else {
                return;
            };
            if route.options.http.tls_mode == TlsMode::Passthrough {
                let (prefix, io) = start.io.finish();
                let mut io = felucca_transport::PrefixedConn::new(prefix, io);
                if let Ok(mut tunnel) = route.open(peer, None).await {
                    crate::relay::splice(&mut io, &mut tunnel, &route).await;
                }
                return;
            }
            start.io.stop();
            let Ok(Ok(mut tls)) =
                tokio::time::timeout_at(deadline, start.into_stream(self.tls_config(false))).await
            else {
                return;
            };
            if let Ok(mut tunnel) = route.open(peer, None).await {
                crate::relay::splice(&mut tls, &mut tunnel, &route).await;
            }
            return;
        }
        start.io.stop();
        let Ok(Ok(tls)) =
            tokio::time::timeout_at(deadline, start.into_stream(self.tls_config(true))).await
        else {
            return;
        };
        http::serve(tls, move |request| {
            let this = self.clone();
            let sni = sni.clone();
            let trace = trace.clone();
            async move {
                this.request_impl(
                    request.map(http::incoming),
                    addresses,
                    Some(sni),
                    scope,
                    trace,
                    nested,
                )
                .await
            }
        })
        .await;
    }

    fn tls_config(&self, http: bool) -> Arc<rustls::ServerConfig> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(self.certificates.clone());
        if http {
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        }
        Arc::new(config)
    }

    async fn reload_static(&self) {
        for (host, files) in self
            .config
            .certificates
            .iter()
            .map(|(host, files)| (Some(host.as_str()), files))
            .chain(self.config.default_tls.iter().map(|files| (None, files)))
        {
            let result = async {
                let cert = tokio::fs::read(&files.cert).await?;
                let key = tokio::fs::read(&files.key).await?;
                let identity = Identity::from_pem(&cert, &key)?;
                let key = Certificates::key(&identity)?;
                if let Some(host) = host {
                    self.certificates.keys.write().insert(host.into(), key);
                } else {
                    *self.certificates.default.write() = Some(key);
                }
                Ok::<_, TransportError>(())
            }
            .await;
            if let Err(error) = result {
                self.certificate_status
                    .record(host.unwrap_or("default"), "reload_failed", None);
                tracing::warn!(host = host.unwrap_or("(default)"), %error, "certificate reload failed; retaining the current certificate");
            } else {
                self.certificate_status
                    .record(host.unwrap_or("default"), "loaded", None);
            }
        }
    }

    pub(crate) async fn request(
        &self,
        request: Request<Body>,
        addresses: Addresses,
        sni: Option<String>,
    ) -> ::http::Response<Body> {
        self.request_impl(request, addresses, sni, None, Default::default(), false)
            .await
    }
    pub(crate) async fn request_nested(
        &self,
        request: Request<Body>,
        addresses: Addresses,
        scope: Option<(felucca_core::ClientId, u32)>,
        trace: felucca_transport::local::RouteTrace,
    ) -> ::http::Response<Body> {
        self.request_impl(request, addresses, None, scope, trace, true)
            .await
    }
    async fn request_impl(
        &self,
        mut request: Request<Body>,
        addresses: Addresses,
        sni: Option<String>,
        scope: Option<(felucca_core::ClientId, u32)>,
        trace: felucca_transport::local::RouteTrace,
        nested: bool,
    ) -> ::http::Response<Body> {
        let peer = addresses.source;
        if request.headers().get_all(header::HOST).iter().count() > 1 {
            return http::response(StatusCode::BAD_REQUEST, "multiple Host headers");
        }
        let raw_host = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .or_else(|| {
                matches!(
                    request.version(),
                    ::http::Version::HTTP_2 | ::http::Version::HTTP_3
                )
                .then(|| request.uri().authority().map(|a| a.as_str()))
                .flatten()
            })
            .unwrap_or("");
        let Ok(authority) = raw_host.parse::<::http::uri::Authority>() else {
            return http::response(StatusCode::BAD_REQUEST, "invalid Host");
        };
        if authority.as_str().contains('@')
            || request
                .uri()
                .authority()
                .is_some_and(|uri_host| !uri_host.as_str().eq_ignore_ascii_case(authority.as_str()))
        {
            return http::response(StatusCode::BAD_REQUEST, "ambiguous request host");
        }
        let host = authority.host().trim_end_matches('.').to_ascii_lowercase();
        if sni.as_ref().is_some_and(|s| !s.is_empty() && s != &host) {
            return http::response(StatusCode::MISDIRECTED_REQUEST, "Host does not match SNI");
        }
        let bridge = if nested {
            None
        } else {
            self.bridge.read().upgrade()
        };
        if let Some(bridge) = bridge
            && let Some(response) = bridge.websocket_upgrade(&mut request, peer, sni.is_some())
        {
            return response;
        }
        if let Some(token) = request
            .uri()
            .path()
            .strip_prefix("/.well-known/acme-challenge/")
        {
            return match self
                .challenges
                .read()
                .get(&(host.clone(), token.to_owned()))
                .cloned()
            {
                Some(value)
                    if request.method() == ::http::Method::GET
                        || request.method() == ::http::Method::HEAD =>
                {
                    http::response(StatusCode::OK, value)
                }
                _ => http::response(StatusCode::NOT_FOUND, "unknown challenge"),
            };
        }
        let route = {
            let routes = self.routes.read();
            routes
                .iter()
                .filter(|(name, _)| host_matches(name, &host))
                .flat_map(|(name, routes)| routes.iter().map(move |route| (name, route)))
                .filter(|(_, r)| {
                    scope.is_none_or(|(client, id)| r.client.id == client && r.id == id)
                        && r.options.http.tls_mode == TlsMode::Http
                        && request.uri().path().starts_with(&r.options.http.path)
                        && r.options.http.scheme.matches(sni.is_some())
                })
                .max_by_key(|(name, r)| {
                    (
                        *name == &host,
                        r.options.http.path.len(),
                        r.options.http.scheme != felucca_core::config::HttpScheme::Any,
                    )
                })
                .map(|(_, r)| r.clone())
        };
        let Some(mut route) = route else {
            return http::response(StatusCode::NOT_FOUND, "unknown host");
        };
        route.trace = trace;
        self.scope_host(&mut route, &host);
        route.destination = addresses.destination;
        let mut context = crate::http_options::Context::new(
            authority.as_str(),
            request.uri(),
            peer,
            sni.is_some(),
        );
        context.https_port = self
            .config
            .tls_bind
            .as_deref()
            .and_then(|bind| felucca_core::config::parse_bind(bind).ok())
            .map_or(443, |address| address.port());
        let options = &route.options.http;
        if route.check(peer).is_err() {
            return crate::http_options::error(options, StatusCode::FORBIDDEN, "route denied");
        }
        if let Some(response) =
            crate::http_options::intercept(&mut request, &route.options, &context)
        {
            return response;
        }
        let original_host = authority.to_string();
        request
            .extensions_mut()
            .insert(http::OriginScheme(if sni.is_some() {
                "https"
            } else {
                "http"
            }));
        *request.uri_mut() = http::origin_uri(request.uri());
        let upgrade = http::wants_upgrade(request.headers());
        http::strip_hop_headers(request.headers_mut(), upgrade);
        request
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_str(&original_host).unwrap());
        if self.config.forwarded_headers {
            // This is the public edge. Untrusted forwarding headers must not let a caller spoof its IP.
            for name in [
                "forwarded",
                "x-forwarded-for",
                "x-forwarded-host",
                "x-forwarded-proto",
                "x-real-ip",
            ] {
                request.headers_mut().remove(name);
            }
            request.headers_mut().insert(
                "x-forwarded-for",
                HeaderValue::from_str(&peer.ip().to_string()).unwrap(),
            );
            request.headers_mut().insert(
                "x-real-ip",
                HeaderValue::from_str(&peer.ip().to_string()).unwrap(),
            );
            request.headers_mut().insert(
                "x-forwarded-host",
                HeaderValue::from_str(&original_host).unwrap(),
            );
            request.headers_mut().insert(
                "x-forwarded-proto",
                HeaderValue::from_static(if sni.is_some() { "https" } else { "http" }),
            );
        }
        if let Err(reason) = crate::http_options::rewrite(&mut request, options, &context) {
            return crate::http_options::error(options, StatusCode::BAD_REQUEST, reason);
        }
        let mut response = http::send(
            request,
            &route,
            peer,
            None,
            self.config.response_timeout,
            &self.pool,
        )
        .await;
        crate::http_options::finish(&mut response, options, &context);
        if sni.is_some()
            && !response.headers().contains_key(header::ALT_SVC)
            && let Some(port) = self
                .config
                .http3_bind
                .as_deref()
                .and_then(|bind| felucca_core::config::parse_bind(bind).ok())
                .map(|addr| addr.port())
        {
            response.headers_mut().insert(
                header::ALT_SVC,
                HeaderValue::from_str(&format!("h3=\":{port}\"; ma=86400")).expect("numeric port"),
            );
        }
        response
    }
}
