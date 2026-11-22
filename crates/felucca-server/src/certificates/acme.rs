//! HTTP-01/TLS-ALPN-01 issuance, durable caches, and live renewal.
use crate::reverse::ReverseProxy;
use anyhow::{Context, bail};
use felucca_core::config::{AcmeChallenge, AcmeConfig};
use felucca_transport::Identity;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt,
    NewAccount, NewOrder, OrderStatus, RetryPolicy,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io, path::Path, sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use x509_parser::prelude::{FromDer, X509Certificate};

#[derive(Serialize, Deserialize)]
struct CachedAccount {
    directory: String,
    credentials: AccountCredentials,
}
#[derive(Serialize, Deserialize)]
struct CachedCertificate {
    chain: String,
    key: String,
}

fn directory(config: &AcmeConfig) -> String {
    if config.staging {
        LetsEncrypt::Staging.url().into()
    } else {
        config.directory.clone()
    }
}

async fn account(config: &AcmeConfig) -> anyhow::Result<Account> {
    let path = config.cache_dir.join("account.json");
    let cached = match tokio::fs::read(&path).await {
        // An unreadable account would fail every hourly attempt until someone
        // deleted it. Move it aside and register a fresh account instead.
        Ok(data) => match serde_json::from_slice::<CachedAccount>(&data) {
            Ok(cache) => Ok(cache),
            Err(error) => {
                let kept = quarantine(&path).await?;
                tracing::warn!(%error, kept = %kept.display(), "cached ACME account is unreadable; registering a new account");
                Err(io::Error::from(io::ErrorKind::NotFound))
            }
        },
        Err(error) => Err(error),
    };
    match cached {
        Ok(cache) => {
            if cache.directory != directory(config) {
                bail!("ACME directory changed; use a separate cache_dir for each CA/environment");
            }
            Ok(Account::builder()?
                .from_credentials(cache.credentials)
                .await?)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let contact = format!("mailto:{}", config.email);
            let binding = if let Some(api_key) = &config.zerossl_api_key {
                Some(crate::zerossl::credentials(api_key).await?)
            } else {
                config.eab.clone()
            };
            let eab = binding
                .as_ref()
                .map(|binding| {
                    Ok::<_, anyhow::Error>(instant_acme::ExternalAccountKey::new(
                        binding.key_id.clone(),
                        &binding.key_bytes()?,
                    ))
                })
                .transpose()?;
            let (account, credentials) = Account::builder()?
                .create(
                    &NewAccount {
                        contact: &[&contact],
                        terms_of_service_agreed: config.accept_tos,
                        only_return_existing: false,
                    },
                    directory(config),
                    eab.as_ref(),
                )
                .await?;
            atomic_write(
                &path,
                &serde_json::to_vec(&CachedAccount {
                    directory: directory(config),
                    credentials,
                })?,
            )
            .await?;
            Ok(account)
        }
        Err(e) => Err(e.into()),
    }
}

async fn atomic_write(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let written = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).await?;
        file.write_all(data).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, path).await
    }
    .await;
    if let Err(error) = written {
        // A full disk must not also leave a partial file per attempt.
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        tokio::fs::File::open(parent).await?.sync_all().await?;
    }
    Ok(())
}

/// Moves an unreadable cache file aside, keeping it for inspection.
async fn quarantine(path: &Path) -> io::Result<std::path::PathBuf> {
    let mut kept = path.as_os_str().to_owned();
    kept.push(format!(".corrupt-{}", crate::relay::now()));
    let kept = std::path::PathBuf::from(kept);
    tokio::fs::rename(path, &kept).await?;
    Ok(kept)
}

/// Removes temporary files left by a crash between write and rename.
async fn remove_stale_temporaries(directory: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let stale = name.strip_suffix(".tmp").is_some_and(|stem| {
            stem.rsplit_once('.')
                .is_some_and(|(_, id)| uuid::Uuid::parse_str(id).is_ok())
        });
        if stale {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// Loads a cached certificate, or `None` if there is no usable one.
///
/// A damaged or mismatched cache file is moved aside so the domain is issued
/// again. Failing on it instead would retry the same error every hour until
/// the served certificate expired.
async fn cached(
    proxy: &ReverseProxy,
    path: &Path,
    domain: &str,
    before: Duration,
) -> anyhow::Result<Option<i64>> {
    let data = match tokio::fs::read(path).await {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let loaded = (|| {
        let cache: CachedCertificate = serde_json::from_slice(&data)?;
        let identity = Identity::from_pem(cache.chain.as_bytes(), cache.key.as_bytes())?;
        let when = renewal_at(&identity, domain, before)?;
        proxy.certificates.insert(domain, &identity)?;
        Ok::<_, anyhow::Error>(when)
    })();
    match loaded {
        Ok(when) => Ok(Some(when)),
        Err(error) => {
            let kept = quarantine(path).await?;
            tracing::warn!(%domain, %error, kept = %kept.display(), "cached certificate is unusable; issuing a new one");
            Ok(None)
        }
    }
}

fn renewal_at(identity: &Identity, domain: &str, before: Duration) -> anyhow::Result<i64> {
    let (_, cert) = X509Certificate::from_der(identity.chain[0].as_ref())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let valid_name = cert.subject_alternative_name()?.is_some_and(|names| names.value.general_names.iter().any(|name|
        matches!(name, x509_parser::extensions::GeneralName::DNSName(dns) if dns.eq_ignore_ascii_case(domain))));
    if !valid_name {
        bail!("cached certificate does not cover {domain}");
    }
    let start = cert.validity().not_before.timestamp();
    let end = cert.validity().not_after.timestamp();
    if end <= start {
        bail!("invalid certificate validity");
    }
    // A shorter-lived CA certificate must not be reissued every iteration.
    let margin = before.as_secs().min(((end - start) / 3) as u64) as i64;
    Ok(end - margin)
}

struct ChallengeGuard {
    proxy: Arc<ReverseProxy>,
    keys: Vec<(String, String)>,
    tls_keys: Vec<(String, Arc<rustls::ServerConfig>)>,
}
impl Drop for ChallengeGuard {
    fn drop(&mut self) {
        let mut challenges = self.proxy.challenges.write();
        for key in &self.keys {
            challenges.remove(key);
        }
        let mut challenges = self.proxy.tls_challenges.write();
        for (host, config) in &self.tls_keys {
            if challenges
                .get(host)
                .is_some_and(|current| Arc::ptr_eq(current, config))
            {
                challenges.remove(host);
            }
        }
    }
}

async fn issue(
    account: &Account,
    proxy: Arc<ReverseProxy>,
    domain: &str,
    kind: AcmeChallenge,
) -> anyhow::Result<CachedCertificate> {
    let ids = [Identifier::Dns(domain.to_owned())];
    let mut order = account.new_order(&NewOrder::new(&ids)).await?;
    let mut guard = ChallengeGuard {
        proxy,
        keys: vec![],
        tls_keys: vec![],
    };
    let mut authorizations = order.authorizations();
    while let Some(authorization) = authorizations.next().await {
        let mut authorization = authorization?;
        if authorization.status == AuthorizationStatus::Valid {
            continue;
        }
        if authorization.status != AuthorizationStatus::Pending {
            bail!("ACME authorization is {:?}", authorization.status);
        }
        let mut challenge = authorization
            .challenge(match kind {
                AcmeChallenge::Http01 => ChallengeType::Http01,
                AcmeChallenge::TlsAlpn01 => ChallengeType::TlsAlpn01,
            })
            .context("CA did not offer the configured challenge type")?;
        let key = (domain.to_owned(), challenge.token.clone());
        guard.proxy.challenges.write().insert(
            key.clone(),
            challenge.key_authorization().as_str().to_owned(),
        );
        guard.keys.push(key);
        if kind == AcmeChallenge::TlsAlpn01 {
            let mut params = rcgen::CertificateParams::new(vec![domain.to_owned()])?;
            params
                .custom_extensions
                .push(rcgen::CustomExtension::new_acme_identifier(
                    challenge.key_authorization().digest().as_ref(),
                ));
            let pair = rcgen::KeyPair::generate()?;
            let cert = params.self_signed(&pair)?;
            let identity =
                Identity::from_pem(cert.pem().as_bytes(), pair.serialize_pem().as_bytes())?;
            // webpki rejects the ACME critical extension even when merely
            // checking a server's key. Parse SPKI separately, keeping the
            // key-match check without relaxing normal certificate validation.
            let provider = rustls::crypto::ring::default_provider();
            let signing = provider
                .key_provider
                .load_private_key(identity.key.clone_key())?;
            let (_, parsed) = X509Certificate::from_der(identity.chain[0].as_ref())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            if signing.public_key().as_ref().map(|key| key.as_ref())
                != Some(parsed.public_key().raw)
            {
                bail!("ACME challenge certificate and signing key do not match");
            }
            let certified = rustls::sign::CertifiedKey::new(identity.chain, signing);
            let mut config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(rustls::sign::SingleCertAndKey::from(certified)));
            config.alpn_protocols = vec![b"acme-tls/1".to_vec()];
            let config = Arc::new(config);
            guard
                .proxy
                .tls_challenges
                .write()
                .insert(domain.to_owned(), config.clone());
            guard.tls_keys.push((domain.to_owned(), config));
        }
        challenge.set_ready().await?;
    }
    if order.poll_ready(&RetryPolicy::default()).await? != OrderStatus::Ready {
        bail!("ACME order is not ready");
    }
    let key = order.finalize().await?;
    let chain = order.poll_certificate(&RetryPolicy::default()).await?;
    Ok(CachedCertificate { chain, key })
}

pub async fn run(proxy: Arc<ReverseProxy>, config: AcmeConfig, cancel: CancellationToken) {
    let mut renewal = HashMap::<String, i64>::new();
    let mut attempts = HashMap::<String, i64>::new();
    let mut current_account = None;
    // Issued but not yet written: kept in memory and retried, because issuing
    // again every hour would reach the CA's duplicate-certificate limit.
    let mut unsaved = HashMap::<String, Vec<u8>>::new();
    remove_stale_temporaries(&config.cache_dir).await;
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = interval.tick() => {},
            () = proxy.certificate_status.retry.notified() => { attempts.clear(); }
        }
        let domains = if config.domains.is_empty() {
            proxy.domains()
        } else {
            config.domains.clone()
        };
        for domain in domains {
            let now = crate::relay::now();
            if attempts.get(&domain).is_some_and(|after| now < *after) {
                continue;
            }
            let work = async {
                let mut directory = tokio::fs::DirBuilder::new();
                directory.recursive(true);
                #[cfg(unix)]
                directory.mode(0o700);
                directory.create(&config.cache_dir).await?;
                let path = config.cache_dir.join(format!("{domain}.json"));
                if let Some(data) = unsaved.get(&domain) {
                    match atomic_write(&path, data).await {
                        Ok(()) => {
                            unsaved.remove(&domain);
                            tracing::info!(%domain, "ACME certificate saved after an earlier write failure");
                        }
                        Err(error) => {
                            tracing::warn!(%domain, %error, "still cannot save the issued ACME certificate")
                        }
                    }
                }
                if !renewal.contains_key(&domain)
                    && let Some(when) = cached(&proxy, &path, &domain, config.renew_before).await?
                {
                    renewal.insert(domain.clone(), when);
                    proxy
                        .certificate_status
                        .record(&domain, "loaded", Some(when));
                }
                if renewal.get(&domain).is_some_and(|when| now < *when) {
                    return Ok::<_, anyhow::Error>(());
                }
                if current_account.is_none() {
                    proxy.certificate_status.record(&domain, "issuing", None);
                    current_account = Some(account(&config).await?);
                }
                proxy.certificate_status.record(&domain, "issuing", None);
                let certificate = issue(
                    current_account.as_ref().unwrap(),
                    proxy.clone(),
                    &domain,
                    config.challenge,
                )
                .await?;
                let identity =
                    Identity::from_pem(certificate.chain.as_bytes(), certificate.key.as_bytes())?;
                let when = renewal_at(&identity, &domain, config.renew_before)?;
                crate::reverse::Certificates::key(&identity)?;
                // Persist the certificate and key together, before exposing them
                // to new handshakes. If the write fails, still serve the new
                // certificate rather than one that is about to expire, and keep
                // retrying the write instead of issuing again.
                let data = serde_json::to_vec(&certificate)?;
                if let Err(error) = atomic_write(&path, &data).await {
                    tracing::error!(%domain, %error, "cannot save the issued ACME certificate; serving it from memory and retrying the write");
                    unsaved.insert(domain.clone(), data);
                }
                proxy.certificates.insert(&domain, &identity)?;
                renewal.insert(domain.clone(), when);
                proxy
                    .certificate_status
                    .record(&domain, "renewed", Some(when));
                tracing::info!(%domain, "ACME certificate installed");
                Ok(())
            };
            let result = tokio::select! {
                () = cancel.cancelled() => return,
                result = tokio::time::timeout(Duration::from_secs(180), work) => result,
            };
            if !matches!(result, Ok(Ok(()))) {
                attempts.insert(domain.clone(), now + 3600);
                proxy.certificate_status.record(
                    &domain,
                    if result.is_err() {
                        "timeout"
                    } else {
                        "issuance_failed"
                    },
                    Some(now + 3600),
                );
                match result {
                    Ok(Err(error)) => {
                        tracing::warn!(%domain, %error, "ACME failed; retrying in one hour")
                    }
                    _ => tracing::warn!(%domain, "ACME timed out; retrying in one hour"),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "acme_tests.rs"]
mod issuance_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn certificate_cache_is_atomic_private_and_reloadable() {
        let root = std::env::temp_dir().join(format!("felucca-acme-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await.unwrap();
        let path = root.join("example.com.json");
        atomic_write(&path, b"first").await.unwrap();
        atomic_write(&path, b"second").await.unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&path)
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
    #[test]
    fn renewal_uses_certificate_validity_and_checks_the_name() {
        let identity = Identity::self_signed(vec!["example.com".into()]).unwrap();
        assert!(renewal_at(&identity, "example.com", Duration::from_secs(30 * 86400)).is_ok());
        assert!(renewal_at(&identity, "other.example", Duration::from_secs(30 * 86400)).is_err());
    }
}
