use super::{
    Admin, Error, Result, auth::LoginError, bad, credentials, forbidden, json, resources,
    secure_headers,
};
use crate::{
    control::{Change, Command, Operation},
    http_support::Body,
};
use felucca_core::{
    Secret,
    config::{AdminConfig, AdminRole},
};
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{net::IpAddr, sync::Arc, time::Duration};

const MAX_BODY: usize = 2 * 1024 * 1024;

impl Admin {
    pub(super) async fn request(
        self: Arc<Self>,
        request: Request<Incoming>,
        peer: IpAddr,
    ) -> Response<Body> {
        self.request_body(request.map(crate::http_support::incoming), peer, None)
            .await
    }
    pub(crate) async fn request_body(
        self: Arc<Self>,
        request: Request<Body>,
        peer: IpAddr,
        reverse: Option<&crate::platform::ReverseIdentity>,
    ) -> Response<Body> {
        // The language follows the request rather than the process: one console
        // serves every operator, and each browser asks in its own language.
        let language = felucca_i18n::from_headers(
            header(&request, "x-felucca-locale"),
            header(&request, header::ACCEPT_LANGUAGE.as_str()),
        )
        .unwrap_or(felucca_i18n::Language::English);
        let mut response = match self.clone().handle(request, peer, reverse, language).await {
            Ok(response) => response,
            Err(Error(status, message)) => json(status, json!({"error":message.render(language)})),
        };
        secure_headers(&mut response);
        response
    }
    async fn handle(
        self: Arc<Self>,
        mut request: Request<Body>,
        peer: IpAddr,
        reverse: Option<&crate::platform::ReverseIdentity>,
        language: felucca_i18n::Language,
    ) -> Result<Response<Body>> {
        let _permit = self.requests.clone().try_acquire_owned().map_err(|_| {
            Error(
                StatusCode::TOO_MANY_REQUESTS,
                "management request limit reached".into(),
            )
        })?;
        let daemon = self.daemon.upgrade().ok_or_else(unavailable)?;
        let (revision, config, server) = daemon.view();
        let admin = config.admin.as_ref().ok_or_else(unavailable)?;
        let origin = origin(
            admin,
            request.headers(),
            request.uri().authority().map(|a| a.as_str()),
        )?;
        let method = request.method().clone();
        let uri = request.uri().clone();
        let path = uri.path().to_owned();
        if request.headers().get_all(header::ORIGIN).iter().count() > 1
            || request
                .headers()
                .get(header::ORIGIN)
                .is_some_and(|v| v.as_bytes() != origin.as_bytes())
            || request
                .headers()
                .get("sec-fetch-site")
                .is_some_and(|v| v == "cross-site")
        {
            return Err(forbidden());
        }

        if method == Method::GET && matches!(path.as_str(), "/connect.sh" | "/connect.ps1") {
            return Ok(super::connect::script(path.ends_with(".ps1"), language));
        }
        if method == Method::GET && path.starts_with("/api/connect/") {
            return super::connect::download(&config, &path, request.headers()).await;
        }
        if method == Method::GET && !path.starts_with("/api/") && path != "/metrics" {
            return super::assets::serve(&path);
        }
        if path == "/api/languages" && method == Method::GET {
            // Public on purpose: it describes this listener's languages and
            // nothing about the deployment, so a client can pick one before it
            // has a session, and an operator can see what is available.
            return Ok(json(
                StatusCode::OK,
                json!({
                    "languages": felucca_i18n::Language::ALL
                        .iter()
                        .map(|language| json!({
                            "code": language.code(),
                            "tag": language.tag(),
                        }))
                        .collect::<Vec<_>>(),
                    "preferred": language.code(),
                }),
            ));
        }
        if path == "/api/login" && method == Method::POST {
            if !request.headers().contains_key(header::ORIGIN) {
                return Err(forbidden());
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Login {
                account: String,
                password: Secret,
            }
            let login: Login = body(request).await?;
            let issued = self
                .auth
                .login(admin, login.account, login.password, peer)
                .await
                .map_err(|e| match e {
                    LoginError::BadCredentials => {
                        Error(StatusCode::UNAUTHORIZED, "invalid credentials".into())
                    }
                    LoginError::RateLimited => Error(
                        StatusCode::TOO_MANY_REQUESTS,
                        "login limit reached; retry later".into(),
                    ),
                    LoginError::Unavailable => unavailable(),
                })?;
            let mut response = json(
                StatusCode::OK,
                json!({"principal":issued.principal,"csrf":issued.csrf}),
            );
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&issued.cookie).expect("cookie encoding"),
            );
            return Ok(response);
        }
        if path == "/api/ws" && method == Method::GET && reverse.is_none() {
            crate::platform::direct::browser_token(request.headers_mut())?;
        }
        let authenticated = if reverse.is_some() {
            crate::platform::authenticate(config.node.as_ref(), request.headers(), reverse)
        } else {
            self.auth
                .authenticate(admin, request.headers())
                .or_else(|| {
                    crate::platform::authenticate(config.node.as_ref(), request.headers(), None)
                })
        }
        .ok_or_else(|| Error(StatusCode::UNAUTHORIZED, "authentication required".into()))?;
        let principal = &authenticated.principal;
        if method != Method::GET
            && method != Method::HEAD
            && (!authenticated.csrf_valid(request.headers())
                || (authenticated.csrf.is_some()
                    && !request.headers().contains_key(header::ORIGIN)))
        {
            return Err(forbidden());
        }
        if path == "/api/ws" && method == Method::GET && reverse.is_none() {
            let node = self.node.read().clone().ok_or_else(unavailable)?;
            return node.upgrade(request, principal.clone());
        }
        if path.starts_with("/api/node/") {
            let node = self.node.read().clone().ok_or_else(unavailable)?;
            return node.api(request, principal, revision).await;
        }
        if method == Method::GET && path.starts_with("/api/diagnostics/") {
            let parts: Vec<_> = path
                .trim_start_matches("/api/diagnostics/")
                .split('/')
                .collect();
            if parts.len() != 2 {
                return Err(super::missing());
            }
            let client = percent_encoding::percent_decode_str(parts[0])
                .decode_utf8()
                .map_err(|_| super::missing())?;
            let id = parts[1].parse().map_err(|_| super::missing())?;
            let result =
                crate::control::diagnostics::backends(&server, &config, principal, &client, id)
                    .await?;
            return Ok(json(
                StatusCode::OK,
                json!({"revision":revision,"diagnostics":result}),
            ));
        }
        match (method.as_str(), path.as_str()) {
            ("GET", "/api/agent-packages") => {
                let packages = tokio::task::spawn_blocking(super::downloads::catalog)
                    .await
                    .map_err(|_| unavailable())?;
                return Ok(json(StatusCode::OK, json!({"packages": packages})));
            }
            ("GET", path) if path.starts_with("/api/agent-packages/") => {
                return super::downloads::download(path.trim_start_matches("/api/agent-packages/"))
                    .await;
            }
            ("GET", "/api/launch") => {
                return Ok(json(
                    StatusCode::OK,
                    json!({"revision":revision,
                    "bridge": {"bind":config.bridge.bind,"quic_bind":config.bridge.quic_bind,
                        "kcp_bind":config.bridge.kcp_bind,"ws_path":config.bridge.ws_path},
                    "fingerprint":server.fingerprint(), "p2p": config.p2p.is_some(),
                    "http_bind":config.http.as_ref().and_then(|http|http.bind.as_ref()),
                    "https_bind":config.http.as_ref().and_then(|http|http.tls_bind.as_ref())}),
                ));
            }
            ("GET", "/api/certificates") => {
                resources::admin(principal)?;
                let mut diagnostics = server.certificate_diagnostics();
                if let Some(certificate) = self.tls.as_ref().and_then(|tls| tls.status()) {
                    diagnostics["certificates"]
                        .as_array_mut()
                        .unwrap()
                        .push(certificate);
                }
                return Ok(json(
                    StatusCode::OK,
                    json!({"revision":revision,"diagnostics":diagnostics}),
                ));
            }
            ("POST", "/api/certificates/retry") => {
                resources::admin(principal)?;
                return Ok(json(
                    StatusCode::OK,
                    json!({"scheduled":server.retry_certificates()}),
                ));
            }
            ("POST", "/api/certificates/upload") => {
                resources::admin(principal)?;
                let upload = body(request).await?;
                let principal = principal.clone();
                let root = daemon
                    .configuration_path()
                    .unwrap_or(&admin.state_path)
                    .to_owned();
                let result = tokio::task::spawn_blocking(move || {
                    crate::control::operations::upload_certificate(&principal, &root, upload)
                })
                .await
                .map_err(|_| unavailable())??;
                return Ok(json(StatusCode::OK, result));
            }
            ("GET", "/api/logs") => {
                resources::admin(principal)?;
                let principal = principal.clone();
                let result = tokio::task::spawn_blocking(move || {
                    crate::control::operations::logs(&principal, &config)
                })
                .await
                .map_err(|_| unavailable())??;
                return Ok(json(StatusCode::OK, result));
            }
            ("GET", "/api/diagnostics") => {
                let result =
                    crate::control::diagnostics::traffic(&server, &config, principal).await;
                return Ok(json(
                    StatusCode::OK,
                    json!({"revision":revision,"diagnostics":result}),
                ));
            }
            ("POST", "/api/token") => {
                return Ok(json(
                    StatusCode::OK,
                    crate::control::configuration::token(principal)?,
                ));
            }
            ("POST", "/api/config/export") => {
                resources::admin(principal)?;
                if expected_revision(request.headers())? != revision {
                    return Err(Error(
                        StatusCode::CONFLICT,
                        "configuration revision changed".into(),
                    ));
                }
                let result = crate::control::configuration::export(
                    principal,
                    &config,
                    revision,
                    daemon.configuration_path(),
                    body(request).await?,
                )?;
                return Ok(json(StatusCode::OK, result));
            }
            ("GET", "/api/monitor") => {
                let window = super::monitor::window(uri.query())?;
                return Ok(json(
                    StatusCode::OK,
                    self.host_metrics.snapshot(principal, window)?,
                ));
            }
            ("POST", "/api/profile") => {
                resources::admin(principal)?;
                if !admin.profiling {
                    return Err(forbidden());
                }
                let options: super::profile::Options = body(request).await?;
                return super::profile::collect(options, self.cancel.clone(), &self.tasks).await;
            }
            ("GET", "/metrics" | "/api/metrics") => {
                let text =
                    super::metrics::render(&server, &config, principal, revision, daemon.healthy())
                        .await;
                let mut response = crate::http_support::response(StatusCode::OK, text);
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
                );
                return Ok(response);
            }
            ("GET", "/api/session") => {
                return Ok(json(
                    StatusCode::OK,
                    json!({"principal":principal,"csrf":authenticated.csrf}),
                ));
            }
            ("POST", "/api/logout") => {
                self.auth.logout(request.headers());
                let mut response = json(StatusCode::OK, json!({"logged_out":true}));
                response.headers_mut().insert(
                    header::SET_COOKIE,
                    HeaderValue::from_static(
                        "felucca_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
                    ),
                );
                return Ok(response);
            }
            ("GET", "/api/overview") => {
                let clients: Vec<_> = config
                    .clients
                    .iter()
                    .filter(|(_, c)| resources::visible(principal, c.user.as_deref()))
                    .map(|(id, c)| {
                        let online = server.registry().get(id);
                        let expected = config.managed.get(id).and_then(|m| m.encoded().ok()).map(|(_, revision)| revision);
                        let (applied, rejected) = online.as_ref().map(|client| {
                            let state = client.config_state.read();
                            (!state.dirty && state.revision == expected && state.rejected.is_empty(), state.rejected.clone())
                        }).unwrap_or_default();
                        let traffic = server
                            .registry()
                            .store
                            .stats(&crate::resources::ResourceKey::Client { name: id.clone() });
                        json!({"id":id,"user":c.user,"remark":c.remark,"online":online.is_some(),"enabled":c.enabled,
                            "connections":online.as_ref().map_or(0,|c|c.live_connections()),
                            "traffic":traffic,"managed":config.managed.contains_key(id),
                            "config_applied":applied,"config_rejected":rejected})
                    })
                    .collect();
                let locals: Vec<_> = config.locals.iter().filter(|(_, c)| resources::visible(principal, c.user.as_deref()))
                    .map(|(id, _)| {
                        let online = server.registry().get(&format!("local.{id}"));
                        json!({"id":id,"online":online.is_some(),"connections":online.as_ref().map_or(0,|c|c.live_connections()),
                            "traffic":online.as_ref().map(|c|c.traffic.stats())})
                    }).collect();
                let tunnels: Vec<_> = server
                    .tunnel_specs()
                    .await
                    .into_iter()
                    .filter(|(key, _)| {
                        let owner = config
                            .clients
                            .get(key)
                            .and_then(|c| c.user.as_deref())
                            .or_else(|| {
                                key.strip_prefix("local.")
                                    .and_then(|key| config.locals.get(key))
                                    .and_then(|c| c.user.as_deref())
                            });
                        resources::visible(principal, owner)
                    })
                    .map(|(client, spec)| json!({"client":client,"spec":resources::redacted(spec)}))
                    .collect();
                return Ok(json(
                    StatusCode::OK,
                    json!({"revision":revision,"healthy":daemon.healthy(),
                    "protocol":felucca_proto::PROTOCOL_VERSION,"node":config.node.is_some(),"profiling":admin.profiling && principal.role == AdminRole::Admin,
                    "clients":clients,"locals":locals,"tunnels":tunnels}),
                ));
            }
            ("GET", "/api/events") => {
                let permit = self
                    .event_streams
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| {
                        Error(
                            StatusCode::TOO_MANY_REQUESTS,
                            "event stream limit reached".into(),
                        )
                    })?;
                return Ok(self.events(
                    request.headers().clone(),
                    daemon.subscribe(),
                    revision,
                    permit,
                ));
            }
            ("GET", "/api/config") if uri.query() == Some("secrets=1") => {
                resources::admin(principal)?;
                return Ok(json(
                    StatusCode::OK,
                    json!({"revision":revision,"config":config}),
                ));
            }
            ("POST", "/api/validate") => {
                resources::admin(principal)?;
                let input: Value = body(request).await?;
                let mut next: felucca_core::ServerConfig =
                    resources::parse(input, Some(resources::value(&config)))?;
                next.validate().map_err(|e| bad(e.to_string()))?;
                return Ok(json(
                    StatusCode::OK,
                    json!({"valid":true,
                        "revision":revision,
                        "activation": if crate::control::configuration::live_compatible(&config,&next) { "live" } else { "reload" },
                        "restart_fields":crate::control::configuration::restart_fields(&config,&next)
                    }),
                ));
            }
            _ => {}
        }
        if method == Method::GET {
            let resource = super::routes::resource(&path)?;
            return Ok(json(
                StatusCode::OK,
                json!({"revision":revision,"config":resources::read(&config, principal, &resource)?}),
            ));
        }
        if principal.role == AdminRole::Viewer {
            return Err(forbidden());
        }
        let expected = expected_revision(request.headers())?;
        let key = idempotency_key(request.headers())?;
        let input: Value = if method == Method::DELETE {
            Value::Null
        } else {
            body(request).await?
        };
        let digest = credentials::digest(
            &serde_json::to_vec(&json!({
                "method":method.as_str(),"path":path,"value":input,"revision":expected,
            }))
            .expect("operation JSON"),
        );
        let operation = Operation {
            key,
            revision: expected,
            digest,
        };
        if let Some(applied) = self.management.replay(principal, &operation)? {
            return Ok(json(StatusCode::OK, applied));
        }
        let command = if method == Method::POST && path == "/api/batch" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Batch {
                operations: Vec<BatchChange>,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct BatchChange {
                method: String,
                path: String,
                #[serde(default)]
                value: Value,
            }
            let batch: Batch = serde_json::from_value(input).map_err(|_| bad("invalid batch"))?;
            let changes = batch
                .operations
                .into_iter()
                .map(|change| {
                    Ok(Change {
                        method: super::routes::write_method(&change.method)?,
                        resource: super::routes::resource(&change.path)?,
                        value: change.value,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Command::Batch(changes)
        } else {
            Command::Change(Change {
                method: super::routes::write_method(method.as_str())?,
                resource: super::routes::resource(&path)?,
                value: input,
            })
        };
        let applied = self.management.apply(principal, operation, command).await?;
        Ok(json(StatusCode::OK, applied))
    }

    fn events(
        self: Arc<Self>,
        headers: HeaderMap,
        receiver: tokio::sync::broadcast::Receiver<u64>,
        revision: u64,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Response<Body> {
        struct State {
            admin: Arc<Admin>,
            headers: HeaderMap,
            receiver: tokio::sync::broadcast::Receiver<u64>,
            initial: Option<u64>,
            _permit: tokio::sync::OwnedSemaphorePermit,
        }
        let state = State {
            admin: self,
            headers,
            receiver,
            initial: Some(revision),
            _permit: permit,
        };
        let stream = futures_util::stream::unfold(state, |mut state| async move {
            let event = if let Some(revision) = state.initial.take() {
                Some(revision)
            } else {
                tokio::select! {
                    () = state.admin.cancel.cancelled() => return None,
                    event = state.receiver.recv() => match event {
                        Ok(revision) => Some(revision),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => state.admin.daemon.upgrade().map(|d| d.config().0),
                        Err(_) => return None,
                    },
                    () = tokio::time::sleep(Duration::from_secs(10)) => None,
                }
            };
            let daemon = state.admin.daemon.upgrade()?;
            let (_, config) = daemon.config();
            state
                .admin
                .auth
                .authenticate(config.admin.as_ref()?, &state.headers)
                .or_else(|| {
                    crate::platform::authenticate(config.node.as_ref(), &state.headers, None)
                })?;
            let text = event.map_or_else(|| ": heartbeat\n\n".to_owned(),
                |revision| format!("id: {revision}\nevent: configuration\ndata: {{\"revision\":{revision}}}\n\n"));
            Some((
                Ok::<_, crate::http_support::BoxError>(hyper::body::Frame::data(
                    bytes::Bytes::from(text),
                )),
                state,
            ))
        });
        let mut response = Response::new(http_body_util::StreamBody::new(stream).boxed_unsync());
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        response
    }
}

fn unavailable() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "management service is stopping".into(),
    )
}
/// One request header as text, ignoring anything that is not UTF-8.
fn header<'a>(request: &'a Request<Body>, name: &str) -> Option<&'a str> {
    request.headers().get(name)?.to_str().ok()
}
pub(crate) async fn body<T: DeserializeOwned>(request: Request<Body>) -> Result<T> {
    if request
        .headers()
        .get_all(header::CONTENT_TYPE)
        .iter()
        .count()
        != 1
        || !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| {
                s.split(';')
                    .next()
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json"))
            })
    {
        return Err(Error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "application/json required".into(),
        ));
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(15),
        collect(request.into_body(), MAX_BODY),
    )
    .await
    .map_err(|_| Error(StatusCode::REQUEST_TIMEOUT, "request body timed out".into()))?
    .map_err(|_| {
        Error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid or oversized request body (2 MiB limit)".into(),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|_| bad("invalid JSON body"))
}
pub(crate) async fn collect(mut body: Body, maximum: usize) -> std::io::Result<bytes::Bytes> {
    let mut bytes = bytes::BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| std::io::ErrorKind::InvalidData)?;
        if let Ok(data) = frame.into_data() {
            if data.len() > maximum.saturating_sub(bytes.len()) {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            bytes.extend_from_slice(&data);
        }
    }
    Ok(bytes.freeze())
}
pub(crate) fn expected_revision(headers: &HeaderMap) -> Result<u64> {
    if headers.get_all(header::IF_MATCH).iter().count() != 1 {
        return Err(Error(
            StatusCode::PRECONDITION_REQUIRED,
            "If-Match revision required".into(),
        ));
    }
    headers
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| bad("If-Match must contain a quoted revision"))
}
pub(crate) fn idempotency_key(headers: &HeaderMap) -> Result<String> {
    if headers.get_all("idempotency-key").iter().count() != 1 {
        return Err(bad("Idempotency-Key required"));
    }
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad("invalid Idempotency-Key"))?;
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(bad(
            "Idempotency-Key requires 1..128 letters, digits, dots, hyphens or underscores",
        ));
    }
    Ok(key.into())
}
fn origin(
    config: &AdminConfig,
    headers: &HeaderMap,
    uri_authority: Option<&str>,
) -> Result<String> {
    if headers.get_all(header::HOST).iter().count() > 1 {
        return Err(bad("one Host header required"));
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or(uri_authority)
        .ok_or_else(|| bad("invalid Host"))?;
    if uri_authority.is_some_and(|authority| !host.eq_ignore_ascii_case(authority)) {
        return Err(bad("ambiguous Host"));
    }
    let authority = host
        .parse::<http::uri::Authority>()
        .map_err(|_| bad("invalid Host"))?;
    if host.contains('@') {
        return Err(bad("invalid Host"));
    }
    let secure = config.secure();
    let default_port = if secure { 443 } else { 80 };
    let checked_port = |authority: &http::uri::Authority| -> Result<u16> {
        let raw = authority.as_str();
        if !raw.ends_with(']')
            && let Some((_, port)) = raw.rsplit_once(':')
        {
            return port
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| bad("invalid Host port"));
        }
        Ok(default_port)
    };
    let port = checked_port(&authority)?;
    let canonical =
        |scheme: &str, authority: &http::uri::Authority, port: u16, default_port: u16| {
            let host = authority.host().to_ascii_lowercase();
            if port == default_port {
                format!("{scheme}://{host}")
            } else {
                format!("{scheme}://{host}:{port}")
            }
        };
    if let Some(origin) = &config.origin {
        let (plain, rest) = felucca_core::config::split_origin(origin)
            .ok_or_else(|| bad("invalid configured origin"))?;
        let expected = rest
            .parse::<http::uri::Authority>()
            .map_err(|_| bad("invalid configured origin"))?;
        let expected_port = checked_port(&expected)?;
        if !authority.host().eq_ignore_ascii_case(expected.host()) || port != expected_port {
            return Err(forbidden());
        }
        let (scheme, default_port) = if plain { ("http", 80) } else { ("https", 443) };
        return Ok(canonical(scheme, &expected, expected_port, default_port));
    }
    let bind = felucca_core::config::parse_bind(&config.bind).map_err(|e| bad(e.to_string()))?;
    let host_ip = authority
        .host()
        .trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .ok();
    if port != bind.port()
        || !(host_ip == Some(bind.ip())
            || (bind.ip().is_loopback() && authority.host().eq_ignore_ascii_case("localhost")))
    {
        return Err(forbidden());
    }
    Ok(canonical(
        if secure { "https" } else { "http" },
        &authority,
        port,
        default_port,
    ))
}
