//! Shared visitor fixtures; test cases live in the feature suites.
use super::*;
use felucca_core::config::{ShareConfig, VisitorConfig};
use tokio::net::UdpSocket;
pub(crate) const SECRET: &str = "visitor-share-secret-32-characters";
pub(crate) fn share() -> ShareConfig {
    toml::from_str(&format!(
        "client='features'\ntunnel=1\nsecret='{SECRET}'\nallow_clients=['guest']"
    ))
    .unwrap()
}
pub(crate) async fn visitor(mode: &str) -> VisitorConfig {
    let port = if mode == "udp" {
        UdpSocket::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    } else {
        free_port().await
    };
    toml::from_str(&format!(
        "name='local'\nshare='office'\nsecret='{SECRET}'\nmode='{mode}'\nbind='127.0.0.1:{port}'\nudp_idle='1s'"
    ))
    .unwrap()
}
pub(crate) fn provider(mode: &str, backend: u16) -> AgentConfig {
    let backend = if backend == 0 {
        String::new()
    } else {
        format!("backend=[{{addr='127.0.0.1:{backend}'}}]\n")
    };
    agent(&format!(
        "[[tunnel]]\nid=1\nmode='{mode}'\npublic=false\n{backend}"
    ))
}
pub(crate) struct Worker {
    pub(crate) agent: Arc<Agent>,
    pub(crate) task: tokio::task::JoinHandle<()>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.agent.shutdown();
        self.task.abort();
    }
}
pub(crate) async fn worker(
    mut config: AgentConfig,
    server: &Server,
    address: &str,
    transport: TransportKind,
) -> Worker {
    config.server = address.into();
    config.transport = transport;
    config.trust = TrustPolicy::Pinned {
        fingerprint: server.fingerprint(),
    };
    let agent = Arc::new(Agent::new(config).unwrap());
    let task = tokio::spawn(agent.clone().run());
    tokio::time::timeout(Duration::from_secs(10), async {
        while !agent.is_ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("visitor fixture agent did not register");
    Worker { agent, task }
}
pub(crate) struct Rig {
    pub(crate) server: Arc<Server>,
    pub(crate) _provider: Worker,
    pub(crate) _visitor: Worker,
    pub(crate) address: String,
    pub(crate) bind: String,
}
impl Drop for Rig {
    fn drop(&mut self) {
        self.server.shutdown();
    }
}
pub(crate) async fn rig(
    mut config: ServerConfig,
    provider: AgentConfig,
    visitor: VisitorConfig,
    transport: TransportKind,
) -> Rig {
    let mut guest = config.clients["features"].clone();
    guest.vkey = "guest-vkey".into();
    guest.allow_config_push = false;
    config.clients.entry("guest".into()).or_insert(guest);
    config.shares.entry("office".into()).or_insert_with(share);
    let udp = if transport == TransportKind::Quic {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        config.bridge.quic_bind = Some(format!("127.0.0.1:{port}"));
        Some(port)
    } else {
        None
    };
    let server = Arc::new(Server::new(config).unwrap());
    let port = server.clone().run().await.unwrap();
    let address = format!("127.0.0.1:{}", udp.unwrap_or(port));
    let provider = worker(provider, &server, &address, transport).await;
    let mut guest = agent("");
    guest.key_id = Some("guest".into());
    guest.vkey = "guest-vkey".into();
    let bind = visitor.bind.clone();
    guest.visitors = vec![visitor];
    let guest = worker(guest, &server, &address, transport).await;
    Rig {
        server,
        _provider: provider,
        _visitor: guest,
        address,
        bind,
    }
}
pub(crate) async fn rejected(bind: &str) {
    let mut stream = TcpStream::connect(bind).await.unwrap();
    let _ = stream.write_all(b"must not reach backend").await;
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut response))
        .await
        .expect("rejected visitor hung");
    assert!(response.is_empty(), "unauthorized bytes returned");
}
pub(crate) async fn released(server: &Server) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if ["features", "guest"].iter().all(|key| {
                server
                    .registry()
                    .get(key)
                    .is_none_or(|c| c.live_connections() == 0)
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("visitor leaked an admission permit");
}
pub(crate) async fn raw_guest(
    server: &Server,
    address: &str,
    tls: bool,
) -> felucca_transport::Tunnel {
    use felucca_proto::ConnectionRole;
    let io = TcpStream::connect(address).await.unwrap();
    let mut io: felucca_transport::BoxedConn = if tls {
        Box::new(
            felucca_transport::tls::handshake(
                io,
                "localhost",
                &felucca_transport::tls::Verification::Pinned {
                    fingerprint: server.fingerprint(),
                },
                &[felucca_transport::BRIDGE_ALPN],
            )
            .await
            .unwrap(),
        )
    } else {
        Box::new(io)
    };
    felucca_proto::session::connect(
        &mut io,
        "guest",
        &"guest-vkey".into(),
        ConnectionRole::Tunnel,
    )
    .await
    .unwrap();
    felucca_transport::Tunnel::over_stream(io, felucca_mux::Role::Client, Default::default())
}
