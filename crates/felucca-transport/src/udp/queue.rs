//! Nonblocking datagram queues bounded by packets, per-peer bytes and shared bytes.
use std::{ops::Deref, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

#[derive(Debug)]
pub struct Packet {
    data: Vec<u8>,
    _local: OwnedSemaphorePermit,
    _shared: OwnedSemaphorePermit,
}

impl Deref for Packet {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

// Charge empty datagrams too, including an allowance for channel bookkeeping.
const ENTRY_BYTES: usize = std::mem::size_of::<Packet>() + 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullOrClosed {
    Full,
    Closed,
}

#[derive(Debug)]
pub struct Sender {
    packets: mpsc::Sender<Packet>,
    local: Arc<Semaphore>,
    shared: Arc<Semaphore>,
}

pub fn channel(
    max_packets: usize,
    max_bytes: usize,
    shared: Arc<Semaphore>,
) -> (Sender, mpsc::Receiver<Packet>) {
    let (packets, receiver) = mpsc::channel(max_packets);
    (
        Sender {
            packets,
            local: Arc::new(Semaphore::new(max_bytes)),
            shared,
        },
        receiver,
    )
}

impl Sender {
    pub fn is_closed(&self) -> bool {
        self.packets.is_closed()
    }

    /// Reserve all limits before copying; a rejected datagram allocates nothing.
    pub fn try_send(&self, data: &[u8]) -> Result<(), FullOrClosed> {
        let slot = self.packets.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => FullOrClosed::Full,
            mpsc::error::TrySendError::Closed(_) => FullOrClosed::Closed,
        })?;
        let charged = data
            .len()
            .checked_add(ENTRY_BYTES)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(FullOrClosed::Full)?;
        let local = self
            .local
            .clone()
            .try_acquire_many_owned(charged)
            .map_err(|_| FullOrClosed::Full)?;
        let shared = self
            .shared
            .clone()
            .try_acquire_many_owned(charged)
            .map_err(|_| FullOrClosed::Full)?;
        slot.send(Packet {
            data: data.to_vec(),
            _local: local,
            _shared: shared,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bursts_preserve_payloads_and_release_credit_only_after_forwarding() {
        let shared = Arc::new(Semaphore::new(1024 * 1024));
        let (send, mut receive) = channel(256, 512 * 1024, shared.clone());
        for i in 0..200u16 {
            let mut data = vec![42; 1400];
            data[..2].copy_from_slice(&i.to_be_bytes());
            send.try_send(&data).unwrap();
        }
        for i in 0..200u16 {
            let data = receive.recv().await.unwrap();
            assert_eq!(data.len(), 1400);
            assert_eq!(&data[..2], i.to_be_bytes());
            assert!(data[2..].iter().all(|&b| b == 42));
        }
        assert_eq!(shared.available_permits(), 1024 * 1024);
        send.try_send(b"held while writing").unwrap();
        let held = receive.recv().await.unwrap();
        drop(receive);
        assert!(shared.available_permits() < 1024 * 1024);
        assert_eq!(send.try_send(b"closed"), Err(FullOrClosed::Closed));
        drop(held);
        assert_eq!(shared.available_permits(), 1024 * 1024);
    }

    #[test]
    fn byte_limits_bound_large_and_empty_packets_across_peers() {
        let charged = 65_507 + ENTRY_BYTES;
        let shared = Arc::new(Semaphore::new(charged * 3));
        let (a, ar) = channel(256, charged * 2, shared.clone());
        let (b, br) = channel(256, charged * 2, shared.clone());
        let data = vec![1; 65_507];
        a.try_send(&data).unwrap();
        a.try_send(&data).unwrap();
        assert_eq!(a.try_send(&data), Err(FullOrClosed::Full));
        b.try_send(&data).unwrap();
        assert_eq!(b.try_send(b""), Err(FullOrClosed::Full));
        drop(ar);
        b.try_send(&data).unwrap();
        drop(br);
        assert_eq!(shared.available_permits(), charged * 3);

        let (empty, receiver) = channel(256, ENTRY_BYTES * 2, shared.clone());
        empty.try_send(b"").unwrap();
        empty.try_send(b"").unwrap();
        assert_eq!(empty.try_send(b""), Err(FullOrClosed::Full));
        drop(receiver);
        assert_eq!(shared.available_permits(), charged * 3);
    }
}
