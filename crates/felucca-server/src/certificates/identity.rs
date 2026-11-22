//! Stable default bridge identity, owned by the locked configuration store.
use felucca_core::ConfigError;
use felucca_i18n::Message;
use felucca_transport::Identity;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedIdentity {
    cert: String,
    key: String,
}

/// The caller holds the configuration lock. Both halves are committed together;
/// unreadable or invalid existing identities are never silently replaced.
pub(crate) fn load_or_create(configuration: &Path) -> io::Result<Identity> {
    let mut name = configuration.as_os_str().to_owned();
    name.push(".identity.json");
    let path = std::path::PathBuf::from(name);
    let saved = match File::open(&path) {
        Ok(file) => {
            if !file.metadata()?.is_file() {
                return Err(io::Error::other(ConfigError::invalid(Message::text(
                    "The saved bridge identity must be a regular file.",
                ))));
            }
            let mut bytes = Vec::new();
            file.take(65537).read_to_end(&mut bytes)?;
            if bytes.len() > 65536 {
                return Err(io::Error::other(ConfigError::invalid(Message::text(
                    "The saved bridge identity exceeds 64 KiB.",
                ))));
            }
            serde_json::from_slice::<SavedIdentity>(&bytes).map_err(io::Error::other)?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let generated = rcgen::generate_simple_self_signed(vec!["felucca".into()])
                .map_err(io::Error::other)?;
            let saved = SavedIdentity {
                cert: generated.cert.pem(),
                key: generated.signing_key.serialize_pem(),
            };
            let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            let result = (|| {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temporary)?;
                file.write_all(&serde_json::to_vec(&saved).map_err(io::Error::other)?)?;
                file.sync_all()?;
                drop(file);
                std::fs::rename(&temporary, &path)?;
                #[cfg(unix)]
                File::open(
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new(".")),
                )?
                .sync_all()?;
                Ok::<_, io::Error>(())
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            result?;
            saved
        }
        Err(error) => return Err(error),
    };
    let identity = Identity::from_pem(saved.cert.as_bytes(), saved.key.as_bytes())
        .map_err(io::Error::other)?;
    identity.server_config().map_err(io::Error::other)?;
    Ok(identity)
}
