//! Ticket-scoped UDP rendezvous. Equal-size authenticated messages and a
//! return-path cookie precede accepting an observed endpoint.
use felucca_core::Secret;
use ring::hmac;
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{net::UdpSocket, sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;

const LEN: usize = 256;
const SIGN: usize = 97;
const MAC_END: usize = SIGN + 32;
const MAGIC: &[u8; 4] = b"\0FP1";
const PROBE: u8 = 1;
const COOKIE: u8 = 2;
const RETURN: u8 = 3;
const OBSERVED: u8 = 4;
const COOKIE_TTL: u64 = 5;
pub type TicketId = [u8; 16];

#[derive(Debug)]
struct Entry {
    keys: [hmac::Key; 2],
    observed: [watch::Sender<Option<SocketAddr>>; 2],
    expires: Instant,
    remaining: std::sync::atomic::AtomicU32,
}
#[derive(Debug)]
pub struct Broker {
    socket: Arc<UdpSocket>,
    entries: Mutex<HashMap<TicketId, Arc<Entry>>>,
    cookie_key: hmac::Key,
    started: Instant,
    max_pending: usize,
}
#[derive(Debug)]
pub struct Registration {
    broker: Weak<Broker>,
    id: TicketId,
    entry: Arc<Entry>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(broker) = self.broker.upgrade() {
            let mut entries = broker.entries.lock().expect("broker lock");
            if entries
                .get(&self.id)
                .is_some_and(|current| Arc::ptr_eq(current, &self.entry))
            {
                entries.remove(&self.id);
            }
        }
    }
}
impl Registration {
    pub async fn observed(&self, role: u8) -> io::Result<SocketAddr> {
        let sender = self
            .entry
            .observed
            .get(role as usize)
            .ok_or(io::ErrorKind::InvalidInput)?;
        let mut receiver = sender.subscribe();
        tokio::time::timeout_at(self.entry.expires, async {
            loop {
                if let Some(address) = *receiver.borrow_and_update() {
                    return Ok(address);
                }
                receiver
                    .changed()
                    .await
                    .map_err(|_| io::ErrorKind::BrokenPipe)?;
            }
        })
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
    }
}
impl Broker {
    pub async fn bind(address: SocketAddr, max_pending: usize) -> io::Result<Arc<Self>> {
        if !(1..=4096).contains(&max_pending) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut random = [0; 32];
        rand::fill(&mut random);
        let socket = UdpSocket::bind(address).await?;
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            entries: Mutex::new(HashMap::new()),
            cookie_key: hmac::Key::new(hmac::HMAC_SHA256, &random),
            started: Instant::now(),
            max_pending,
        }))
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
    pub fn register(
        self: &Arc<Self>,
        id: TicketId,
        keys: [&Secret; 2],
        ttl: Duration,
    ) -> io::Result<Registration> {
        if ttl.is_zero() || ttl > Duration::from_secs(30) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut entries = self.entries.lock().expect("broker lock");
        entries.retain(|_, entry| entry.expires > Instant::now());
        if entries.len() >= self.max_pending || entries.contains_key(&id) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let entry = Arc::new(Entry {
            keys: keys.map(|key| hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes())),
            observed: std::array::from_fn(|_| watch::channel(None).0),
            expires: Instant::now() + ttl,
            remaining: std::sync::atomic::AtomicU32::new(128),
        });
        entries.insert(id, entry.clone());
        Ok(Registration {
            broker: Arc::downgrade(self),
            id,
            entry,
        })
    }
    pub async fn run(self: Arc<Self>, cancel: CancellationToken) -> io::Result<()> {
        let mut buffer = [0; LEN + 1];
        let mut receiver = crate::udp::Receiver::new();
        loop {
            let (len, source) = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                received = receiver.recv_from(&self.socket, &mut buffer) => received,
            };
            if len != LEN {
                continue;
            }
            if let Some(reply) = self.handle(&buffer[..len], source) {
                // One response exactly as large as the request; no task/queue
                // per packet and no retries on socket congestion.
                match self.socket.try_send_to(&reply, source) {
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => tracing::debug!(%error, "rendezvous reply failed"),
                }
            }
        }
    }
    fn handle(&self, packet: &[u8], source: SocketAddr) -> Option<[u8; LEN]> {
        let mut reply: [u8; LEN] = packet.try_into().ok()?;
        if &packet[..4] != MAGIC || packet[5] > 1 || packet[MAC_END..].iter().any(|b| *b != 0) {
            return None;
        }
        let id: TicketId = packet[6..22].try_into().ok()?;
        let entry = self.entries.lock().expect("broker lock").get(&id)?.clone();
        if Instant::now() >= entry.expires {
            return None;
        }
        let role = packet[5] as usize;
        if hmac::verify(&entry.keys[role], &packet[..SIGN], &packet[SIGN..MAC_END]).is_err() {
            return None;
        }
        if entry
            .remaining
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_sub(1),
            )
            .is_err()
        {
            return None;
        }
        let now = self.started.elapsed().as_secs();
        match packet[4] {
            PROBE => {
                reply[4] = COOKIE;
                reply[38..46].copy_from_slice(&now.to_be_bytes());
                let cookie = self.cookie(source, &reply[..46]);
                reply[46..78].copy_from_slice(cookie.as_ref());
            }
            RETURN => {
                let stamp = u64::from_be_bytes(packet[38..46].try_into().ok()?);
                if stamp > now || now - stamp > COOKIE_TTL {
                    return None;
                }
                // The challenge was signed with kind COOKIE. Reconstruct that
                // input before verifying the source-bound cookie.
                let mut challenge = reply;
                challenge[4] = COOKIE;
                let input = cookie_input(source, &challenge[..46]);
                if hmac::verify(&self.cookie_key, &input, &packet[46..78]).is_err() {
                    return None;
                }
                let mut address = *entry.observed[role].borrow();
                if address.is_some_and(|address| address != source) {
                    return None;
                }
                if address.is_none() {
                    address = Some(source);
                    entry.observed[role].send_replace(address);
                }
                reply[4] = OBSERVED;
                encode_address(source, &mut reply[78..97]);
            }
            _ => return None,
        }
        sign(&mut reply, &entry.keys[role]);
        Some(reply)
    }
    fn cookie(&self, source: SocketAddr, prefix: &[u8]) -> hmac::Tag {
        hmac::sign(&self.cookie_key, &cookie_input(source, prefix))
    }
}
fn cookie_input(source: SocketAddr, prefix: &[u8]) -> Vec<u8> {
    let mut input = b"felucca-p2p-cookie-v1".to_vec();
    input.extend_from_slice(prefix);
    let mut addr = [0; 19];
    encode_address(source, &mut addr);
    input.extend_from_slice(&addr);
    input
}
fn sign(packet: &mut [u8; LEN], key: &hmac::Key) {
    let mac = hmac::sign(key, &packet[..SIGN]);
    packet[SIGN..MAC_END].copy_from_slice(mac.as_ref());
}
fn encode_address(address: SocketAddr, bytes: &mut [u8]) {
    bytes.fill(0);
    match address.ip() {
        IpAddr::V4(ip) => {
            bytes[0] = 4;
            bytes[1..5].copy_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            bytes[0] = 6;
            bytes[1..17].copy_from_slice(&ip.octets());
        }
    }
    bytes[17..19].copy_from_slice(&address.port().to_be_bytes());
}
fn decode_address(bytes: &[u8]) -> Option<SocketAddr> {
    let ip = match bytes[0] {
        4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[1..5]).ok()?)),
        6 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[1..17]).ok()?)),
        _ => return None,
    };
    let port = u16::from_be_bytes(bytes[17..19].try_into().ok()?);
    if port == 0 || ip.is_unspecified() || ip.is_multicast() {
        return None;
    }
    Some(SocketAddr::new(ip, port))
}

/// Observe an address using the same socket subsequently handed to QUIC.
/// Only authenticated responses from the specified broker may change state.
pub async fn observe(
    broker: SocketAddr,
    local: IpAddr,
    id: TicketId,
    role: u8,
    key: &Secret,
    timeout: Duration,
) -> io::Result<(std::net::UdpSocket, SocketAddr)> {
    if role > 1
        || broker.is_ipv4() != local.is_ipv4()
        || timeout.is_zero()
        || timeout > Duration::from_secs(30)
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let socket = UdpSocket::bind(SocketAddr::new(local, 0)).await?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes());
    let mut request = [0; LEN];
    request[..4].copy_from_slice(MAGIC);
    request[4] = PROBE;
    request[5] = role;
    request[6..22].copy_from_slice(&id);
    rand::fill(&mut request[22..38]);
    sign(&mut request, &key);
    let observed = tokio::time::timeout(timeout, async {
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut bytes = [0; LEN + 1];
        let mut receiver = crate::udp::Receiver::new();
        loop {
            let (size, source) = tokio::select! {
                _ = tick.tick()=>{socket.send_to(&request,broker).await?;continue;},
                received=receiver.recv_from(&socket, &mut bytes)=>received,
            };
            if source != broker
                || size != LEN
                || &bytes[..4] != MAGIC
                || bytes[5..38] != request[5..38]
                || bytes[MAC_END..LEN].iter().any(|b| *b != 0)
                || hmac::verify(&key, &bytes[..SIGN], &bytes[SIGN..MAC_END]).is_err()
            {
                continue;
            }
            match bytes[4] {
                COOKIE => {
                    request[4] = RETURN;
                    request[38..78].copy_from_slice(&bytes[38..78]);
                    sign(&mut request, &key);
                    socket.send_to(&request, broker).await?;
                }
                OBSERVED => {
                    if request[4] != RETURN || bytes[38..78] != request[38..78] {
                        continue;
                    }
                    if let Some(address) = decode_address(&bytes[78..97])
                        .filter(|address| address.is_ipv4() == local.is_ipv4())
                    {
                        return Ok::<_, io::Error>(address);
                    }
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    Ok((socket.into_std()?, observed))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn keys() -> [Secret; 2] {
        ["provider-probe-key".into(), "visitor-probe-key".into()]
    }
    #[tokio::test]
    async fn observes_each_role_only_after_an_authenticated_return_path_and_releases_capacity() {
        let broker = Broker::bind("127.0.0.1:0".parse().unwrap(), 1)
            .await
            .unwrap();
        let keys = keys();
        let id = [1; 16];
        let entry = broker
            .register(id, [&keys[0], &keys[1]], Duration::from_secs(5))
            .unwrap();
        assert!(
            broker
                .register([2; 16], [&keys[0], &keys[1]], Duration::from_secs(5))
                .is_err()
        );
        let stop = CancellationToken::new();
        let task = tokio::spawn(broker.clone().run(stop.clone()));
        for role in 0..=1 {
            let (socket, address) = observe(
                broker.local_addr().unwrap(),
                "127.0.0.1".parse().unwrap(),
                id,
                role,
                &keys[role as usize],
                Duration::from_secs(2),
            )
            .await
            .unwrap();
            assert_eq!(socket.local_addr().unwrap(), address);
            assert_eq!(entry.observed(role).await.unwrap(), address);
        }
        drop(entry);
        assert!(broker.entries.lock().unwrap().is_empty());
        assert!(
            broker
                .register(id, [&keys[0], &keys[1]], Duration::from_secs(5))
                .is_ok()
        );
        stop.cancel();
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn spoofed_return_wrong_role_key_and_replayed_ticket_never_publish_an_address() {
        let broker = Broker::bind("127.0.0.1:0".parse().unwrap(), 2)
            .await
            .unwrap();
        let keys = keys();
        let id = [7; 16];
        let entry = broker
            .register(id, [&keys[0], &keys[1]], Duration::from_secs(5))
            .unwrap();
        let source = "127.0.0.1:4567".parse().unwrap();
        let other = "127.0.0.1:4568".parse().unwrap();
        let key = hmac::Key::new(hmac::HMAC_SHA256, keys[0].as_bytes());
        let mut request = [0; LEN];
        request[..4].copy_from_slice(MAGIC);
        request[4] = PROBE;
        request[6..22].copy_from_slice(&id);
        request[22..38].fill(9);
        sign(&mut request, &key);
        let challenge = broker.handle(&request, source).unwrap();
        assert_eq!(challenge[4], COOKIE);
        assert!(entry.entry.observed[0].borrow().is_none());
        let mut response = challenge;
        response[4] = RETURN;
        sign(&mut response, &key);
        assert!(broker.handle(&response, other).is_none());
        assert!(entry.entry.observed[0].borrow().is_none());
        let mut tampered = response;
        tampered[5] = 1;
        sign(&mut tampered, &key);
        assert!(broker.handle(&tampered, source).is_none());
        tampered = response;
        tampered[46] ^= 1;
        sign(&mut tampered, &key);
        assert!(broker.handle(&tampered, source).is_none());
        assert_eq!(broker.handle(&response, source).unwrap()[4], OBSERVED);
        drop(entry);
        assert!(broker.handle(&response, source).is_none());
        // A different registration sharing the id must not accept the old cookie.
        // Ticket ids are never reused by the broker owner; keys provide a second boundary.
        let other_keys: [Secret; 2] = ["new-provider-key".into(), "new-visitor-key".into()];
        let _next = broker
            .register(id, [&other_keys[0], &other_keys[1]], Duration::from_secs(5))
            .unwrap();
        assert!(broker.handle(&response, source).is_none());
    }
    #[tokio::test(start_paused = true)]
    async fn cookies_and_tickets_expire_and_packet_budget_stays_bounded() {
        let broker = Broker::bind("127.0.0.1:0".parse().unwrap(), 1)
            .await
            .unwrap();
        let keys = keys();
        let id = [3; 16];
        let _entry = broker
            .register(id, [&keys[0], &keys[1]], Duration::from_secs(15))
            .unwrap();
        let source = "127.0.0.1:4444".parse().unwrap();
        let key = hmac::Key::new(hmac::HMAC_SHA256, keys[0].as_bytes());
        let mut packet = [0; LEN];
        packet[..4].copy_from_slice(MAGIC);
        packet[4] = PROBE;
        packet[6..22].copy_from_slice(&id);
        sign(&mut packet, &key);
        let mut returned = broker.handle(&packet, source).unwrap();
        returned[4] = RETURN;
        sign(&mut returned, &key);
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(broker.handle(&returned, source).is_none());
        for _ in 0..126 {
            assert!(broker.handle(&packet, source).is_some());
        }
        assert!(broker.handle(&packet, source).is_none());
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(broker.handle(&packet, source).is_none());
        assert!(
            broker
                .register([4; 16], [&keys[0], &keys[1]], Duration::from_secs(1))
                .is_ok()
        );
    }
}
