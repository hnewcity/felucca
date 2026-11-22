//! A loopback DNS authority for resolver/agent tests, with optional UDP truncation.
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UdpSocket},
    task::JoinSet,
};

pub struct Dns {
    pub address: SocketAddr,
    pub hits: Arc<Mutex<Vec<String>>>,
    pub tcp_hits: Arc<AtomicUsize>,
    _tasks: JoinSet<()>,
}
fn answer(query: &[u8], truncated: bool, hits: &Mutex<Vec<String>>) -> Vec<u8> {
    assert!(query.len() >= 12);
    assert_eq!(&query[4..6], &[0, 1]);
    let mut cursor = 12;
    let mut labels = Vec::new();
    while query[cursor] != 0 {
        let size = usize::from(query[cursor]);
        cursor += 1;
        labels.push(String::from_utf8_lossy(&query[cursor..cursor + size]).to_ascii_lowercase());
        cursor += size;
    }
    cursor += 1;
    let kind = u16::from_be_bytes(query[cursor..cursor + 2].try_into().unwrap());
    cursor += 4;
    hits.lock().unwrap().push(labels.join("."));
    let data = if truncated {
        Vec::new()
    } else {
        match kind {
            1 => vec![127, 0, 0, 1],
            28 => std::net::Ipv6Addr::LOCALHOST.octets().to_vec(),
            _ => Vec::new(),
        }
    };
    let mut response = query[..2].to_vec();
    response.extend_from_slice(&[
        0x81 | if truncated { 2 } else { 0 },
        0x80,
        0,
        1,
        0,
        u8::from(!data.is_empty()),
        0,
        0,
        0,
        0,
    ]);
    response.extend_from_slice(&query[12..cursor]);
    if !data.is_empty() {
        response.extend_from_slice(&[0xc0, 0x0c]);
        response.extend_from_slice(&kind.to_be_bytes());
        response.extend_from_slice(&[0, 1, 0, 0, 0, 60, 0, data.len() as u8]);
        response.extend_from_slice(&data);
    }
    response
}
impl Dns {
    pub async fn start(truncate: bool) -> Self {
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = udp.local_addr().unwrap();
        let tcp = TcpListener::bind(address).await.unwrap();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let tcp_hits = Arc::new(AtomicUsize::new(0));
        let mut tasks = JoinSet::new();
        let udp_hits = hits.clone();
        tasks.spawn(async move {
            let mut buffer = vec![0; 4096];
            while let Ok((n, peer)) = udp.recv_from(&mut buffer).await {
                let response = answer(&buffer[..n], truncate, &udp_hits);
                let _ = udp.send_to(&response, peer).await;
            }
        });
        let queries = hits.clone();
        let count = tcp_hits.clone();
        tasks.spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _ = connections.join_next(), if !connections.is_empty() => {},
                    accepted = tcp.accept() => {
                        let Ok((mut io, _)) = accepted else { break; };
                        let hits = queries.clone();
                        let count = count.clone();
                        connections.spawn(async move {
                            while let Ok(size) = io.read_u16().await {
                                if size > 4096 { break; }
                                let mut query = vec![0; usize::from(size)];
                                if io.read_exact(&mut query).await.is_err() { break; }
                                count.fetch_add(1, Ordering::SeqCst);
                                let response = answer(&query, false, &hits);
                                if io.write_u16(response.len() as u16).await.is_err() { break; }
                                if io.write_all(&response).await.is_err() { break; }
                            }
                        });
                    },
                }
            }
        });
        Self {
            address,
            hits,
            tcp_hits,
            _tasks: tasks,
        }
    }
}
