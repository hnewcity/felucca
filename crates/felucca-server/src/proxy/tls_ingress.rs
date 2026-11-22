//! Retain ClientHello bytes until SNI routing decides whether to terminate TLS.
use bytes::Bytes;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct Captured<T> {
    io: T,
    prefix: Vec<u8>,
    recording: bool,
}
impl<T> Captured<T> {
    pub fn new(io: T) -> Self {
        Self {
            io,
            prefix: Vec::new(),
            recording: true,
        }
    }
    pub fn finish(self) -> (Bytes, T) {
        (Bytes::from(self.prefix), self.io)
    }
    pub fn stop(&mut self) {
        self.recording = false;
        self.prefix.clear();
    }
}
impl<T: AsyncRead + Unpin> AsyncRead for Captured<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.io).poll_read(cx, buf);
        if this.recording && matches!(result, Poll::Ready(Ok(()))) {
            let read = &buf.filled()[before..];
            if this.prefix.len() + read.len() > 65536 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ClientHello exceeds 64 KiB",
                )));
            }
            this.prefix.extend_from_slice(read);
        }
        result
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for Captured<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().io).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}
