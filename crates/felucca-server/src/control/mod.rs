//! Management operations independent of HTTP routing and browser sessions.
use felucca_i18n::Message;

pub(crate) mod configuration;
pub(crate) mod connection;
pub mod credentials;
pub(crate) mod diagnostics;
pub(crate) mod host_metrics;
mod identity;
pub(crate) mod operations;
pub(crate) mod resources;
mod service;
pub(crate) mod store;
#[cfg(test)]
mod tests;

pub use identity::Principal;
pub(crate) use service::{Change, Command, Management, Operation};

/// A management failure.
///
/// The message text carries [`felucca_i18n::Message`]: an English source with
/// its values named, so the admin listener can answer in the caller's language
/// while logs keep the English text.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(Message),
    #[error("permission denied")]
    Forbidden,
    #[error("resource not found")]
    Missing,
    #[error("{0}")]
    Conflict(Message),
    #[error("{0}")]
    Unavailable(Message),
    #[error("password worker limit reached")]
    Busy,
}
impl Error {
    /// The message a client should read, keyed for translation.
    pub fn message(&self) -> Message {
        match self {
            Self::Invalid(message) | Self::Conflict(message) | Self::Unavailable(message) => {
                message.clone()
            }
            Self::Forbidden => Message::text("permission denied"),
            Self::Missing => Message::text("resource not found"),
            Self::Busy => Message::text("password worker limit reached"),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;
fn bad(message: impl Into<Message>) -> Error {
    Error::Invalid(message.into())
}
fn forbidden() -> Error {
    Error::Forbidden
}
fn missing() -> Error {
    Error::Missing
}

/// Resource identities contain domain identifiers, never HTTP paths.
#[derive(Debug, Clone)]
pub enum Resource {
    Platforms,
    Platform(String),
    Webhooks,
    Webhook(String),
    Shares,
    Share(String),
    Users,
    User(String),
    Clients,
    Client(String),
    Managed(String),
    ManagedTunnel(String, u32),
    Locals,
    Local(String),
    LocalTunnel(String, u32),
    Accounts,
    Account(String),
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Write {
    Put,
    Delete,
}
