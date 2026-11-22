//! Explicit, bounded standalone-binary updates; the supplied digest is the trust anchor.
use super::messages::say;
use anyhow::{Context, ensure};
use clap::Args;
use felucca_i18n::{Message, Value};
use ring::digest::{Context as DigestContext, SHA256};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Standalone binary file or HTTPS URL (archives and redirects are rejected).
    #[arg(long)]
    source: String,
    /// SHA-256 obtained from a trusted release manifest.
    #[arg(long)]
    sha256: String,
    /// Require this version in addition to the digest and program name.
    #[arg(long)]
    expect_version: Option<String>,
    /// Environment variable containing a download bearer token.
    #[arg(long)]
    token_env: Option<String>,
    /// Download and verify without replacing the installed executable.
    #[arg(long)]
    check_only: bool,
}
const LIMIT: u64 = 512 * 1024 * 1024;

impl UpdateArgs {
    pub async fn run(self, role: &str) -> anyhow::Result<()> {
        ensure!(
            self.sha256.len() == 64 && self.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "sha256 requires exactly 64 hexadecimal digits"
        );
        let exe = std::env::current_exe()?.canonicalize()?;
        let parent = exe
            .parent()
            .context(say("executable has no parent directory"))?;
        let mut lock_name = exe
            .file_name()
            .context(say("executable has no filename"))?
            .to_os_string();
        lock_name.push(".update.lock");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(parent.join(lock_name))
            .context(say("cannot create update lock beside executable"))?;
        fs2::FileExt::try_lock_exclusive(&lock).context(say("another update is in progress"))?;
        // Keep the lock file: deleting it lets later writers lock a different inode.
        let staging = tempfile::Builder::new()
            .prefix(".felucca-update-")
            .suffix(std::env::consts::EXE_SUFFIX)
            .tempfile_in(parent)?;
        let mut output = tokio::fs::File::from_std(staging.reopen()?);
        let digest = tokio::time::timeout(
            Duration::from_secs(300),
            download(&self.source, self.token_env.as_deref(), &mut output),
        )
        .await
        .context(say("update download exceeded five minutes"))??;
        output.flush().await?;
        output.sync_all().await?;
        drop(output);
        ensure!(
            digest.eq_ignore_ascii_case(&self.sha256),
            "update SHA-256 mismatch; executable unchanged"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            staging
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o700))?;
        }
        let path = staging.into_temp_path(); // close write handle before exec (Linux ETXTBSY).
        let version = identify(&path, role).await?;
        if let Some(expected) = self.expect_version {
            ensure!(
                version == expected,
                "update version does not match --expect-version"
            );
        }
        if self.check_only {
            println!(
                "{}",
                say(Message::with(
                    "verified {role} {version} SHA256 {digest}; executable unchanged",
                    [
                        Value::new("role", role),
                        Value::new("version", version),
                        Value::new("digest", digest),
                    ],
                ))
            );
            return Ok(());
        }
        self_replace::self_replace(&path).context(say("cannot replace installed executable"))?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        println!(
            "{}",
            say(Message::with(
                "installed {role} {version} SHA256 {digest}; restart the service to use it",
                [
                    Value::new("role", role),
                    Value::new("version", version),
                    Value::new("digest", digest),
                ],
            ))
        );
        Ok(())
    }
}

async fn identify(path: &Path, role: &str) -> anyhow::Result<String> {
    use std::process::Stdio;
    let mut child = tokio::process::Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context(say("verified artifact cannot run on this platform"))?;
    let stdout = child
        .stdout
        .take()
        .context(say("version stdout unavailable"))?;
    let (status, bytes) = tokio::time::timeout(Duration::from_secs(10), async {
        let mut bytes = Vec::new();
        stdout.take(4097).read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= 4096, "version output exceeds limit");
        Ok::<_, anyhow::Error>((child.wait().await?, bytes))
    })
    .await
    .context(say("version probe timed out"))??;
    ensure!(
        status.success(),
        "verified artifact failed its version probe"
    );
    let text = std::str::from_utf8(&bytes).context(say("invalid version output"))?;
    let words: Vec<_> = text.split_whitespace().collect();
    ensure!(
        words.len() == 2 && words[0] == role,
        "artifact is not a {role} executable"
    );
    ensure!(
        words[1].len() <= 128
            && words[1]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b)),
        "invalid artifact version"
    );
    Ok(words[1].into())
}

async fn download(
    source: &str,
    token_env: Option<&str>,
    output: &mut tokio::fs::File,
) -> anyhow::Result<String> {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty};
    let mut digest = DigestContext::new(&SHA256);
    let mut count = 0u64;
    if source.contains("://") {
        ensure!(source.len() <= 16384, "update URL too long");
        let url = url::Url::parse(source)
            .map_err(|_| anyhow::anyhow!("{}", say("invalid update URL")))?;
        ensure!(
            url.scheme() == "https"
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "update URL requires HTTPS without userinfo or fragment"
        );
        let tls = felucca_transport::Verification::WebPki {
            server_name: String::new(),
        }
        .rustls_client_config()?;
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_only()
            .enable_http1()
            .enable_http2()
            .build();
        let client: hyper_util::client::legacy::Client<_, Empty<Bytes>> =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(connector);
        let mut request = http::Request::builder()
            .uri(url.as_str())
            .header("accept", "application/octet-stream")
            .body(Empty::<Bytes>::new())?;
        if let Some(name) = token_env {
            let token = std::env::var(name)
                .context(say("update token environment variable unavailable"))?;
            ensure!(
                !token.is_empty() && token.len() <= 8192,
                "update token is empty or too large"
            );
            let mut header = http::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| anyhow::anyhow!("{}", say("invalid update token")))?;
            header.set_sensitive(true);
            request
                .headers_mut()
                .insert(http::header::AUTHORIZATION, header);
        }
        let response = client
            .request(request)
            .await
            .map_err(|_| anyhow::anyhow!("{}", say("update download failed")))?;
        ensure!(
            response.status() == http::StatusCode::OK,
            "update server returned HTTP {} (redirects are not followed)",
            response.status().as_u16()
        );
        let declared = response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        ensure!(
            declared.is_none_or(|length| length <= LIMIT),
            "update exceeds 512 MiB"
        );
        ensure!(
            response
                .headers()
                .get(http::header::CONTENT_ENCODING)
                .is_none_or(|v| v == "identity"),
            "compressed update bodies are unsupported"
        );
        let mut body = response.into_body();
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame
                .map_err(|_| anyhow::anyhow!("{}", say("update download interrupted")))?
                .into_data()
            {
                append(output, &mut digest, &mut count, &data).await?;
            }
        }
    } else {
        ensure!(
            token_env.is_none(),
            "--token-env is only for HTTPS downloads"
        );
        let mut file = tokio::fs::File::open(source)
            .await
            .context(say("cannot open update artifact"))?;
        let metadata = file.metadata().await?;
        ensure!(
            metadata.is_file() && metadata.len() <= LIMIT,
            "update must be a regular file no larger than 512 MiB"
        );
        let mut buffer = vec![0; 65536];
        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            append(output, &mut digest, &mut count, &buffer[..n]).await?;
        }
    }
    ensure!(count != 0, "empty update artifact");
    Ok(digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
async fn append(
    output: &mut tokio::fs::File,
    digest: &mut DigestContext,
    count: &mut u64,
    bytes: &[u8],
) -> anyhow::Result<()> {
    ensure!(
        bytes.len() as u64 <= LIMIT - *count,
        "update exceeds 512 MiB"
    );
    output.write_all(bytes).await?;
    digest.update(bytes);
    *count += bytes.len() as u64;
    Ok(())
}
