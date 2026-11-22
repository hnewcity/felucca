//! Control-plane messages.
//!
//! Unlike the Go original, which tags each connection with a 4-byte ASCII flag
//! (`main`, `chan`, `vstr`, `conf`, ...) and then parses `*#*`-separated
//! payloads, felucca authenticates the transport connection once and then
//! carries everything as typed messages over multiplexed streams.

use serde::{Deserialize, Serialize};

/// Wire protocol version this build speaks.
pub const PROTOCOL_VERSION: u16 = 10;

/// Oldest protocol version this build still understands.
pub const MIN_PROTOCOL_VERSION: u16 = 10;

/// Optional live-configuration capability query, carried in a legacy Ping.
/// Legacy servers echo it; supporting servers return ConfigChanged. Only then
/// may the agent send WatchConfig, which legacy decoders do not understand.
pub const LIVE_CONFIG_QUERY: u64 = 0x6665_6c75_6363_6131;

// An inverted range would make `negotiate_version` reject every peer, including
// one running this exact build.
const _: () = assert!(MIN_PROTOCOL_VERSION <= PROTOCOL_VERSION);

/// Length of a handshake nonce, in bytes.
pub const NONCE_LEN: usize = 32;

/// Length of the handshake authentication tag (HMAC-SHA256), in bytes.
pub const MAC_LEN: usize = 32;

/// What an agent wants this transport connection for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionRole {
    /// The agent's tunnel: carries proxied streams and control traffic.
    Tunnel,
    /// A one-shot connection that pushes config and disconnects.
    ///
    /// The Go original's `WORK_CONFIG` mode, kept because it lets an agent
    /// register its tunnel definitions without holding a tunnel open.
    ConfigPush,
}

/// First message an agent sends, before authenticating.
///
/// Version negotiation is a range on both sides rather than a single number, so
/// an agent and a server from different releases can find common ground instead
/// of failing on inequality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Newest protocol version the agent speaks.
    pub protocol: u16,
    /// Oldest protocol version the agent still accepts.
    pub min_protocol: u16,
    /// Human-readable build identifier, for logs and the dashboard.
    pub agent_version: String,
    pub role: ConnectionRole,
}

/// The server's answer to [`Hello`], carrying the authentication challenge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    /// Protocol version the server chose for this connection.
    pub protocol: u16,
    /// Random per-connection challenge the agent must authenticate.
    pub nonce: [u8; NONCE_LEN],
    /// Whether the server enforces the handshake timestamp window.
    pub strict_clock: bool,
}

/// The agent's proof that it holds the verify key.
///
/// The key itself is never transmitted: only its identifier and an HMAC over
/// the server's nonce. On a plaintext transport a passive observer therefore
/// learns nothing reusable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auth {
    /// Which verify key this is, so the server knows which secret to check.
    pub key_id: String,
    /// Agent's clock at handshake time, in unix seconds.
    pub timestamp: i64,
    /// `HMAC-SHA256(vkey, transcript)` — see [`crate::handshake`].
    pub mac: [u8; MAC_LEN],
}

/// Why a server refused a connection.
///
/// Deliberately coarse: a peer that has not authenticated learns only that it
/// failed, never whether the key id exists or the MAC was the wrong part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthReject {
    /// Bad key id, bad MAC, or a timestamp outside the accepted window.
    BadCredentials,
    /// No protocol version in common.
    VersionMismatch,
    /// Credentials were fine, but the account is disabled or out of quota.
    NotPermitted,
    /// Server is shutting down or at capacity.
    Unavailable,
}

/// The server's verdict on [`Auth`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthResult {
    Ok {
        /// The id this agent is known by, for correlating logs and metrics.
        client_id: u32,
        /// How often the agent should send [`Control::Ping`], in seconds.
        keepalive_secs: u32,
        /// The server's own proof that it holds the verify key.
        ///
        /// Without this the handshake authenticates only one direction, and
        /// anything in the network path could accept an agent's connection,
        /// answer `Ok` without knowing the key, and read every byte of the
        /// tunnel. The agent must check this before sending anything.
        server_mac: [u8; MAC_LEN],
    },
    Err(AuthReject),
}

/// What a newly opened stream is for.
///
/// The server sends this as the first message on a stream it opens; the agent
/// answers with [`StreamResponse`] before either side sends payload bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamRequest {
    /// Serve one connection for a registered tunnel.
    ///
    /// The agent chooses which of that tunnel's backends to use, because only it
    /// knows which are currently healthy. A server that named the target instead
    /// would have to track backend health remotely and would still be a probe
    /// interval behind the agent's own view.
    Tunnel {
        tunnel_id: u32,
    },
    /// A registered route, with the original peer for ACLs and source hashing.
    /// `target` is present only for a forward proxy request.
    Routed {
        tunnel_id: u32,
        source: std::net::IpAddr,
        target: Option<String>,
        /// A SOCKS5 UDP association target. Fixed UDP routes need no flag.
        datagram: bool,
    },
    /// Connect to a specific TCP address on the agent's side.
    ///
    /// For the modes where the *client* named the target — SOCKS5, HTTP proxy —
    /// so there is nothing for the agent to choose.
    Tcp {
        target: String,
    },
    /// Carry datagrams for a UDP session to `target`.
    Udp {
        target: String,
    },
    /// Carry an HTTP request to a reverse-proxy backend.
    ///
    /// Separate from [`StreamRequest::Tcp`] so the agent can apply per-host
    /// settings such as a TLS-terminating backend.
    Http {
        target: String,
        tls: bool,
    },
    /// The long-lived stream carrying [`Control`] messages both ways.
    Control,
    Visit(crate::visitor::VisitRequest),
    Peer(Box<crate::peer::ProviderOffer>),
    /// Optional extension: older peers close only this stream. Existing variant
    /// indices and control messages are unchanged.
    Diagnostics {
        tunnel_id: u32,
    },
}

impl StreamRequest {
    /// Bind payload authentication to this exact routing request.
    pub fn payload_context(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }
}

/// Why an agent could not honour a [`StreamRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamReject {
    /// The local target refused the connection.
    ConnectionRefused,
    /// The local target did not answer in time.
    Timeout,
    /// The target address could not be resolved.
    UnknownHost,
    /// The agent's configuration does not permit this target.
    Forbidden,
    /// The agent is at its connection limit.
    TooManyConnections,
    /// Anything else; details stay in the agent's log.
    Internal,
}

/// The agent's answer to a [`StreamRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamResponse {
    /// Connected; payload bytes follow on this stream.
    Ok,
    Err(StreamReject),
}

/// Messages exchanged on the control stream after authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Control {
    /// Liveness probe. `nonce` is echoed in the matching [`Control::Pong`] so a
    /// late reply cannot be mistaken for a fresh one when measuring RTT.
    /// The reserved [`LIVE_CONFIG_QUERY`] nonce instead probes the optional
    /// configuration extension; a supporting server replies ConfigChanged.
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },

    /// An agent registering the tunnels it wants served.
    PushConfig {
        tunnels: Vec<TunnelSpec>,
        remote_config: bool,
        revision: Option<[u8; 32]>,
    },
    /// Canonical JSON is redacted in Debug and includes agent-side backend options.
    Configure {
        revision: [u8; 32],
        config: felucca_core::Secret,
    },
    RestoreConfig,
    /// The server's verdict on a [`Control::PushConfig`].
    ConfigResult {
        accepted: Vec<u32>,
        rejected: Vec<ConfigRejection>,
    },

    /// Periodic byte counters from the agent, for reconciliation.
    TrafficReport {
        tunnel_id: u32,
        inlet: u64,
        export: u64,
    },

    /// The server is going away; stop reconnecting until told otherwise.
    Shutdown {
        reason: String,
    },
    /// Appended extension: this agent polls desired state without replacing
    /// its bridge session. Send only after a positive LIVE_CONFIG_QUERY reply.
    WatchConfig,
    /// Reply to a watching agent's Ping: negotiate the latest desired state.
    ConfigChanged,
}

/// One rejected entry from a config push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigRejection {
    pub tunnel_id: u32,
    pub reason: String,
}

/// What a tunnel does with traffic arriving on its public port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelMode {
    /// Forward raw TCP to a fixed target.
    Tcp,
    /// Forward UDP datagrams to a fixed target.
    Udp,
    /// Serve SOCKS5, choosing the target per request.
    Socks5,
    /// Serve an HTTP forward proxy.
    HttpProxy,
    /// Serve both SOCKS5 and HTTP proxy on one port, detected per connection.
    MixedProxy,
    Http,
    Transparent,
}

/// A tunnel an agent asks the server to expose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelSpec {
    pub public: bool,
    /// Backend is a confined file service; no directory or credentials are sent.
    pub file: bool,
    pub tunnel_id: u32,
    pub mode: TunnelMode,
    /// Public port to listen on.
    pub port: u16,
    /// Address to bind; `None` means every interface.
    pub bind: Option<String>,
    /// Where the agent sends the traffic, for the fixed-target modes.
    pub target: Option<String>,
    pub remark: String,
    pub hosts: Vec<String>,
    pub acl_mode: felucca_core::AclMode,
    pub acl_rules: String,
    pub options: felucca_core::config::RouteOptions,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{read_frame, write_frame};

    async fn round_trip<T>(msg: &T) -> T
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        let mut buf = Vec::new();
        write_frame(&mut buf, msg).await.unwrap();
        read_frame(&mut buf.as_slice()).await.unwrap()
    }

    #[tokio::test]
    async fn hello_round_trips() {
        let msg = Hello {
            protocol: PROTOCOL_VERSION,
            min_protocol: MIN_PROTOCOL_VERSION,
            agent_version: "felucca/0.1.0".into(),
            role: ConnectionRole::Tunnel,
        };
        assert_eq!(round_trip(&msg).await, msg);
    }

    #[tokio::test]
    async fn server_hello_round_trips_with_its_nonce_intact() {
        let msg = ServerHello {
            protocol: PROTOCOL_VERSION,
            nonce: [0xab; NONCE_LEN],
            strict_clock: true,
        };
        assert_eq!(round_trip(&msg).await, msg);
    }

    #[tokio::test]
    async fn auth_and_its_verdict_round_trip() {
        let msg = Auth {
            key_id: "abc123".into(),
            timestamp: 1_700_000_000,
            mac: [7; MAC_LEN],
        };
        assert_eq!(round_trip(&msg).await, msg);

        let ok = AuthResult::Ok {
            client_id: 42,
            keepalive_secs: 30,
            server_mac: [3; MAC_LEN],
        };
        assert_eq!(round_trip(&ok).await, ok);

        let err = AuthResult::Err(AuthReject::BadCredentials);
        assert_eq!(round_trip(&err).await, err);
    }

    #[tokio::test]
    async fn stream_negotiation_round_trips() {
        let req = StreamRequest::Tunnel { tunnel_id: 7 };
        assert_eq!(round_trip(&req).await, req);

        let req = StreamRequest::Tcp {
            target: "127.0.0.1:8080".into(),
        };
        assert_eq!(round_trip(&req).await, req);

        let req = StreamRequest::Http {
            target: "10.0.0.2:443".into(),
            tls: true,
        };
        assert_eq!(round_trip(&req).await, req);

        assert_eq!(round_trip(&StreamResponse::Ok).await, StreamResponse::Ok);

        let rej = StreamResponse::Err(StreamReject::ConnectionRefused);
        assert_eq!(round_trip(&rej).await, rej);
    }

    #[tokio::test]
    async fn control_messages_round_trip() {
        let ping = Control::Ping { nonce: u64::MAX };
        assert_eq!(round_trip(&ping).await, ping);

        let push = Control::PushConfig {
            remote_config: false,
            revision: None,
            tunnels: vec![TunnelSpec {
                public: true,
                file: false,
                tunnel_id: 1,
                mode: TunnelMode::Tcp,
                port: 8080,
                bind: None,
                target: Some("127.0.0.1:22".into()),
                remark: "ssh".into(),
                hosts: vec![],
                acl_mode: felucca_core::AclMode::Off,
                acl_rules: String::new(),
                options: Default::default(),
            }],
        };
        assert_eq!(round_trip(&push).await, push);

        let report = Control::TrafficReport {
            tunnel_id: 1,
            inlet: 1 << 40,
            export: 99,
        };
        assert_eq!(round_trip(&report).await, report);
    }

    #[tokio::test]
    async fn a_config_push_with_many_tunnels_fits_in_a_frame() {
        // The frame cap has to accommodate a realistic bulk registration.
        let tunnels = (0..2000)
            .map(|i| TunnelSpec {
                public: true,
                file: false,
                tunnel_id: i,
                mode: TunnelMode::Tcp,
                port: 10_000 + (i as u16 % 1000),
                bind: Some("0.0.0.0".into()),
                target: Some(format!("192.168.1.{}:8080", i % 256)),
                remark: format!("tunnel number {i}"),
                hosts: vec![],
                acl_mode: felucca_core::AclMode::Off,
                acl_rules: String::new(),
                options: Default::default(),
            })
            .collect();
        let push = Control::PushConfig {
            tunnels,
            remote_config: false,
            revision: None,
        };
        assert_eq!(round_trip(&push).await, push);
    }
}
