//! Local file service settings, kept on the agent rather than sent to a peer.
use crate::{ConfigError, access::Authentication};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub root: PathBuf,
    #[serde(default = "root_prefix")]
    pub prefix: String,
    #[serde(default = "yes")]
    pub read_only: bool,
    #[serde(default = "yes")]
    pub auto_index: bool,
    #[serde(default)]
    pub auth: Authentication,
}
fn root_prefix() -> String {
    "/".into()
}
fn yes() -> bool {
    true
}
impl FileConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.root.as_os_str().is_empty() || !self.root.is_dir() {
            return Err(ConfigError::invalid(
                "file.root must name an existing directory",
            ));
        }
        self.validate_options()
    }
    pub fn validate_options(&self) -> Result<(), ConfigError> {
        if !self.prefix.starts_with('/')
            || self.prefix.contains(['?', '#', '\\', '%'])
            || self.prefix.bytes().any(|byte| byte <= 32 || byte == 127)
            || self
                .prefix
                .split('/')
                .any(|part| part == "." || part == "..")
        {
            return Err(ConfigError::invalid(
                "file.prefix must be a plain absolute URL path",
            ));
        }
        self.auth.validate()
    }
}

#[cfg(test)]
mod tests {
    use crate::AgentConfig;
    #[test]
    fn file_mode_defaults_to_read_only_and_can_share_the_http_listener() {
        let mut config: AgentConfig = toml::from_str(
            r#"
server = "127.0.0.1:14028"
vkey = "key"
[[tunnel]]
mode = "file"
port = 8080
hosts = ["files.test"]
[tunnel.file]
root = "."
[[tunnel]]
mode = "http"
port = 8080
hosts = ["app.test"]
backend = [{addr = "127.0.0.1:3000"}]
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.tunnels[0].exposes_http());
        assert!(config.tunnels[0].file.as_ref().unwrap().read_only);
        config.tunnels[0]
            .options
            .auth
            .users
            .insert("a".into(), "secret".into());
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("file.auth")
        );
    }
    #[test]
    fn file_settings_reject_ignored_options_and_unsafe_prefixes() {
        let text = r#"
server = "localhost:14028"
vkey = "secret"
[[tunnel]]
mode = "file"
port = 9000
[tunnel.file]
root = "."
prefix = "/files"
"#;
        let mut config: AgentConfig = toml::from_str(text).unwrap();
        config.validate().unwrap();
        for prefix in ["relative", "/../secret", "/a\\b", "/a?b", "/%2e%2e"] {
            let mut config = config.clone();
            config.tunnels[0].file.as_mut().unwrap().prefix = prefix.into();
            assert!(config.validate().is_err(), "{prefix}");
        }
        let mut mismatch: AgentConfig =
            toml::from_str(&text.replace("mode = \"file\"", "mode = \"tcp\"")).unwrap();
        assert!(
            mismatch
                .validate()
                .unwrap_err()
                .to_string()
                .contains("requires file mode")
        );
        config.tunnels[0].file = None;
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("requires [tunnel.file]")
        );
    }
}
