use crate::file_fs::ConfinedFs;
use base64::Engine;
use dav_server::{DavHandler, DavMethodSet, memls::MemLs};
use felucca_core::{access::Authentication, config::file::FileConfig};
use http::{HeaderMap, HeaderValue, Response, StatusCode, header};
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use std::{convert::Infallible, io, time::Duration};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Body = UnsyncBoxBody<bytes::Bytes, Error>;

#[derive(Clone)]
pub(crate) struct FileServer {
    handler: DavHandler,
    auth: Authentication,
}
impl std::fmt::Debug for FileServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileServer")
            .field("authenticated", &self.auth.required())
            .finish()
    }
}
impl FileServer {
    pub fn new(config: &FileConfig) -> io::Result<Self> {
        let fs = ConfinedFs::open_root(&config.root, config.read_only)?;
        Self::from_fs(config, fs)
    }
    pub(crate) fn from_fs(config: &FileConfig, fs: ConfinedFs) -> io::Result<Self> {
        let mut handler = DavHandler::builder()
            .filesystem(Box::new(fs))
            .locksystem(MemLs::new())
            .strip_prefix(config.prefix.trim_end_matches('/'))
            .autoindex(config.auto_index)
            .indexfile("index.html")
            .hide_symlinks(true)
            .read_buf_size(64 * 1024);
        if config.read_only {
            handler = handler.methods(
                DavMethodSet::from_vec(vec!["GET", "HEAD", "PROPFIND", "OPTIONS"])
                    .expect("static read-only DAV methods"),
            );
        }
        Ok(Self {
            handler: handler.build_handler(),
            auth: config.auth.clone(),
        })
    }
    pub async fn serve<T: felucca_transport::Conn + 'static>(
        &self,
        io: felucca_transport::payload::PayloadStream<T>,
    ) {
        let file = self.clone();
        let service = hyper::service::service_fn(move |mut request: http::Request<Incoming>| {
            let file = file.clone();
            async move {
                if !authorized(request.headers(), &file.auth) {
                    let mut response = Response::new(
                        Full::new(bytes::Bytes::from_static(b"authentication required"))
                            .map_err(|never: Infallible| -> Error { match never {} })
                            .boxed_unsync(),
                    );
                    *response.status_mut() = StatusCode::UNAUTHORIZED;
                    response.headers_mut().insert(
                        header::WWW_AUTHENTICATE,
                        HeaderValue::from_static(
                            "Basic realm=\"felucca files\", charset=\"UTF-8\"",
                        ),
                    );
                    return Ok::<Response<Body>, Infallible>(response);
                }
                // A file handler has no upstream that should receive credentials.
                request.headers_mut().remove(header::AUTHORIZATION);
                let response = file.handler.handle(request).await.map(|body| {
                    body.map_err(|error| -> Error { Box::new(error) })
                        .boxed_unsync()
                });
                Ok(response)
            }
        });
        let mut builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(Duration::from_secs(15))
            .max_buf_size(32 * 1024);
        builder
            .http2()
            .max_concurrent_streams(64)
            .max_header_list_size(32 * 1024);
        if let Err(error) = builder.serve_connection(TokioIo::new(io), service).await {
            tracing::debug!(%error, "file connection ended");
        }
    }
}
fn authorized(headers: &HeaderMap, auth: &Authentication) -> bool {
    if !auth.required() {
        return true;
    }
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    if values.next().is_some() || value.len() > 1024 {
        return false;
    }
    let Some((scheme, token)) = value.split_once(' ') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return false;
    }
    let Ok(value) = base64::engine::general_purpose::STANDARD.decode(token.trim()) else {
        return false;
    };
    let Some(colon) = value.iter().position(|byte| *byte == b':') else {
        return false;
    };
    auth.verify(&value[..colon], &value[colon + 1..])
}
