//! Saved connections, one JSON file in the user's configuration directory.
//!
//! The file holds launch URIs, and a launch URI carries the device key, so it is
//! written owner-only and replaced atomically: a crash mid-write must not leave
//! a half file that loses every connection.

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One connection the user added.
#[derive(Clone, Serialize, Deserialize)]
pub struct Saved {
    pub id: String,
    pub name: String,
    /// The `felucca://` launch URI, including the device key.
    pub uri: String,
    pub enabled: bool,
}

impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("uri", &"<redacted>")
            .field("enabled", &self.enabled)
            .finish()
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Contents {
    version: u32,
    connections: Vec<Saved>,
}

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            path: directory.join("connections.json"),
        }
    }

    /// The saved connections; a missing file is an empty list.
    pub fn load(&self) -> std::io::Result<Vec<Saved>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice::<Contents>(&bytes)
                .map(|contents| contents.connections)
                .map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    pub fn save(&self, connections: &[Saved]) -> std::io::Result<()> {
        let directory = self.path.parent().expect("the file has a directory");
        std::fs::create_dir_all(directory)?;
        let bytes = serde_json::to_vec_pretty(&Contents {
            version: 1,
            connections: connections.to_vec(),
        })
        .map_err(std::io::Error::other)?;
        let temporary = self.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_never_prints_the_key() {
        let directory =
            std::env::temp_dir().join(format!("felucca-store-{}", uuid::Uuid::new_v4()));
        let store = Store::new(directory.clone());
        assert!(store.load().unwrap().is_empty());
        let saved = Saved {
            id: "a".into(),
            name: "Home".into(),
            uri: "felucca://example.com:14028?vkey=secret".into(),
            enabled: true,
        };
        store.save(std::slice::from_ref(&saved)).unwrap();
        assert_eq!(store.load().unwrap()[0].uri, saved.uri);
        assert!(!format!("{saved:?}").contains("secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.join("connections.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
