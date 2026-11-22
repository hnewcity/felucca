//! Admin-only host artifacts. Paths come from active configuration, never a URL.
use super::{Principal, Result, bad, resources};
use felucca_core::ServerConfig;
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(crate) fn logs(principal: &Principal, config: &ServerConfig) -> Result<Value> {
    resources::admin(principal)?;
    let Some(path) = &config.log.file else {
        return Ok(json!({"configured":false}));
    };
    let mut file = std::fs::File::open(path).map_err(|_| bad("cannot read configured log file"))?;
    let length = file
        .metadata()
        .map_err(|_| bad("cannot read configured log file"))?
        .len();
    file.seek(SeekFrom::Start(length.saturating_sub(65536)))
        .map_err(|_| bad("cannot read configured log file"))?;
    let mut bytes = Vec::new();
    file.take(65536)
        .read_to_end(&mut bytes)
        .map_err(|_| bad("cannot read configured log file"))?;
    let original = resources::value(config);
    let redacted = resources::redacted(config);
    let mut secrets = Vec::new();
    secret_values(&original, &redacted, &mut secrets);
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    // Do not expose the suffix/prefix of a credential at a tail boundary.
    if length > 65536 {
        text = text
            .split_once('\n')
            .map_or(String::new(), |(_, rest)| rest.to_owned());
    }
    if !text.ends_with('\n') {
        text.truncate(text.rfind('\n').map_or(0, |index| index + 1));
    }
    for secret in secrets {
        if !secret.is_empty() {
            text = text.replace(&secret, resources::REDACTED);
        }
    }
    Ok(json!({"configured":true,"text":text,"truncated":length>65536}))
}

fn secret_values(original: &Value, redacted: &Value, result: &mut Vec<String>) {
    if redacted.as_str() == Some(resources::REDACTED) {
        if let Some(value) = original.as_str() {
            result.push(value.to_owned());
        }
    } else if let Some(map) = original.as_object() {
        for (key, value) in map {
            secret_values(value, &redacted[key], result);
        }
    } else if let Some(list) = original.as_array() {
        for (index, value) in list.iter().enumerate() {
            secret_values(value, &redacted[index], result);
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CertificateUpload {
    pub cert: felucca_core::Secret,
    pub key: felucca_core::Secret,
}

pub(crate) fn upload_certificate(
    principal: &Principal,
    root: &Path,
    upload: CertificateUpload,
) -> Result<Value> {
    resources::admin(principal)?;
    let identity =
        felucca_transport::Identity::from_pem(upload.cert.as_bytes(), upload.key.as_bytes())
            .map_err(|_| bad("invalid certificate or private key"))?;
    crate::reverse::Certificates::key(&identity)
        .map_err(|_| bad("invalid certificate or private key"))?;
    let mut digest_input = upload.cert.as_bytes().to_vec();
    digest_input.extend_from_slice(upload.key.as_bytes());
    let name = super::credentials::hex(&super::credentials::digest(&digest_input));
    let directory = root.with_extension("certificates").join(name);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&directory)
        .map_err(|_| bad("cannot store certificate files"))?;
    // Content-addressed immutable files make a lost-response retry safe. A prior
    // partial write is rejected, never silently accepted as an uploaded identity.
    for (name, contents) in [
        ("certificate.pem", upload.cert.as_bytes()),
        ("key.pem", upload.key.as_bytes()),
    ] {
        let path = directory.join(name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                if file
                    .write_all(contents)
                    .and_then(|()| file.sync_all())
                    .is_err()
                {
                    let _ = std::fs::remove_file(path);
                    return Err(bad("cannot store certificate files"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::read(path).ok().as_deref() != Some(contents) {
                    return Err(bad("cannot store certificate files"));
                }
            }
            Err(_) => return Err(bad("cannot store certificate files")),
        }
    }
    Ok(json!({"cert":directory.join("certificate.pem"),"key":directory.join("key.pem")}))
}
