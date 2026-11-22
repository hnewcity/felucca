//! Shared backend service engine. Bridge connection management remains in the agent.
pub mod dial;
mod file_fs;
mod file_server;
mod health;
pub mod pool;
pub mod remote;
mod service;
pub use pool::{Lease, Member, Pool};
pub use service::{Services, VirtualDial};

mod specs;
pub use specs::specs;

pub mod socks;
