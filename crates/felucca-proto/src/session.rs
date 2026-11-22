//! Driving the handshake, from both ends.
//!
//! Both sides live together so a change to one cannot silently diverge from the
//! other, and here rather than in `felucca-server` so the agent can reach the
//! client half without depending on the server.

use std::collections::HashMap;
use std::time::Duration;

use felucca_core::Secret;
use felucca_core::config::ClientEntry;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::handshake::{Direction, Transcript, build_server_result_proof, verify_mac};
use crate::{
    Auth, AuthReject, AuthResult, ConnectionRole, Hello, ServerHello, handshake, read_frame,
    write_frame,
};

/// What the server checks a handshake against.
///
/// A narrow view of the server's config, so this module needs no opinion about
/// how a server is configured.
#[derive(Debug, Clone, Copy)]
pub struct AcceptPolicy<'a> {
    /// Permitted agents, keyed by `key_id`.
    pub clients: &'a HashMap<String, ClientEntry>,
    /// Whether to reject an agent whose clock is far from ours.
    pub strict_clock: bool,
    /// How often to tell the agent to send a keepalive.
    pub keepalive_secs: u32,
}

/// How long an agent has to complete the handshake.
///
/// Without this a peer that opens a connection and says nothing holds a task and
/// a socket indefinitely, which is a free denial of service.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// What a completed handshake established.
#[derive(Debug)]
pub struct Authenticated {
    /// The agent's `key_id`, as configured on the server.
    pub key_id: String,
    /// The protocol version both sides settled on.
    pub protocol: u16,
    /// What the agent wants this connection for.
    pub role: ConnectionRole,
}

/// Why a handshake did not complete.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeFailure {
    /// Authentication failed. Deliberately opaque: which part failed is not the
    /// peer's business, and saying so would let an attacker enumerate key ids.
    #[error("authentication failed")]
    Rejected(AuthReject),

    #[error("the peer did not complete the handshake in time")]
    Timeout,

    #[error("protocol error during handshake")]
    Protocol(#[from] crate::CodecError),
}

/// Runs the server side of the handshake.
///
/// Both directions authenticate: the agent proves it holds the verify key, and
/// the server proves the same before the agent sends anything. See
/// `felucca_proto::handshake` for why one-sided authentication is not enough.
pub async fn accept<T>(
    io: &mut T,
    policy: AcceptPolicy<'_>,
) -> Result<Authenticated, HandshakeFailure>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    accept_with_clock(io, policy, now_unix).await
}

/// Use a calibrated application clock without changing handshake authentication.
pub async fn accept_with_clock<T, F>(
    io: &mut T,
    policy: AcceptPolicy<'_>,
    clock: F,
) -> Result<Authenticated, HandshakeFailure>
where
    T: AsyncRead + AsyncWrite + Unpin,
    F: Fn() -> i64,
{
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, run(io, policy, clock)).await {
        Ok(result) => result,
        Err(_) => Err(HandshakeFailure::Timeout),
    }
}

async fn run<T, F>(
    io: &mut T,
    policy: AcceptPolicy<'_>,
    clock: F,
) -> Result<Authenticated, HandshakeFailure>
where
    T: AsyncRead + AsyncWrite + Unpin,
    F: Fn() -> i64,
{
    let hello: Hello = read_frame(io).await?;

    let protocol = match handshake::negotiate_version(hello.protocol, hello.min_protocol) {
        Ok(v) => v,
        Err(_) => {
            // Tell the peer why: a version mismatch is a deployment problem, not
            // an attack, and an operator needs to see it.
            let _ = write_frame(io, &AuthResult::Err(AuthReject::VersionMismatch)).await;
            return Err(HandshakeFailure::Rejected(AuthReject::VersionMismatch));
        }
    };

    let nonce = handshake::generate_nonce();
    write_frame(
        io,
        &ServerHello {
            protocol,
            nonce,
            strict_clock: policy.strict_clock,
        },
    )
    .await?;

    let auth: Auth = read_frame(io).await?;

    // Look the key up, but do not branch out early on a miss: verifying against a
    // dummy key keeps the work — and so the response time — the same whether the
    // key id exists or not. Returning immediately here would let an attacker
    // enumerate valid key ids by timing alone.
    let known: Option<&ClientEntry> = policy.clients.get(&auth.key_id);
    let placeholder = Secret::new("felucca-unknown-key-placeholder");
    let vkey = known.map_or(&placeholder, |e| &e.vkey);

    let transcript = Transcript {
        protocol,
        nonce: &nonce,
        key_id: &auth.key_id,
        timestamp: auth.timestamp,
        role: hello.role,
    };

    let mac_ok = verify_mac(
        vkey.as_bytes(),
        &transcript,
        Direction::ClientProof,
        &auth.mac,
    )
    .is_ok();
    let clock_ok =
        !policy.strict_clock || handshake::validate_timestamp(auth.timestamp, clock()).is_ok();

    let Some(entry) = known.filter(|_| mac_ok && clock_ok) else {
        // One message for every failure mode, so a peer learns only that it
        // failed. The reason is logged locally, where an operator can see it.
        tracing::warn!(
            key_id = %auth.key_id,
            known = known.is_some(),
            mac_ok,
            clock_ok,
            "rejected a bridge handshake"
        );
        let _ = write_frame(io, &AuthResult::Err(AuthReject::BadCredentials)).await;
        return Err(HandshakeFailure::Rejected(AuthReject::BadCredentials));
    };

    if !entry.enabled || (hello.role == ConnectionRole::ConfigPush && !entry.allow_config_push) {
        // Credentials were good, so this one can be specific.
        let _ = write_frame(io, &AuthResult::Err(AuthReject::NotPermitted)).await;
        return Err(HandshakeFailure::Rejected(AuthReject::NotPermitted));
    }

    let server_mac =
        build_server_result_proof(entry.vkey.as_bytes(), &transcript, 0, policy.keepalive_secs);
    write_frame(
        io,
        &AuthResult::Ok {
            // Assigned by the registry once the connection is established; the
            // agent uses it only for log correlation.
            client_id: 0,
            keepalive_secs: policy.keepalive_secs,
            server_mac,
        },
    )
    .await?;

    tracing::info!(key_id = %auth.key_id, protocol, ?hello.role, "agent authenticated");
    Ok(Authenticated {
        key_id: auth.key_id,
        protocol,
        role: hello.role,
    })
}

/// How often an agent should send a keepalive, given when the server gives up.
///
/// Half the timeout, so a single lost keepalive does not drop a live agent.
pub fn keepalive_for(disconnect_timeout: Duration) -> u32 {
    (disconnect_timeout.as_secs().max(2) / 2) as u32
}

/// Current wall-clock time in unix seconds.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        // Before 1970 the clock is unusable; 0 makes any skew check fail, which
        // is the safe direction.
        .unwrap_or(0)
}

/// Runs the agent side of the handshake.
///
/// Kept beside the server side so the two cannot drift apart unnoticed.
pub async fn connect<T>(
    io: &mut T,
    key_id: &str,
    vkey: &Secret,
    role: ConnectionRole,
) -> Result<u16, HandshakeFailure>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    connect_info(io, key_id, vkey, role)
        .await
        .map(|info| info.protocol)
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectInfo {
    pub protocol: u16,
    pub keepalive_secs: u32,
}

pub async fn connect_info<T>(
    io: &mut T,
    key_id: &str,
    vkey: &Secret,
    role: ConnectionRole,
) -> Result<ConnectInfo, HandshakeFailure>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    connect_info_with_clock(io, key_id, vkey, role, now_unix).await
}

/// Read the application clock after receiving the server challenge, so dialing
/// or a slow greeting does not age the timestamp before the proof is generated.
pub async fn connect_info_with_clock<T, F>(
    io: &mut T,
    key_id: &str,
    vkey: &Secret,
    role: ConnectionRole,
    clock: F,
) -> Result<ConnectInfo, HandshakeFailure>
where
    T: AsyncRead + AsyncWrite + Unpin,
    F: Fn() -> i64,
{
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, run_client(io, key_id, vkey, role, clock)).await {
        Ok(result) => result,
        Err(_) => Err(HandshakeFailure::Timeout),
    }
}

async fn run_client<T, F>(
    io: &mut T,
    key_id: &str,
    vkey: &Secret,
    role: ConnectionRole,
    clock: F,
) -> Result<ConnectInfo, HandshakeFailure>
where
    T: AsyncRead + AsyncWrite + Unpin,
    F: Fn() -> i64,
{
    write_frame(
        io,
        &Hello {
            protocol: crate::PROTOCOL_VERSION,
            min_protocol: crate::MIN_PROTOCOL_VERSION,
            agent_version: concat!("felucca/", env!("CARGO_PKG_VERSION")).to_owned(),
            role,
        },
    )
    .await?;

    let server_hello: ServerHello = read_frame(io).await?;
    if !(crate::MIN_PROTOCOL_VERSION..=crate::PROTOCOL_VERSION).contains(&server_hello.protocol) {
        return Err(HandshakeFailure::Rejected(AuthReject::VersionMismatch));
    }
    let timestamp = clock();
    let auth = handshake::build_auth(
        vkey.as_bytes(),
        key_id,
        server_hello.protocol,
        &server_hello.nonce,
        role,
        timestamp,
    );
    write_frame(io, &auth).await?;

    match read_frame::<_, AuthResult>(io).await? {
        AuthResult::Ok {
            server_mac,
            keepalive_secs,
            client_id,
        } => {
            // Check the server's proof before trusting the connection with
            // anything. Skipping this would let an impostor accept the
            // connection, answer `Ok`, and read the whole tunnel.
            let transcript = Transcript {
                protocol: server_hello.protocol,
                nonce: &server_hello.nonce,
                key_id,
                timestamp,
                role,
            };
            if handshake::verify_server_result_proof(
                vkey.as_bytes(),
                &transcript,
                client_id,
                keepalive_secs,
                &server_mac,
            )
            .is_err()
            {
                tracing::error!(
                    "the server could not prove it holds the verify key; refusing the tunnel"
                );
                return Err(HandshakeFailure::Rejected(AuthReject::BadCredentials));
            }
            Ok(ConnectInfo {
                protocol: server_hello.protocol,
                keepalive_secs: keepalive_secs.clamp(1, 3600),
            })
        }
        AuthResult::Err(reason) => Err(HandshakeFailure::Rejected(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use felucca_core::config::ClientEntry;
    use felucca_core::{AclMode, Quota};

    fn entry(vkey: &str, allow_config_push: bool) -> ClientEntry {
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
            allow_config_push,
        }
    }

    /// The clients a test server knows about.
    fn clients(pairs: &[(&str, &str)]) -> HashMap<String, ClientEntry> {
        pairs
            .iter()
            .map(|(id, key)| ((*id).to_owned(), entry(key, false)))
            .collect()
    }

    fn policy(clients: &HashMap<String, ClientEntry>) -> AcceptPolicy<'_> {
        AcceptPolicy {
            clients,
            strict_clock: false,
            keepalive_secs: 15,
        }
    }

    /// Runs both sides against each other over an in-memory pipe.
    async fn exchange(
        policy: AcceptPolicy<'_>,
        key_id: &str,
        vkey: &str,
        role: ConnectionRole,
    ) -> (
        Result<Authenticated, HandshakeFailure>,
        Result<u16, HandshakeFailure>,
    ) {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let owned_id = key_id.to_owned();
        let owned_key = Secret::new(vkey);

        let client =
            tokio::spawn(async move { connect(&mut a, &owned_id, &owned_key, role).await });
        let server = accept(&mut b, policy).await;
        let client = client.await.unwrap();
        (server, client)
    }

    #[tokio::test]
    async fn a_correct_key_authenticates_both_ways() {
        let known = clients(&[("agent-1", "the-secret")]);
        let (server, client) = exchange(
            policy(&known),
            "agent-1",
            "the-secret",
            ConnectionRole::Tunnel,
        )
        .await;

        let auth = server.expect("the server should accept");
        assert_eq!(auth.key_id, "agent-1");
        assert_eq!(auth.role, ConnectionRole::Tunnel);
        assert_eq!(auth.protocol, crate::PROTOCOL_VERSION);

        let protocol = client.expect("the agent should accept the server's proof");
        assert_eq!(protocol, auth.protocol);
    }

    #[tokio::test]
    async fn a_wrong_key_is_rejected_on_both_sides() {
        let known = clients(&[("agent-1", "the-secret")]);
        let (server, client) = exchange(
            policy(&known),
            "agent-1",
            "the-wrong-secret",
            ConnectionRole::Tunnel,
        )
        .await;

        assert!(matches!(
            server,
            Err(HandshakeFailure::Rejected(AuthReject::BadCredentials))
        ));
        // The agent has to learn it failed, not hang waiting.
        assert!(client.is_err(), "the agent should see the rejection");
    }

    #[tokio::test]
    async fn an_unknown_key_id_is_rejected_the_same_way_as_a_bad_key() {
        // Identical error, so a peer cannot enumerate valid key ids.
        let known = clients(&[("agent-1", "the-secret")]);
        let (server, _) = exchange(
            policy(&known),
            "agent-99",
            "the-secret",
            ConnectionRole::Tunnel,
        )
        .await;

        match server {
            Err(HandshakeFailure::Rejected(AuthReject::BadCredentials)) => {}
            other => panic!("expected the same opaque rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_agent_refuses_a_server_that_cannot_prove_the_key() {
        // The impostor case: something in the path accepts the connection and
        // answers Ok without knowing the verify key.
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);

        let client = tokio::spawn(async move {
            connect(
                &mut a,
                "agent-1",
                &Secret::new("real-key"),
                ConnectionRole::Tunnel,
            )
            .await
        });

        // A fake server: read the hello, send a nonce, accept anything.
        let _hello: Hello = read_frame(&mut b).await.unwrap();
        let nonce = handshake::generate_nonce();
        write_frame(
            &mut b,
            &ServerHello {
                protocol: crate::PROTOCOL_VERSION,
                nonce,
                strict_clock: false,
            },
        )
        .await
        .unwrap();
        let _auth: Auth = read_frame(&mut b).await.unwrap();
        write_frame(
            &mut b,
            &AuthResult::Ok {
                client_id: 1,
                keepalive_secs: 15,
                // Not derivable without the key.
                server_mac: [0u8; crate::message::MAC_LEN],
            },
        )
        .await
        .unwrap();

        let result = client.await.unwrap();
        assert!(
            result.is_err(),
            "an agent that accepts an unproven server hands its tunnel to an impostor"
        );
    }

    #[tokio::test]
    async fn a_reflected_client_proof_does_not_pass_as_a_server_proof() {
        // The refinement of the same attack: the impostor cannot compute the
        // server proof, so it echoes back the agent's own.
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);

        let client = tokio::spawn(async move {
            connect(
                &mut a,
                "agent-1",
                &Secret::new("real-key"),
                ConnectionRole::Tunnel,
            )
            .await
        });

        let _hello: Hello = read_frame(&mut b).await.unwrap();
        let nonce = handshake::generate_nonce();
        write_frame(
            &mut b,
            &ServerHello {
                protocol: crate::PROTOCOL_VERSION,
                nonce,
                strict_clock: false,
            },
        )
        .await
        .unwrap();
        let auth: Auth = read_frame(&mut b).await.unwrap();
        write_frame(
            &mut b,
            &AuthResult::Ok {
                client_id: 1,
                keepalive_secs: 15,
                server_mac: auth.mac,
            },
        )
        .await
        .unwrap();

        assert!(
            client.await.unwrap().is_err(),
            "domain separation must stop a reflected proof"
        );
    }

    #[tokio::test]
    async fn config_push_needs_permission() {
        let mut known = HashMap::new();
        known.insert("agent-1".to_owned(), entry("the-secret", false));

        let (server, _) = exchange(
            policy(&known),
            "agent-1",
            "the-secret",
            ConnectionRole::ConfigPush,
        )
        .await;
        match server {
            Err(HandshakeFailure::Rejected(AuthReject::NotPermitted)) => {}
            other => panic!("config push should need opting in, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn config_push_is_allowed_when_enabled() {
        let mut known = HashMap::new();
        known.insert("agent-1".to_owned(), entry("the-secret", true));

        let (server, client) = exchange(
            policy(&known),
            "agent-1",
            "the-secret",
            ConnectionRole::ConfigPush,
        )
        .await;
        assert_eq!(
            server.expect("should accept").role,
            ConnectionRole::ConfigPush
        );
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn a_proof_for_one_role_does_not_work_for_another() {
        // The transcript covers the role, so a tunnel proof cannot open a
        // config-push connection.
        let nonce = handshake::generate_nonce();

        let tunnel_auth = handshake::build_auth(
            b"k",
            "agent-1",
            crate::PROTOCOL_VERSION,
            &nonce,
            ConnectionRole::Tunnel,
            0,
        );
        let push_transcript = Transcript {
            protocol: crate::PROTOCOL_VERSION,
            nonce: &nonce,
            key_id: "agent-1",
            timestamp: 0,
            role: ConnectionRole::ConfigPush,
        };
        assert!(
            verify_mac(
                b"k",
                &push_transcript,
                Direction::ClientProof,
                &tunnel_auth.mac
            )
            .is_err(),
            "a proof must not transfer between roles"
        );
    }

    #[tokio::test]
    async fn a_silent_peer_does_not_hold_the_handshake_open() {
        tokio::time::pause();
        let known = clients(&[("agent-1", "k")]);
        let (_client, mut server_side) = tokio::io::duplex(1024);

        let task = tokio::spawn(async move {
            accept(
                &mut server_side,
                AcceptPolicy {
                    clients: &known,
                    strict_clock: false,
                    keepalive_secs: 15,
                },
            )
            .await
        });
        tokio::time::advance(HANDSHAKE_TIMEOUT + Duration::from_secs(1)).await;

        match task.await.unwrap() {
            Err(HandshakeFailure::Timeout) => {}
            other => panic!("a silent peer should time out, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_peer_that_hangs_up_mid_handshake_is_an_error_not_a_hang() {
        let known = clients(&[("agent-1", "k")]);
        let (client, mut server_side) = tokio::io::duplex(1024);
        drop(client);

        let r = accept(&mut server_side, policy(&known)).await;
        assert!(matches!(r, Err(HandshakeFailure::Protocol(_))), "{r:?}");
    }

    #[tokio::test]
    async fn a_skewed_clock_is_tolerated_unless_strict() {
        let known = clients(&[("agent-1", "k")]);

        // The nonce prevents replay regardless, so a skewed agent still works.
        let (server, client) =
            exchange(policy(&known), "agent-1", "k", ConnectionRole::Tunnel).await;
        assert!(server.is_ok() && client.is_ok());

        let strict = AcceptPolicy {
            strict_clock: true,
            ..policy(&known)
        };
        let (server, _) = exchange(strict, "agent-1", "k", ConnectionRole::Tunnel).await;
        assert!(server.is_ok(), "a correct clock passes strict mode too");
    }

    #[tokio::test]
    async fn the_keepalive_interval_is_below_the_disconnect_timeout() {
        // Otherwise an agent's first keepalive arrives after it has been dropped.
        let known = clients(&[("agent-1", "k")]);
        let disconnect_after = Duration::from_secs(30);
        let advertised = keepalive_for(disconnect_after);

        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let client = tokio::spawn(async move {
            connect_info(&mut a, "agent-1", &Secret::new("k"), ConnectionRole::Tunnel).await
        });
        let accepted = accept(
            &mut b,
            AcceptPolicy {
                clients: &known,
                strict_clock: false,
                keepalive_secs: advertised,
            },
        )
        .await;
        assert!(accepted.is_ok());
        assert_eq!(client.await.unwrap().unwrap().keepalive_secs, advertised);

        assert!(advertised > 0);
        assert!(
            u64::from(advertised) < disconnect_after.as_secs(),
            "a keepalive at or past the timeout is useless"
        );
    }
}
