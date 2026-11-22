//! Server-owned desired state, constrained by permissions held on the agent.
use super::{PortMapping, TunnelConfig};
use crate::{ConfigError, Secret, access::TargetPolicy, geodata::GeoResolver};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};

/// Leave room for the control envelope within the 1 MiB frame cap.
pub const MAX_MANAGED_BYTES: usize = 900 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedConfig {
    #[serde(default, rename = "tunnel")]
    pub tunnels: Vec<TunnelConfig>,
    #[serde(default, rename = "mapping")]
    pub mappings: Vec<PortMapping>,
    /// Replaces the agent's `remote.target_policy` when that agent sets
    /// `remote.console_scope`. Absent keeps the agent's own boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_policy: Option<TargetPolicy>,
}

impl ManagedConfig {
    pub fn validate(&mut self, geo: &GeoResolver) -> Result<(), ConfigError> {
        super::tunnel::normalize_with_files(
            &mut self.tunnels,
            &mut self.mappings,
            geo,
            false,
            true,
        )?;
        if let Some(policy) = &self.target_policy {
            crate::access::TargetAcl::compile_with_geo(policy, geo)?;
        }
        self.encoded()?;
        Ok(())
    }

    /// Canonical JSON keeps config-only tagged enums independent of postcard.
    /// Secret redacts the serialized file/proxy credentials in control logs.
    pub fn encoded(&self) -> Result<(Secret, [u8; 32]), ConfigError> {
        fn sort(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(map) => {
                    for child in map.values_mut() {
                        sort(child);
                    }
                    map.sort_keys();
                }
                serde_json::Value::Array(values) => values.iter_mut().for_each(sort),
                _ => {}
            }
        }
        let mut value = serde_json::to_value(self)
            .map_err(|_| ConfigError::invalid("cannot serialize managed configuration"))?;
        sort(&mut value);
        let text = serde_json::to_string(&value)
            .map_err(|_| ConfigError::invalid("cannot serialize managed configuration"))?;
        if text.len() > MAX_MANAGED_BYTES {
            return Err(ConfigError::invalid(
                "managed configuration exceeds 900 KiB",
            ));
        }
        let digest = Sha256::digest(text.as_bytes()).into();
        Ok((Secret::from(text), digest))
    }

    pub fn decode(text: &str, revision: [u8; 32], geo: &GeoResolver) -> Result<Self, ConfigError> {
        if text.len() > MAX_MANAGED_BYTES {
            return Err(ConfigError::invalid(
                "managed configuration exceeds 900 KiB",
            ));
        }
        // The envelope binds the bytes. Re-encoding after normalization ensures
        // the advertised digest also describes what the agent actually serves.
        let mut config: Self = serde_json::from_str(text)
            .map_err(|_| ConfigError::invalid("invalid managed configuration JSON"))?;
        config.validate(geo)?;
        if config.encoded()?.1 != revision {
            return Err(ConfigError::invalid(
                "managed configuration digest mismatch",
            ));
        }
        Ok(config)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemotePolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub target_policy: TargetPolicy,
    /// Let the console replace `target_policy` through managed configuration.
    #[serde(default)]
    pub console_scope: bool,
    /// Named directory capabilities. Remote configs use `name/relative/path`.
    #[serde(default, rename = "file_root")]
    pub file_roots: BTreeMap<String, RemoteFileRoot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteFileRoot {
    pub path: PathBuf,
    #[serde(default = "yes")]
    pub read_only: bool,
}
fn yes() -> bool {
    true
}

impl RemotePolicy {
    pub fn validate(&self, geo: &GeoResolver) -> Result<(), ConfigError> {
        crate::access::TargetAcl::compile_with_geo(&self.target_policy, geo)?;
        if self.file_roots.len() > 128 {
            return Err(ConfigError::invalid(
                "at most 128 remote file roots are allowed",
            ));
        }
        for (name, root) in &self.file_roots {
            if !super::local::valid_name(name)
                || name == "."
                || name == ".."
                || root.path.as_os_str().is_empty()
                || !root.path.is_dir()
            {
                return Err(ConfigError::invalid(
                    "remote file roots require a valid name and existing directory",
                ));
            }
        }
        Ok(())
    }
}

pub fn file_capability(path: &std::path::Path) -> Result<(&str, PathBuf), ConfigError> {
    let bad = || {
        ConfigError::invalid(
            "remote file.root must be a named capability followed by a relative path",
        )
    };
    let text = path.to_str().ok_or_else(bad)?;
    if text.contains('\\') || text.split('/').any(|part| part == ".." || part == ".") {
        return Err(bad());
    }
    let mut parts = path.components();
    let Some(std::path::Component::Normal(name)) = parts.next() else {
        return Err(bad());
    };
    let name = name.to_str().ok_or_else(bad)?;
    if !super::local::valid_name(name) {
        return Err(bad());
    }
    let mut relative = PathBuf::new();
    for part in parts {
        let std::path::Component::Normal(part) = part else {
            return Err(bad());
        };
        relative.push(part);
    }
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }
    Ok((name, relative))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_file_names_do_not_need_to_exist_on_the_server() {
        let mut config: ManagedConfig = toml::from_str(
            "[[tunnel]]\nmode='file'\nport=8080\n[tunnel.file]\nroot='documents/shared'",
        )
        .unwrap();
        config.validate(&GeoResolver::default()).unwrap();
        let (text, digest) = config.encoded().unwrap();
        assert!(ManagedConfig::decode(text.expose(), digest, &GeoResolver::default()).is_ok());
        config.tunnels[0].file.as_mut().unwrap().read_only = false;
        assert_ne!(config.encoded().unwrap().1, digest);
        assert!(ManagedConfig::decode(text.expose(), [0; 32], &GeoResolver::default()).is_err());
        for path in [
            "/tmp",
            "../tmp",
            "documents/../tmp",
            "documents/./tmp",
            "a\\b",
        ] {
            assert!(file_capability(path.as_ref()).is_err(), "{path}");
        }
    }
    #[test]
    fn disabled_tunnels_retain_ids_and_do_not_conflict_on_ports() {
        let mut config: ManagedConfig = toml::from_str(
            "[[tunnel]]\nid=1\nenabled=false\nmode='socks5'\nport=8080\n[[tunnel]]\nid=2\nmode='socks5'\nport=8080",
        ).unwrap();
        config.validate(&GeoResolver::default()).unwrap();
        config.tunnels[0].enabled = true;
        assert!(config.validate(&GeoResolver::default()).is_err());
    }
}
