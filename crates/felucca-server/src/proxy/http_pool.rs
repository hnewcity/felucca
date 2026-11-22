//! Backend pools are separated by route, session and original source address.
use crate::relay::Route;
use http::{Request, Response, Uri};
use hyper::body::Incoming;
use hyper_util::{
    client::legacy::{
        Client,
        connect::{Connected, Connection},
    },
    rt::{TokioExecutor, TokioIo, TokioTimer},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

type BackendClient = Client<Connector, crate::http::Body>;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct Key {
    trace: felucca_transport::local::RouteTrace,
    host_scope: Option<String>,
    client: felucca_core::ClientId,
    tunnel: u32,
    source: IpAddr,
    source_port: u16,
    destination: SocketAddr,
    target: Option<String>,
    origin: String,
}
struct Entry {
    client: BackendClient,
    touched: Instant,
}
#[derive(Default)]
pub struct HttpPool {
    entries: Mutex<HashMap<Key, Entry>>,
}
impl std::fmt::Debug for HttpPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpPool")
            .field("entries", &self.entries.lock().len())
            .finish()
    }
}
impl HttpPool {
    pub fn remove(&self, client: felucca_core::ClientId) {
        self.entries.lock().retain(|key, _| key.client != client);
    }
    fn client(
        &self,
        route: &Route,
        peer: SocketAddr,
        target: Option<String>,
        origin: String,
    ) -> BackendClient {
        let key = Key {
            trace: route.trace.clone(),
            host_scope: route.host_scope.clone(),
            client: route.client.id,
            tunnel: route.id,
            source: peer.ip(),
            // PROXY headers describe a transport peer, not an individual HTTP
            // request. Never reuse a connection for a different asserted peer.
            source_port: if route.options.proxy_protocol == felucca_core::config::ProxyVersion::Off
            {
                0
            } else {
                peer.port()
            },
            destination: route.destination,
            target: target.clone(),
            origin,
        };
        let mut entries = self.entries.lock();
        entries.retain(|_, entry| entry.touched.elapsed() < Duration::from_secs(120));
        if let Some(entry) = entries.get_mut(&key) {
            entry.touched = Instant::now();
            return entry.client.clone();
        }
        if entries.len() >= 256
            && let Some(key) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| key.clone())
        {
            entries.remove(&key);
        }
        let client = Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(8)
            .http2_only(route.options.http.backend_http2)
            .build(Connector {
                route: route.clone(),
                peer,
                target,
            });
        entries.insert(
            key,
            Entry {
                client: client.clone(),
                touched: Instant::now(),
            },
        );
        client
    }
    pub async fn request(
        &self,
        request: Request<crate::http::Body>,
        route: &Route,
        peer: SocketAddr,
        target: Option<String>,
    ) -> Result<Response<Incoming>, hyper_util::client::legacy::Error> {
        let origin = format!(
            "{}://{}",
            request.uri().scheme_str().unwrap_or("http"),
            request
                .uri()
                .authority()
                .map_or("", |authority| authority.as_str())
                .to_ascii_lowercase()
        );
        self.client(route, peer, target, origin)
            .request(request)
            .await
    }
}

#[derive(Clone)]
struct Connector {
    route: Route,
    peer: SocketAddr,
    target: Option<String>,
}
impl tower_service::Service<Uri> for Connector {
    type Response = BackendIo;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<BackendIo>> + Send>>;
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, _: Uri) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let io = this.route.metered(this.peer, this.target).await?;
            Ok(BackendIo {
                io: TokioIo::new(io),
                h2: this.route.options.http.backend_http2,
            })
        })
    }
}
struct BackendIo {
    io: TokioIo<tokio::io::DuplexStream>,
    h2: bool,
}
impl Connection for BackendIo {
    fn connected(&self) -> Connected {
        if self.h2 {
            Connected::new().negotiated_h2()
        } else {
            Connected::new()
        }
    }
}
impl hyper::rt::Read for BackendIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        hyper::rt::Read::poll_read(Pin::new(&mut self.get_mut().io), cx, buf)
    }
}
impl hyper::rt::Write for BackendIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        hyper::rt::Write::poll_write(Pin::new(&mut self.get_mut().io), cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        hyper::rt::Write::poll_flush(Pin::new(&mut self.get_mut().io), cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        hyper::rt::Write::poll_shutdown(Pin::new(&mut self.get_mut().io), cx)
    }
}
