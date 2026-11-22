//! Bounded in-process streams, carrying route context outside the wire protocol.
use std::{io, net::SocketAddr};
use tokio::{io::DuplexStream, sync::mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Default, Hash, PartialEq, Eq)]
pub struct RouteTrace(pub Vec<(String, u32)>);

impl RouteTrace {
    pub fn enter(&self, client: &str, id: u32) -> io::Result<Self> {
        if self.0.len() >= 16
            || self
                .0
                .iter()
                .any(|(key, route)| key == client && *route == id)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "nested route cycle or hop limit",
            ));
        }
        let mut next = self.clone();
        next.0.push((client.to_owned(), id));
        Ok(next)
    }
}

#[derive(Debug, Clone)]
pub struct LocalContext {
    pub peer: SocketAddr,
    pub trace: RouteTrace,
}

#[derive(Debug, Clone)]
pub struct LocalHandle {
    sender: mpsc::Sender<(DuplexStream, LocalContext)>,
    cancel: CancellationToken,
}

#[derive(Debug)]
pub struct LocalListener {
    receiver: mpsc::Receiver<(DuplexStream, LocalContext)>,
    cancel: CancellationToken,
}

pub fn channel(backlog: usize) -> (LocalHandle, LocalListener) {
    let (sender, receiver) = mpsc::channel(backlog.clamp(1, 4096));
    let cancel = CancellationToken::new();
    (
        LocalHandle {
            sender,
            cancel: cancel.clone(),
        },
        LocalListener { receiver, cancel },
    )
}

impl LocalHandle {
    pub async fn open(&self, context: LocalContext) -> io::Result<DuplexStream> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(io::ErrorKind::NotConnected.into()),
            permit = self.sender.reserve() => {
                let permit = permit.map_err(|_| io::Error::from(io::ErrorKind::NotConnected))?;
                // Allocate buffers only after admission; no unbounded pending stream queue.
                let (stream, incoming) = tokio::io::duplex(32 * 1024);
                permit.send((incoming, context));
                Ok(stream)
            }
        }
    }
    pub fn close(&self) {
        self.cancel.cancel();
    }
    pub fn is_closed(&self) -> bool {
        self.cancel.is_cancelled() || self.sender.is_closed()
    }
}
impl LocalListener {
    pub async fn accept(&mut self) -> Option<(DuplexStream, LocalContext)> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => None,
            stream = self.receiver.recv() => stream,
        }
    }
}
impl Drop for LocalListener {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn local_backpressure_and_cancellation_preserve_half_close_and_context() {
        let (handle, mut listener) = channel(1);
        let context = LocalContext {
            peer: "198.51.100.7:4321".parse().unwrap(),
            trace: RouteTrace::default().enter("local.office", 1).unwrap(),
        };
        let mut first = handle.open(context.clone()).await.unwrap();
        let second = handle.open(context.clone());
        tokio::pin!(second);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut second)
                .await
                .is_err()
        );
        let (mut backend, received) = listener.accept().await.unwrap();
        assert_eq!(received.peer, context.peer);
        assert_eq!(received.trace, context.trace);
        let _second = second.await.unwrap();
        first.write_all(b"request").await.unwrap();
        first.shutdown().await.unwrap();
        let mut data = Vec::new();
        backend.read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"request");
        backend.write_all(b"response").await.unwrap();
        backend.shutdown().await.unwrap();
        data.clear();
        first.read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"response");
        let waiting = handle.open(context);
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        drop(listener);
        assert!(waiting.await.is_err());
        assert!(handle.is_closed());
    }

    #[test]
    fn route_history_rejects_cycles_and_caps_allocation() {
        let mut trace = RouteTrace::default();
        for id in 1..=16 {
            trace = trace.enter("client", id).unwrap();
        }
        assert!(trace.enter("client", 17).is_err());
        assert!(
            RouteTrace::default()
                .enter("one", 1)
                .unwrap()
                .enter("one", 1)
                .is_err()
        );
        assert!(
            RouteTrace::default()
                .enter("one", 1)
                .unwrap()
                .enter("two", 1)
                .is_ok()
        );
    }
}
