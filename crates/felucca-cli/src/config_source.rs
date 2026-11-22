//! Bounded local/remote configuration loading. Expansion happens after parsing:
//! environment values cannot inject TOML sections or JSON keys.
use super::messages::say;
use anyhow::{Context, bail};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::{borrow::Cow, time::Duration};
use tokio::io::AsyncReadExt;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_SOURCE: usize = 6 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

fn scheme_is(source: &str, expected: &str) -> bool {
    source
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(expected))
}

/// Safe to print even when a URL, launch URI or inline config carries secrets.
pub fn label(source: &str) -> Cow<'_, str> {
    if scheme_is(source, "http") || scheme_is(source, "https") {
        "remote configuration".into()
    } else if felucca_core::config::launch::is_launch_uri(source) {
        "launch URI".into()
    } else if source.starts_with("base64:") || source.trim_start().starts_with('{') {
        "inline configuration".into()
    } else if source.contains("://") {
        "configuration URI".into()
    } else {
        source.into()
    }
}

pub async fn load<T: DeserializeOwned>(
    source: &str,
    expand_env: bool,
    token_env: Option<&str>,
) -> anyhow::Result<T> {
    if source.len() > MAX_SOURCE {
        bail!("configuration source is too large");
    }
    let mut value = if felucca_core::config::launch::is_launch_uri(source) {
        felucca_core::config::launch::fields(source)?
    } else {
        let bytes = if scheme_is(source, "http") || scheme_is(source, "https") {
            fetch(source, token_env).await?
        } else if let Some(encoded) = source.strip_prefix("base64:") {
            STANDARD
                .decode(encoded)
                .or_else(|_| URL_SAFE_NO_PAD.decode(encoded))
                .map_err(|_| anyhow::anyhow!("invalid base64 configuration"))?
        } else if source.trim_start().starts_with('{') {
            source.as_bytes().to_vec()
        } else if source.contains("://") {
            bail!("unsupported configuration URI");
        } else {
            let metadata = tokio::fs::metadata(source)
                .await
                .context(say("cannot access configuration file"))?;
            if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
                bail!("configuration must be a regular file no larger than 4 MiB");
            }
            let file = tokio::fs::File::open(source)
                .await
                .context(say("cannot open configuration file"))?;
            let mut bytes = Vec::new();
            file.take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .await
                .context(say("cannot read configuration file"))?;
            bytes
        };
        parse(&bytes)?
    };
    if expand_env {
        let mut budget = MAX_BYTES;
        expand(
            &mut value,
            &|name| match std::env::var(name) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(_) => bail!("environment variable {name} is not UTF-8"),
            },
            &mut budget,
        )?;
    }
    serde_json::from_value(value).context(say("configuration fields are invalid"))
}

fn parse(bytes: &[u8]) -> anyhow::Result<Value> {
    if bytes.len() > MAX_BYTES {
        bail!("configuration exceeds 4 MiB");
    }
    let text = std::str::from_utf8(bytes)
        .context(say("configuration is not UTF-8"))?
        .trim_start_matches('\u{feff}');
    if text.trim_start().starts_with('{') {
        // Reject duplicate JSON fields at every depth, including account maps.
        serde_json::from_str::<UniqueValue>(text)
            .map(|value| value.0)
            .map_err(|error| {
                anyhow::anyhow!(
                    "invalid JSON configuration at line {}, column {}",
                    error.line(),
                    error.column()
                )
            })
    } else {
        let value: toml::Value =
            toml::from_str(text).map_err(|_| anyhow::anyhow!("invalid TOML configuration"))?;
        serde_json::to_value(value).context(say(
            "configuration cannot be represented as structured data",
        ))
    }
}

async fn fetch(source: &str, token_env: Option<&str>) -> anyhow::Result<Vec<u8>> {
    if source.len() > 16384 {
        bail!("configuration URL is too long");
    }
    let url = url::Url::parse(source).map_err(|_| anyhow::anyhow!("invalid configuration URL"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host().is_none()
    {
        bail!("configuration URL must have a host and cannot contain userinfo or a fragment");
    }
    let tls = felucca_transport::Verification::WebPki {
        server_name: String::new(),
    }
    .rustls_client_config()?;
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let client: hyper_util::client::legacy::Client<_, Empty<Bytes>> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);
    let mut request = http::Request::builder()
        .uri(url.as_str())
        .header("accept", "application/json, application/toml, text/plain")
        .body(Empty::<Bytes>::new())
        .map_err(|_| anyhow::anyhow!("invalid configuration request"))?;
    if let Some(name) = token_env {
        let token = std::env::var(name).map_err(|_| {
            anyhow::anyhow!("configuration token environment variable is unavailable")
        })?;
        if token.is_empty() || token.len() > 8192 {
            bail!("configuration token is empty or too large");
        }
        let mut value = http::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| anyhow::anyhow!("configuration token contains invalid header bytes"))?;
        value.set_sensitive(true);
        request
            .headers_mut()
            .insert(http::header::AUTHORIZATION, value);
    }
    tokio::time::timeout(TIMEOUT, async {
        let response = client
            .request(request)
            .await
            .map_err(|_| anyhow::anyhow!("configuration download failed"))?;
        // Hyper does not follow redirects; never forward credentials to another origin.
        if response.status() != http::StatusCode::OK {
            bail!(
                "configuration server returned HTTP {}",
                response.status().as_u16()
            );
        }
        if response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|size| size > MAX_BYTES as u64)
        {
            bail!("remote configuration exceeds 4 MiB");
        }
        if response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .is_some_and(|encoding| encoding != "identity")
        {
            bail!("compressed remote configuration is not supported");
        }
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame =
                frame.map_err(|_| anyhow::anyhow!("configuration download was interrupted"))?;
            if let Ok(data) = frame.into_data() {
                if data.len() > MAX_BYTES - bytes.len() {
                    bail!("remote configuration exceeds 4 MiB");
                }
                bytes.extend_from_slice(&data);
            }
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| anyhow::anyhow!("configuration download timed out after 15s"))?
}

fn expand(
    value: &mut Value,
    lookup: &impl Fn(&str) -> anyhow::Result<Option<String>>,
    budget: &mut usize,
) -> anyhow::Result<()> {
    match value {
        Value::String(text) => {
            *text = expand_string(text, lookup, *budget)?;
            *budget = budget
                .checked_sub(text.len())
                .context(say("expanded configuration exceeds 4 MiB"))?;
        }
        Value::Array(values) => {
            for value in values {
                expand(value, lookup, budget)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                expand(value, lookup, budget)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn expand_string(
    text: &str,
    lookup: &impl Fn(&str) -> anyhow::Result<Option<String>>,
    budget: usize,
) -> anyhow::Result<String> {
    let mut output = String::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        let value: Cow<'_, str>;
        if let Some(rest) = remaining.strip_prefix("$$") {
            value = "$".into();
            remaining = rest;
        } else if let Some(rest) = remaining.strip_prefix("${") {
            let (word, rest) = rest
                .split_once('}')
                .context(say("unterminated environment placeholder"))?;
            let (name, fallback) = word
                .split_once(":-")
                .map_or((word, None), |(name, fallback)| (name, Some(fallback)));
            if name.is_empty()
                || name.len() > 128
                || !name.bytes().enumerate().all(|(i, b)| {
                    b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit()
                })
                || word.contains('{')
            {
                bail!("invalid environment placeholder");
            }
            value = match lookup(name)? {
                Some(value) if !value.is_empty() || fallback.is_none() => Cow::Owned(value),
                _ => Cow::Borrowed(fallback.with_context(|| {
                    say(felucca_i18n::Message::with(
                        "environment variable {name} is not set",
                        [felucca_i18n::Value::new("name", name)],
                    ))
                })?),
            };
            remaining = rest;
        } else {
            let len = remaining.chars().next().expect("nonempty").len_utf8();
            value = (&remaining[..len]).into();
            remaining = &remaining[len..];
        }
        if value.len() > budget.saturating_sub(output.len()) {
            bail!("expanded configuration exceeds 4 MiB");
        }
        output.push_str(&value);
    }
    Ok(output)
}

struct UniqueValue(Value);
impl<'de> serde::Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON configuration")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<UniqueValue, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<UniqueValue, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<UniqueValue, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::Number(
                    serde_json::Number::from_f64(v).ok_or_else(|| E::custom("nonfinite number"))?,
                )))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UniqueValue, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<UniqueValue, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueValue, A::Error> {
                let mut fields = Map::new();
                while let Some((name, value)) = map.next_entry::<String, UniqueValue>()? {
                    if fields.insert(name, value.0).is_some() {
                        return Err(serde::de::Error::custom("duplicate configuration field"));
                    }
                }
                Ok(UniqueValue(Value::Object(fields)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[cfg(test)]
#[path = "config_source_tests.rs"]
mod tests;
