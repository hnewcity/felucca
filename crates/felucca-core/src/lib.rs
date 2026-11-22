//! Shared primitives for the felucca server and agent.
//!
//! This crate holds the pieces both sides agree on — identifiers, access
//! control, traffic accounting and rate limiting — and deliberately depends on
//! no async runtime, so it stays cheap to unit test.

pub mod access;
pub mod acl;
pub mod config;
pub mod error;
pub mod geodata;
pub mod id;
pub mod rate;
pub mod secret;
pub mod traffic;

pub use acl::{Acl, AclMode, AclRule};
pub use config::{AgentConfig, ServerConfig, TransportKind, TrustPolicy};
pub use error::{ConfigError, RuleParseError};
pub use id::{ClientId, HostId, StreamId, TunnelId, UserId};
pub use rate::RateLimiter;
pub use secret::Secret;
pub use traffic::{Quota, QuotaState, TrafficCounter, TrafficStats};
