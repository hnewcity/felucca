//! End-to-end feature suites with explicit shared fixtures.
mod support;
use felucca_client::Agent;
use felucca_core::config::{HttpProxyConfig, TlsConfig};
use felucca_core::{ServerConfig, TransportKind, TrustPolicy};
use felucca_server::Server;
use std::{sync::Arc, time::Duration};
use support::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
#[path = "features/admin.rs"]
mod admin_features;
#[path = "features/certificates.rs"]
mod certificates_features;
#[path = "features/config_launch.rs"]
mod config_launch_features;
#[path = "features/daemon.rs"]
mod daemon_features;
#[path = "features/files.rs"]
mod files_features;
#[path = "features/forwarding.rs"]
mod forwarding_features;
#[path = "features/http.rs"]
mod http_features;
#[path = "features/kcp.rs"]
mod kcp_features;
#[path = "features/local.rs"]
mod local_features;
#[path = "features/managed.rs"]
mod managed_features;
#[path = "features/network.rs"]
mod network_features;
#[path = "features/node.rs"]
mod node_features;
#[path = "features/ntp.rs"]
mod ntp_features;
#[path = "features/payload.rs"]
mod payload_features;
#[path = "features/peer.rs"]
mod peer_features;
#[path = "features/policy.rs"]
mod policy_features;
#[path = "features/realtime.rs"]
mod realtime_features;
#[path = "features/shared_quic.rs"]
mod shared_quic_features;
#[path = "features/socks.rs"]
mod socks_features;
#[path = "features/transparent.rs"]
mod transparent_features;
#[path = "features/visitor.rs"]
mod visitor_features;
#[path = "features/websocket.rs"]
mod websocket_features;
