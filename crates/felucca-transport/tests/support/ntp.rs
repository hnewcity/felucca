use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::net::UdpSocket;

pub struct Ntp {
    pub address: SocketAddr,
    pub hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Ntp {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Ntp {
    pub async fn start(offset_seconds: i64, stale_first: bool) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        let task = tokio::spawn(async move {
            let mut request = [0; 512];
            loop {
                let (len, peer) = socket.recv_from(&mut request).await.unwrap();
                assert_eq!(len, 48);
                assert_eq!(request[0] & 7, 3);
                count.fetch_add(1, Ordering::SeqCst);
                let mut packet = [0; 48];
                packet[0] = (4 << 3) | 4;
                packet[1] = 2;
                packet[24..32].copy_from_slice(&request[40..48]);
                let stamp = timestamp(offset_seconds);
                packet[32..40].copy_from_slice(&stamp);
                packet[40..48].copy_from_slice(&stamp);
                if stale_first {
                    let mut stale = packet;
                    stale[24] ^= 1;
                    socket.send_to(&stale, peer).await.unwrap();
                }
                socket.send_to(&packet, peer).await.unwrap();
            }
        });
        Self {
            address,
            hits,
            task,
        }
    }
}
fn timestamp(offset: i64) -> [u8; 8] {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let seconds = (now.as_secs() as i64 + 2_208_988_800 + offset) as u32;
    let fraction = ((now.subsec_nanos() as u64 * (1u64 << 32)) / 1_000_000_000) as u32;
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&seconds.to_be_bytes());
    bytes[4..].copy_from_slice(&fraction.to_be_bytes());
    bytes
}
