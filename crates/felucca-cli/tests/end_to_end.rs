//! The whole system, end to end.
//!
//! Starts a real server and a real agent over loopback and pushes bytes through
//! a forwarded port. This is the test that says whether felucca works at all:
//! every unit test can pass while the pieces fail to fit together.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use felucca_client::Agent;
use felucca_core::config::{
    AgentConfig, Backend, BridgeConfig, ClientEntry, LoadBalance, TunnelConfig, TunnelMode,
};
use felucca_core::{AclMode, Quota, Secret, ServerConfig, TransportKind, TrustPolicy};
use felucca_server::Server;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Turns on logging once, so a failing run explains itself.
fn init_logging() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "felucca=debug".into()),
            )
            .with_test_writer()
            .try_init();
    });
}

/// An echo service standing in for whatever sits behind the agent.
async fn echo_service() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if stream.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    port
}

/// A free port, found by binding and releasing.
async fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn client_entry(vkey: &str) -> ClientEntry {
    ClientEntry {
        enabled: true,
        user: None,
        vkey: Secret::new(vkey),
        remark: String::new(),
        quota: Quota::UNLIMITED,
        rate_limit: None,
        max_connections: None,
        max_tunnels: None,
        acl_mode: AclMode::Off,
        acl_rules: String::new(),
        // Tunnels are registered by the agent pushing config.
        allow_config_push: true,
    }
}

/// Brings up a server and an agent with one TCP tunnel to `backend_port`.
///
/// Returns the public port the tunnel listens on.
async fn harness(backend_port: u16, tunnel_port: u16, vkey: &str) -> u16 {
    init_logging();
    let mut clients = HashMap::new();
    clients.insert("agent-1".to_owned(), client_entry(vkey));

    let server_config = ServerConfig {
        bridge: BridgeConfig {
            bind: "127.0.0.1:0".to_owned(),
            ..Default::default()
        },
        clients,
        ..Default::default()
    };

    let server = Arc::new(Server::new(server_config).expect("server should build"));
    let fingerprint = server.fingerprint();
    let bridge_port = Arc::clone(&server).run().await.expect("bridge should bind");

    let agent_config = AgentConfig {
        p2p: Default::default(),
        visitors: Vec::new(),
        remote: Default::default(),
        kcp: Default::default(),
        ntp: None,
        network: Default::default(),
        geo: Default::default(),
        mappings: Vec::new(),
        mux: Default::default(),
        server: format!("127.0.0.1:{bridge_port}"),
        vkey: Secret::new(vkey),
        key_id: Some("agent-1".to_owned()),
        transport: TransportKind::Tls,
        // The bridge generated its own certificate, so pin it rather than
        // switching verification off.
        trust: TrustPolicy::Pinned { fingerprint },
        ws_path: "/ws".to_owned(),
        reconnect_min: Duration::from_millis(50),
        reconnect_max: Duration::from_secs(1),
        dial_timeout: Duration::from_secs(5),
        max_connections: None,
        tunnels: vec![TunnelConfig {
            public: true,
            enabled: true,
            file: None,
            id: None,
            mode: TunnelMode::Tcp,
            port: tunnel_port,
            bind: Some("127.0.0.1".to_owned()),
            backends: vec![Backend::new(format!("127.0.0.1:{backend_port}"))],
            balance: LoadBalance::RoundRobin,
            health: None,
            hosts: Vec::new(),
            acl_mode: AclMode::Off,
            acl_rules: String::new(),
            options: Default::default(),
            target_policy: Default::default(),
            remark: "e2e".to_owned(),
        }],
    };

    tokio::spawn(Arc::new(Agent::new(agent_config).unwrap()).run());

    // Wait for the agent to connect and its listener to come up.
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", tunnel_port)).await.is_ok() {
            return tunnel_port;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the tunnel never came up on port {tunnel_port}");
}

#[tokio::test]
async fn bytes_travel_through_a_forwarded_port() {
    let backend = echo_service().await;
    let port = harness(backend, free_port().await, "e2e-secret").await;

    let mut client = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("should connect");
    client.write_all(b"hello through the tunnel").await.unwrap();

    let mut got = vec![0u8; 24];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut got))
        .await
        .expect("the echo should come back")
        .unwrap();
    assert_eq!(&got, b"hello through the tunnel");
}

#[tokio::test]
async fn a_bulk_transfer_survives_the_round_trip() {
    const LEN: usize = 2 * 1024 * 1024;
    let backend = echo_service().await;
    let port = harness(backend, free_port().await, "bulk-secret").await;

    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let payload: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();

    let (mut read_half, mut write_half) = client.split();
    let expected = payload.clone();
    let reader = async move {
        let mut got = vec![0u8; LEN];
        read_half.read_exact(&mut got).await.unwrap();
        assert_eq!(got, expected, "payload corrupted in transit");
    };
    let writer = async move {
        write_half.write_all(&payload).await.unwrap();
    };

    tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(writer, reader)
    })
    .await
    .expect("a bulk transfer should not stall");
}

#[tokio::test]
async fn several_connections_are_served_at_once() {
    let backend = echo_service().await;
    let port = harness(backend, free_port().await, "concurrent-secret").await;

    let mut tasks = Vec::new();
    for i in 0..24u32 {
        tasks.push(tokio::spawn(async move {
            let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let tag = i.to_be_bytes();
            c.write_all(&tag).await.unwrap();
            let mut back = [0u8; 4];
            c.read_exact(&mut back).await.unwrap();
            assert_eq!(back, tag, "a reply landed on the wrong connection");
        }));
    }
    for t in tasks {
        tokio::time::timeout(Duration::from_secs(30), t)
            .await
            .expect("connection timed out")
            .unwrap();
    }
}

#[tokio::test]
async fn a_half_close_reaches_the_backend() {
    // The `nc -N` shape: the client finishes sending and still expects a reply.
    let backend = echo_service().await;
    let port = harness(backend, free_port().await, "halfclose-secret").await;

    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(b"final request").await.unwrap();
    client.shutdown().await.unwrap();

    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut got))
        .await
        .expect("the response direction must survive a half-close")
        .unwrap();
    assert_eq!(&got, b"final request");
}

#[tokio::test]
async fn traffic_is_accounted_against_the_client() {
    let backend = echo_service().await;
    let tunnel_port = free_port().await;

    let mut clients = HashMap::new();
    clients.insert("agent-1".to_owned(), client_entry("metered-secret"));
    let server = Arc::new(
        Server::new(ServerConfig {
            bridge: BridgeConfig {
                bind: "127.0.0.1:0".to_owned(),
                ..Default::default()
            },
            clients,
            ..Default::default()
        })
        .unwrap(),
    );
    let fingerprint = server.fingerprint();
    let bridge_port = Arc::clone(&server).run().await.unwrap();

    let agent_config = AgentConfig {
        p2p: Default::default(),
        visitors: Vec::new(),
        remote: Default::default(),
        kcp: Default::default(),
        ntp: None,
        network: Default::default(),
        geo: Default::default(),
        mappings: Vec::new(),
        mux: Default::default(),
        server: format!("127.0.0.1:{bridge_port}"),
        vkey: Secret::new("metered-secret"),
        key_id: Some("agent-1".to_owned()),
        transport: TransportKind::Tls,
        trust: TrustPolicy::Pinned { fingerprint },
        ws_path: "/ws".to_owned(),
        reconnect_min: Duration::from_millis(50),
        reconnect_max: Duration::from_secs(1),
        dial_timeout: Duration::from_secs(5),
        max_connections: None,
        tunnels: vec![TunnelConfig {
            public: true,
            enabled: true,
            file: None,
            id: None,
            mode: TunnelMode::Tcp,
            port: tunnel_port,
            bind: Some("127.0.0.1".to_owned()),
            backends: vec![Backend::new(format!("127.0.0.1:{backend}"))],
            balance: LoadBalance::RoundRobin,
            health: None,
            hosts: Vec::new(),
            acl_mode: AclMode::Off,
            acl_rules: String::new(),
            options: Default::default(),
            target_policy: Default::default(),
            remark: String::new(),
        }],
    };
    tokio::spawn(Arc::new(Agent::new(agent_config).unwrap()).run());

    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", tunnel_port)).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut client = TcpStream::connect(("127.0.0.1", tunnel_port))
        .await
        .unwrap();
    client.write_all(&[7u8; 1000]).await.unwrap();
    let mut got = vec![0u8; 1000];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut got))
        .await
        .expect("echo should return")
        .unwrap();
    drop(client);

    // Counters update as data moves, so they should be visible shortly after.
    let mut stats = None;
    for _ in 0..40 {
        if let Some(c) = server.registry().get("agent-1") {
            let s = c.traffic.stats();
            if s.inlet > 0 && s.export > 0 {
                stats = Some(s);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let stats = stats.expect("traffic should have been counted");
    assert!(stats.inlet >= 1000, "inlet undercounted: {stats:?}");
    assert!(stats.export >= 1000, "export undercounted: {stats:?}");
}

#[tokio::test]
async fn a_wrong_vkey_never_brings_the_tunnel_up() {
    let backend = echo_service().await;
    let tunnel_port = free_port().await;

    let mut clients = HashMap::new();
    clients.insert("agent-1".to_owned(), client_entry("the-real-secret"));
    let server = Arc::new(
        Server::new(ServerConfig {
            bridge: BridgeConfig {
                bind: "127.0.0.1:0".to_owned(),
                ..Default::default()
            },
            clients,
            ..Default::default()
        })
        .unwrap(),
    );
    let fingerprint = server.fingerprint();
    let bridge_port = Arc::clone(&server).run().await.unwrap();

    let agent_config = AgentConfig {
        p2p: Default::default(),
        visitors: Vec::new(),
        remote: Default::default(),
        kcp: Default::default(),
        ntp: None,
        network: Default::default(),
        geo: Default::default(),
        mappings: Vec::new(),
        mux: Default::default(),
        server: format!("127.0.0.1:{bridge_port}"),
        // Not the key the server knows.
        vkey: Secret::new("the-wrong-secret"),
        key_id: Some("agent-1".to_owned()),
        transport: TransportKind::Tls,
        trust: TrustPolicy::Pinned { fingerprint },
        ws_path: "/ws".to_owned(),
        reconnect_min: Duration::from_millis(50),
        reconnect_max: Duration::from_millis(200),
        dial_timeout: Duration::from_secs(5),
        max_connections: None,
        tunnels: vec![TunnelConfig {
            public: true,
            enabled: true,
            file: None,
            id: None,
            mode: TunnelMode::Tcp,
            port: tunnel_port,
            bind: Some("127.0.0.1".to_owned()),
            backends: vec![Backend::new(format!("127.0.0.1:{backend}"))],
            balance: LoadBalance::RoundRobin,
            health: None,
            hosts: Vec::new(),
            acl_mode: AclMode::Off,
            acl_rules: String::new(),
            options: Default::default(),
            target_policy: Default::default(),
            remark: String::new(),
        }],
    };
    tokio::spawn(Arc::new(Agent::new(agent_config).unwrap()).run());

    // Give it long enough to have retried several times.
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert!(
        server.registry().is_empty(),
        "an agent with the wrong key must not be registered"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", tunnel_port))
            .await
            .is_err(),
        "no listener should exist for an unauthenticated agent"
    );
}
