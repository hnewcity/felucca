//! Error types shared across the felucca crates.

use felucca_i18n::{Message, Value};
use std::path::PathBuf;

/// Failure while loading or validating a configuration file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot parse config {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// The file parsed but describes something we refuse to run.
    ///
    /// `Display` keeps the `invalid config:` prefix an operator reading a log or
    /// a CLI error expects; [`ConfigError::message`] hands the console the
    /// message on its own, since the page already says it is a configuration
    /// error.
    #[error("invalid config: {0}")]
    Invalid(Message),
}

impl ConfigError {
    pub fn invalid(msg: impl Into<Message>) -> Self {
        Self::Invalid(msg.into())
    }

    /// The message a client should read, keyed for translation.
    ///
    /// `Display` keeps the English source text, so logs and existing
    /// assertions are unchanged; browsers use [`ConfigError::message`].
    pub fn message(&self) -> Message {
        match self {
            Self::Read { path, .. } => Message::with(
                "cannot read config {path}",
                [Value::new("path", path.display())],
            ),
            Self::Parse { path, .. } => Message::with(
                "cannot parse config {path}",
                [Value::new("path", path.display())],
            ),
            Self::Invalid(message) => message.clone(),
        }
    }
}

/// Failure while parsing an access-control rule or port range.
#[derive(Debug, thiserror::Error)]
#[error("invalid {kind} rule {value:?}: {reason}")]
pub struct RuleParseError {
    pub kind: &'static str,
    pub value: String,
    pub reason: String,
}

impl RuleParseError {
    /// The message a client should read, keyed for translation.
    pub fn message(&self) -> Message {
        Message::with(
            "invalid {kind} rule {value}: {reason}",
            [
                Value::new("kind", self.kind),
                Value::debug("value", &self.value),
                Value::new("reason", &self.reason),
            ],
        )
    }
}
