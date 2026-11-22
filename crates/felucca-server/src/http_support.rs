//! HTTP body adapters and connection serving shared by management and proxies.
use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use std::{
    convert::Infallible,
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type Body = UnsyncBoxBody<Bytes, BoxError>;

pub fn incoming(body: Incoming) -> Body {
    body.map_err(|error| -> BoxError { Box::new(error) })
        .boxed_unsync()
}

pub fn response(status: StatusCode, text: impl Into<Bytes>) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(
            Full::new(text.into())
                .map_err(|e: Infallible| -> BoxError { match e {} })
                .boxed_unsync(),
        )
        .expect("static status and body")
}

pub async fn serve<T, F, Fut>(io: T, handler: F)
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: Fn(Request<Incoming>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Response<Body>> + Send + 'static,
{
    let handler = Arc::new(handler);
    let service = hyper::service::service_fn(move |request| {
        let handler = handler.clone();
        async move { Ok::<_, Infallible>(handler(request).await) }
    });
    let mut builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(15))
        .max_buf_size(32 * 1024)
        .half_close(true);
    // PING an idle HTTP/2 connection so one whose client slept or roamed away
    // is closed instead of holding a listener slot for months.
    builder
        .http2()
        .timer(TokioTimer::new())
        .keep_alive_interval(Some(Duration::from_secs(30)))
        .keep_alive_timeout(Duration::from_secs(20))
        .enable_connect_protocol()
        .max_concurrent_streams(128)
        .max_header_list_size(32 * 1024);
    let io = FirstRead {
        io,
        deadline: Some(Box::pin(tokio::time::sleep(REQUEST_START))),
    };
    let _ = builder
        .serve_connection_with_upgrades(TokioIo::new(io), service)
        .await;
}

/// How long a new connection may stay silent before its first byte.
const REQUEST_START: Duration = Duration::from_secs(15);

/// Fails a connection whose client sends nothing within [`REQUEST_START`].
///
/// hyper's `header_read_timeout` starts only once the auto builder has read
/// enough to choose HTTP/1 or HTTP/2. A client that connects, or completes TLS,
/// and then stays silent would otherwise hold a descriptor and task forever.
struct FirstRead<T> {
    io: T,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<T: AsyncRead + Unpin> AsyncRead for FirstRead<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let before = buf.filled().len();
        let poll = Pin::new(&mut this.io).poll_read(cx, buf);
        if let Some(deadline) = this.deadline.as_mut() {
            match poll {
                Poll::Ready(Ok(())) if buf.filled().len() > before => this.deadline = None,
                Poll::Pending if deadline.as_mut().poll(cx).is_ready() => {
                    return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
                }
                _ => {}
            }
        }
        poll
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for FirstRead<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(start_paused = true)]
    async fn a_silent_connection_is_closed_after_the_request_start_deadline() {
        let (client, server) = tokio::io::duplex(1024);
        let served = tokio::spawn(serve(server, |_| async { response(StatusCode::OK, "ok") }));
        tokio::time::timeout(REQUEST_START * 2, served)
            .await
            .expect("a silent client must not hold the connection")
            .unwrap();
        drop(client);
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_after_a_pause_shorter_than_the_deadline_is_served() {
        let (mut client, server) = tokio::io::duplex(4096);
        tokio::spawn(serve(server, |_| async { response(StatusCode::OK, "ok") }));
        tokio::time::sleep(REQUEST_START / 2).await;
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).await.unwrap();
        assert!(reply.starts_with(b"HTTP/1.1 200"));
    }
}
