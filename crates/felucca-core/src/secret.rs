//! A string that does not leak into logs.

use std::fmt;

/// A shared secret — a verify key, a proxy password — that redacts itself.
///
/// Config structs get logged: at startup, in a debug dump, inside an error
/// context. A plain `String` verify key would land in a log file the first time
/// anyone wrote `{:?}` on the struct holding it, and log files are copied into
/// bug reports. Redacting in `Debug` makes that leak impossible to write by
/// accident; reading the value takes an explicit [`expose`](Self::expose) that a
/// reviewer can grep for.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret itself. Every call site is a place to check nothing is logged.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The secret's bytes, for keying an HMAC.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The length is safe to show and helps diagnose an empty or truncated
        // key without revealing the value.
        write!(f, "Secret(<redacted, {} bytes>)", self.0.len())
    }
}

impl fmt::Display for Secret {
    /// Also redacted: `{}` in a log line is at least as common as `{:?}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Secret {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl<'de> serde::Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self)
    }
}

impl serde::Serialize for Secret {
    /// Serializes the real value.
    ///
    /// Config round-trips have to preserve it — writing back `<redacted>` would
    /// quietly destroy a user's key. Redaction is a logging concern, and logging
    /// goes through `Debug` and `Display`, both of which redact.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_hides_the_value() {
        let s = Secret::new("hunter2-the-real-key");
        let shown = format!("{s:?}");
        assert!(!shown.contains("hunter2"), "leaked: {shown}");
        assert!(shown.contains("redacted"), "{shown}");
        assert!(shown.contains("20"), "the length is useful: {shown}");
    }

    #[test]
    fn display_output_hides_the_value() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s}"), "<redacted>");
    }

    #[test]
    fn a_struct_holding_a_secret_does_not_leak_it() {
        // The realistic leak: nobody prints the secret directly, they print the
        // config struct that contains it.
        #[derive(Debug)]
        #[allow(dead_code, reason = "only the Debug output is under test")]
        struct Config {
            name: String,
            vkey: Secret,
        }
        let c = Config {
            name: "agent-1".into(),
            vkey: Secret::new("top-secret-vkey"),
        };
        let shown = format!("{c:?}");
        assert!(shown.contains("agent-1"), "non-secret fields still show");
        assert!(!shown.contains("top-secret-vkey"), "leaked: {shown}");
    }

    #[test]
    fn the_value_is_available_when_asked_for_explicitly() {
        let s = Secret::new("abc");
        assert_eq!(s.expose(), "abc");
        assert_eq!(s.as_bytes(), b"abc");
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        assert!(Secret::new("").is_empty());
    }

    #[test]
    fn serialization_preserves_the_real_value() {
        // Redacting here would silently destroy a user's key on a config rewrite.
        let s = Secret::new("keep-me");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"keep-me\"");

        let back: Secret = serde_json::from_str(&json).unwrap();
        assert_eq!(back.expose(), "keep-me");
    }

    #[test]
    fn secrets_compare_by_value() {
        assert_eq!(Secret::new("a"), Secret::new("a"));
        assert_ne!(Secret::new("a"), Secret::new("b"));
    }
}
