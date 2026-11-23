//! Shared process, network and configuration fixtures.
pub(crate) mod admin;
pub(crate) mod platform;
pub(crate) mod visitor;
use felucca_client::Agent;
use felucca_core::config::TlsConfig;
use felucca_core::{AgentConfig, ServerConfig, TransportKind, TrustPolicy};
use felucca_server::Server;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
/// A loopback port that is currently free for both TCP and UDP.
///
/// `bind(0)` draws from the ephemeral range, which outbound connections and
/// client UDP sockets share, so a port released there can be taken by another
/// test's `connect` before the fixture binds it. Allocate below every
/// platform's ephemeral range instead (Linux starts at 32768, macOS and Windows
/// at 49152) and never hand out the same port twice in one process.
pub(crate) async fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU32, Ordering};
    const FIRST: u32 = 20_000;
    const SPAN: u32 = 12_000;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    // Start each test binary at a different offset.
    let base = std::process::id() % SPAN;
    for _ in 0..SPAN {
        let port = (FIRST + (base + NEXT.fetch_add(1, Ordering::Relaxed)) % SPAN) as u16;
        let Ok(tcp) = TcpListener::bind(("127.0.0.1", port)).await else {
            continue;
        };
        let Ok(udp) = tokio::net::UdpSocket::bind(("127.0.0.1", port)).await else {
            continue;
        };
        drop((tcp, udp));
        return port;
    }
    panic!("no free loopback port between {FIRST} and {}", FIRST + SPAN);
}
pub(crate) fn agent(tunnels: &str) -> AgentConfig {
    let mut config: AgentConfig = toml::from_str(&format!("server='127.0.0.1:1'\nvkey='feature-secret'\nkey_id='features'\nreconnect_min='30ms'\nreconnect_max='100ms'\n{tunnels}")).unwrap();
    config.validate().unwrap();
    config
}
pub(crate) fn server_config() -> ServerConfig {
    toml::from_str("[bridge]\nbind='127.0.0.1:0'\n[client.features]\nvkey='feature-secret'\nallow_config_push=true\n").unwrap()
}
pub(crate) struct Running {
    pub(crate) server: Arc<Server>,
    pub(crate) agent: Arc<Agent>,
    pub(crate) agent_task: tokio::task::JoinHandle<()>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.agent.shutdown();
        self.server.shutdown();
        self.agent_task.abort();
    }
}
pub(crate) async fn start(
    mut server_config: ServerConfig,
    mut config: AgentConfig,
    transport: TransportKind,
) -> Running {
    let quic = if matches!(transport, TransportKind::Quic | TransportKind::Kcp) {
        let configured = if transport == TransportKind::Kcp {
            &server_config.bridge.kcp_bind
        } else {
            &server_config.bridge.quic_bind
        };
        let port = if let Some(bind) = configured {
            felucca_core::config::parse_bind(bind).unwrap().port()
        } else {
            free_port().await
        };
        if transport == TransportKind::Kcp {
            server_config.bridge.kcp_bind = Some(format!("127.0.0.1:{port}"));
        } else {
            server_config.bridge.quic_bind = Some(format!("127.0.0.1:{port}"));
        }
        Some(port)
    } else {
        None
    };
    let server = Arc::new(Server::new(server_config).unwrap());
    let port = server.clone().run().await.unwrap();
    config.server = format!("127.0.0.1:{}", quic.unwrap_or(port));
    config.transport = transport;
    config.trust = TrustPolicy::Pinned {
        fingerprint: server.fingerprint(),
    };
    let agent = Arc::new(Agent::new(config).unwrap());
    let agent_task = tokio::spawn(agent.clone().run());
    tokio::time::timeout(Duration::from_secs(10), async {
        while !agent.is_ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent did not register");
    Running {
        server,
        agent,
        agent_task,
    }
}
pub(crate) async fn echo() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut io, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = io.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}
pub(crate) async fn roundtrip(mut io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin) {
    io.write_all(b"hello").await.unwrap();
    io.shutdown().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), io.read_to_end(&mut response))
        .await
        .expect("echo timed out")
        .unwrap();
    assert_eq!(response, b"hello");
}
pub(crate) async fn head(io: &mut (impl tokio::io::AsyncRead + Unpin)) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(io.read_u8().await.unwrap());
        assert!(head.len() < 32 * 1024);
    }
    String::from_utf8(head).unwrap()
}
pub(crate) async fn web_backend(body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut io, _)) = listener.accept().await {
            tokio::spawn(async move {
                let request = head(&mut io).await;
                let response = format!("{body}\n{request}");
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                io.write_all(header.as_bytes()).await.unwrap();
                io.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    port
}
pub(crate) async fn http_request(port: u16, request: &str) -> String {
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(5), io.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}
pub(crate) struct TestIdentity {
    pub(crate) root: std::path::PathBuf,
    pub(crate) identity: felucca_transport::Identity,
    pub(crate) config: TlsConfig,
}
impl Drop for TestIdentity {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
pub(crate) async fn identity(host: &str) -> TestIdentity {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let generated = rcgen::generate_simple_self_signed(vec![host.into()]).unwrap();
    let cert = generated.cert.pem();
    let key = generated.signing_key.serialize_pem();
    let identity = felucca_transport::Identity::from_pem(cert.as_bytes(), key.as_bytes()).unwrap();
    let root = std::env::temp_dir().join(format!(
        "felucca-identity-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let config = TlsConfig {
        cert: root.join("cert.pem"),
        key: root.join("key.pem"),
    };
    std::fs::write(&config.cert, cert).unwrap();
    std::fs::write(&config.key, key).unwrap();
    TestIdentity {
        root,
        identity,
        config,
    }
}
pub(crate) async fn socks_login(port: u16, user: &str, password: &str) -> TcpStream {
    let mut io = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    io.write_all(&[5, 2, 0, 2]).await.unwrap();
    let mut answer = [0; 2];
    io.read_exact(&mut answer).await.unwrap();
    assert_eq!(
        answer,
        [5, 2],
        "configured authentication must not downgrade"
    );
    let mut auth = vec![1, user.len() as u8];
    auth.extend(user.as_bytes());
    auth.push(password.len() as u8);
    auth.extend(password.as_bytes());
    io.write_all(&auth).await.unwrap();
    io.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, [1, 0]);
    io
}
