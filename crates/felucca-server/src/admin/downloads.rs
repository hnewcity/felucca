//! Authenticated distribution of administrator-provided, matching agent builds.
use super::{Result, missing};
use crate::http_support::{Body, BoxError};
use bytes::Bytes;
use http::{Response, StatusCode, header};
use http_body_util::{BodyExt, StreamBody};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Seek},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use tokio::io::AsyncReadExt;

const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "aarch64-pc-windows-msvc",
    "x86_64-pc-windows-msvc",
];
const MAX_BINARY: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Package {
    pub target: String,
    version: String,
    protocol: u16,
    filename: String,
    bytes: u64,
    pub(super) sha256: String,
    /// Publisher metadata, not a replacement for the device's Gatekeeper check.
    #[serde(default)]
    notarized: bool,
}
fn roots() -> Vec<PathBuf> {
    std::env::var_os("FELUCCA_AGENT_ASSETS")
        .map(PathBuf::from)
        .into_iter()
        .chain(
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("agents"))),
        )
        .collect()
}
fn metadata(root: &Path, target: &str) -> Option<Package> {
    if !TARGETS.contains(&target) {
        return None;
    }
    let path = root.join(target).join("manifest.json");
    let mut file = std::fs::File::open(path).ok()?.take(4097);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    if bytes.len() > 4096 {
        return None;
    }
    let package: Package = serde_json::from_slice(&bytes).ok()?;
    let filename = if target.contains("windows") {
        "felucca-agent.exe"
    } else {
        "felucca-agent"
    };
    (package.target == target
        && package.version == env!("CARGO_PKG_VERSION")
        && package.protocol == felucca_proto::PROTOCOL_VERSION
        && package.filename == filename
        && (1..=MAX_BINARY).contains(&package.bytes)
        && package.sha256.len() == 64
        && package.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        && root
            .join(target)
            .join(filename)
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() == package.bytes))
    .then_some(package)
}
pub(super) fn catalog() -> Vec<Package> {
    let roots = roots();
    TARGETS
        .iter()
        .filter_map(|target| roots.iter().find_map(|root| metadata(root, target)))
        .collect()
}
fn verified(root: &Path, target: &str) -> Option<(Package, std::fs::File)> {
    let package = metadata(root, target)?;
    let root = root.canonicalize().ok()?;
    let path = root
        .join(target)
        .join(&package.filename)
        .canonicalize()
        .ok()?;
    if !path.starts_with(&root) {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0; 65536];
    let mut total = 0;
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > package.bytes {
            return None;
        }
        digest.update(&buffer[..count]);
    }
    let hash: String = digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if total != package.bytes || hash != package.sha256 {
        return None;
    }
    file.rewind().ok()?;
    Some((package, file))
}
pub(super) async fn download(target: &str) -> Result<Response<Body>> {
    static DOWNLOADS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let permit = DOWNLOADS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            super::Error(
                StatusCode::TOO_MANY_REQUESTS,
                "management request limit reached".into(),
            )
        })?;
    let roots = roots();
    let target = target.to_owned();
    let (package, file) =
        tokio::task::spawn_blocking(move || roots.iter().find_map(|root| verified(root, &target)))
            .await
            .map_err(|_| missing())?
            .ok_or_else(missing)?;
    let stream = futures_util::stream::try_unfold(
        (tokio::fs::File::from_std(file).take(package.bytes), permit),
        |(mut file, permit)| async move {
            let mut buffer = vec![0; 65536];
            let count = file.read(&mut buffer).await?;
            if count == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            buffer.truncate(count);
            Ok(Some((
                hyper::body::Frame::data(Bytes::from(buffer)),
                (file, permit),
            )))
        },
    );
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, package.bytes)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", package.filename),
        )
        .header(header::ETAG, format!("\"{}\"", package.sha256))
        .body(
            StreamBody::new(stream)
                .map_err(|e| -> BoxError { Box::new(e) })
                .boxed_unsync(),
        )
        .expect("validated package headers"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packages_are_bounded_matched_and_verified_before_serving() {
        let root = std::env::temp_dir().join(format!("agent-assets-{}", uuid::Uuid::new_v4()));
        let target = TARGETS[0];
        std::fs::create_dir_all(root.join(target)).unwrap();
        let content = b"fixture agent";
        let sha256: String = ring::digest::digest(&ring::digest::SHA256, content)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut package = Package {
            target: target.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: felucca_proto::PROTOCOL_VERSION,
            filename: "felucca-agent".into(),
            bytes: content.len() as u64,
            sha256,
            notarized: false,
        };
        let manifest = root.join(target).join("manifest.json");
        std::fs::write(root.join(target).join("felucca-agent"), content).unwrap();
        std::fs::write(&manifest, serde_json::to_vec(&package).unwrap()).unwrap();
        assert!(verified(&root, target).is_some());
        assert!(verified(&root, "../").is_none());
        std::fs::write(root.join(target).join("felucca-agent"), b"tampered!!!!!").unwrap();
        assert!(verified(&root, target).is_none());
        package.version = "different".into();
        std::fs::write(&manifest, serde_json::to_vec(&package).unwrap()).unwrap();
        assert!(metadata(&root, target).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
