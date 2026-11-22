//! Management listener, independent of reloadable forwarding listeners.
pub(crate) mod api;
mod assets;
pub(crate) mod auth;
mod connect;
mod downloads;
mod routes;
pub use crate::control::credentials;
mod metrics;
mod monitor;
use crate::platform as node;
mod profile;
use crate::control::resources;
use crate::{Daemon, http_support::Body};
use auth::AuthState;
use felucca_core::config::AdminConfig;
use felucca_i18n::Message;
use std::{
    io,
    net::IpAddr,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) struct Prepared {
    listener: tokio::net::TcpListener,
    tls: Option<Arc<Tls>>,
}
impl Prepared {
    pub async fn bind(config: &AdminConfig) -> io::Result<Self> {
        let tls = config
            .tls
            .as_ref()
            .map(|files| {
                Tls::load(files).map(|current| {
                    Arc::new(Tls {
                        files: files.clone(),
                        current: arc_swap::ArcSwap::from_pointee(current),
                    })
                })
            })
            .transpose()
            .map_err(io::Error::other)?;
        let listener = tokio::net::TcpListener::bind(
            felucca_core::config::parse_bind(&config.bind).map_err(io::Error::other)?,
        )
        .await?;
        Ok(Self { listener, tls })
    }
}

/// How often the management certificate files are reread.
const CERT_RELOAD_INTERVAL: Duration = Duration::from_secs(30);

/// The management certificate, reread from disk like the bridge's.
///
/// An external renewer such as certbot replaces the files every 60 to 90 days.
/// Reading them once at bind would serve an expired certificate after that.
pub(crate) struct Tls {
    files: felucca_core::config::TlsConfig,
    current: arc_swap::ArcSwap<TlsIdentity>,
}
struct TlsIdentity {
    acceptor: tokio_rustls::TlsAcceptor,
    chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    status: Option<serde_json::Value>,
}
impl Tls {
    fn load(
        files: &felucca_core::config::TlsConfig,
    ) -> std::result::Result<TlsIdentity, felucca_transport::TransportError> {
        let identity = felucca_transport::Identity::from_pem_files(&files.cert, &files.key)?;
        let status = identity
            .chain
            .first()
            .map(|der| crate::certificates::status::certificate("admin", der));
        let mut tls = (*identity.server_config()?).clone();
        tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(TlsIdentity {
            acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(tls)),
            chain: identity.chain,
            status,
        })
    }

    /// Swaps in a changed certificate. A half-written or mismatched pair fails
    /// validation in `load` and leaves the working identity in place.
    async fn reload(self: Arc<Self>) {
        let this = self.clone();
        let loaded = tokio::task::spawn_blocking(move || Self::load(&this.files)).await;
        match loaded {
            Ok(Ok(next)) if next.chain != self.current.load().chain => {
                self.current.store(Arc::new(next));
                tracing::info!("management certificate reloaded");
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "management certificate reload failed; retaining current certificate");
            }
            Err(error) => tracing::warn!(%error, "management certificate reload task failed"),
        }
    }

    fn acceptor(&self) -> tokio_rustls::TlsAcceptor {
        self.current.load().acceptor.clone()
    }

    pub(crate) fn status(&self) -> Option<serde_json::Value> {
        self.current.load().status.clone()
    }
}

pub(crate) struct Admin {
    node: parking_lot::RwLock<Option<Arc<node::Node>>>,
    pub(crate) daemon: Weak<Daemon>,
    pub(crate) auth: AuthState,
    pub(crate) management: Arc<crate::control::Management>,
    host_metrics: Arc<crate::control::host_metrics::HostMetrics>,
    tls: Option<Arc<Tls>>,
    cancel: CancellationToken,
    tasks: TaskTracker,
    connections: Arc<tokio::sync::Semaphore>,
    requests: Arc<tokio::sync::Semaphore>,
    event_streams: Arc<tokio::sync::Semaphore>,
}
impl std::fmt::Debug for Admin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Admin")
            .field("stopping", &self.cancel.is_cancelled())
            .finish()
    }
}
impl Admin {
    pub fn start(
        prepared: Prepared,
        daemon: Weak<Daemon>,
        journal: Option<Arc<node::Store>>,
    ) -> Arc<Self> {
        let auth = AuthState::new();
        let management = Arc::new(crate::control::Management::new(
            daemon.clone(),
            auth.password_workers(),
        ));
        let admin = Arc::new(Self {
            node: Default::default(),
            daemon,
            auth,
            management,
            host_metrics: crate::control::host_metrics::HostMetrics::new(),
            tls: prepared.tls.clone(),
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
            connections: Arc::new(tokio::sync::Semaphore::new(256)),
            requests: Arc::new(tokio::sync::Semaphore::new(32)),
            event_streams: Arc::new(tokio::sync::Semaphore::new(16)),
        });
        admin
            .host_metrics
            .start(admin.cancel.child_token(), &admin.tasks);
        if let Some(journal) = journal {
            *admin.node.write() = Some(node::Node::start(&admin, journal));
        }
        if let Some(tls) = prepared.tls.clone() {
            let cancel = admin.cancel.clone();
            admin.tasks.spawn(async move {
                let mut interval = tokio::time::interval(CERT_RELOAD_INTERVAL);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval.tick().await;
                loop {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => break,
                        _ = interval.tick() => tls.clone().reload().await,
                    }
                }
            });
        }
        let this = admin.clone();
        admin.tasks.spawn(async move {
            let mut listener = felucca_transport::tcp::Acceptor::new(prepared.listener);
            let mut full_logged: Option<tokio::time::Instant> = None;
            loop {
                tokio::select! {
                    biased;
                    () = this.cancel.cancelled() => break,
                    (io, peer) = listener.accept() => {
                        let Ok(slot) = this.connections.clone().try_acquire_owned() else {
                            let now = tokio::time::Instant::now();
                            if full_logged.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(30)) {
                                tracing::warn!(%peer, "management connection limit reached; refusing connection");
                                full_logged = Some(now);
                            }
                            continue;
                        };
                        let tls = prepared.tls.as_ref().map(|tls| tls.acceptor());
                        let admin = this.clone();
                        this.tasks.spawn(async move {
                            let _slot = slot;
                            if let Some(tls) = tls {
                                let accepted = tokio::select! {
                                    () = admin.cancel.cancelled() => return,
                                    accepted = tokio::time::timeout(Duration::from_secs(15), tls.accept(io)) => accepted,
                                };
                                if let Ok(Ok(io)) = accepted { admin.serve(io, peer.ip()).await; }
                            } else {
                                admin.serve(io, peer.ip()).await;
                            }
                        });
                    }
                }
            }
        });
        admin
    }
    pub async fn serve(self: Arc<Self>, io: impl felucca_transport::Conn + 'static, peer: IpAddr) {
        let cancel = self.cancel.clone();
        tokio::select! {
            () = cancel.cancelled() => {},
            () = crate::http_support::serve(io, move |request| {
                let admin = self.clone();
                async move { admin.request(request, peer).await }
            }) => {},
        }
    }
    pub fn stop(&self) {
        if let Some(node) = self.node.read().as_ref() {
            node.stop();
        }
        self.cancel.cancel();
        self.tasks.close();
    }
    pub async fn stopped(&self) {
        self.tasks.wait().await;
        let node = self.node.write().take();
        if let Some(node) = node {
            node.stopped().await;
        }
    }
}
pub(crate) fn compatible(old: Option<&AdminConfig>, new: Option<&AdminConfig>) -> bool {
    let identity = |config: &AdminConfig| {
        (
            config.bind.clone(),
            config.state_path.clone(),
            config.origin.clone(),
            config
                .tls
                .as_ref()
                .map(|tls| (tls.cert.clone(), tls.key.clone())),
        )
    };
    old.map(identity) == new.map(identity)
}

/// A management failure with the language it should be answered in.
///
/// The status code travels unchanged; the message renders in the language the
/// request asked for, so a console, a probe and a browser all read their own.
#[derive(Debug)]
pub(crate) struct Error(pub(crate) http::StatusCode, pub(crate) Message);
pub(crate) type Result<T> = std::result::Result<T, Error>;
pub(crate) fn bad(message: impl Into<Message>) -> Error {
    Error(http::StatusCode::BAD_REQUEST, message.into())
}
pub(crate) fn forbidden() -> Error {
    Error(
        http::StatusCode::FORBIDDEN,
        Message::text("permission denied"),
    )
}
pub(crate) fn missing() -> Error {
    Error(
        http::StatusCode::NOT_FOUND,
        Message::text("resource not found"),
    )
}
pub(crate) fn json(status: http::StatusCode, value: impl serde::Serialize) -> http::Response<Body> {
    let mut response =
        crate::http_support::response(status, serde_json::to_vec(&value).expect("JSON response"));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    response
}
fn secure_headers(response: &mut http::Response<Body>) {
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, http::HeaderValue::from_static(value));
    }
}

impl From<crate::control::Error> for Error {
    fn from(error: crate::control::Error) -> Self {
        use crate::control::Error as Domain;
        use http::StatusCode;
        let status = match &error {
            Domain::Invalid(_) => StatusCode::BAD_REQUEST,
            Domain::Forbidden => StatusCode::FORBIDDEN,
            Domain::Missing => StatusCode::NOT_FOUND,
            Domain::Conflict(_) => StatusCode::CONFLICT,
            Domain::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Domain::Busy => StatusCode::TOO_MANY_REQUESTS,
        };
        Self(status, error.message())
    }
}
