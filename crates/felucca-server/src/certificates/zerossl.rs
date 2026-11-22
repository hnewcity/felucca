//! Obtain EAB credentials, then use the regular durable ACME account workflow.
use anyhow::{Context, bail};
use bytes::Bytes;
use felucca_core::{Secret, config::ExternalAccountBinding};
use http_body_util::{BodyExt, Empty, Limited};
use hyper_util::rt::TokioIo;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

pub async fn credentials(key: &Secret) -> anyhow::Result<ExternalAccountBinding> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let tls = felucca_transport::tls::connect(
            "api.zerossl.com:443",
            &felucca_transport::Verification::WebPki {
                server_name: "api.zerossl.com".into(),
            },
        )
        .await
        .context("ZeroSSL API TLS connection failed")?;
        request(tls, key).await
    })
    .await
    .context("ZeroSSL API request timed out")?
}

async fn request<T>(io: T, key: &Secret) -> anyhow::Result<ExternalAccountBinding>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // The API takes its key in the query. Never include this URI or response
    // body in errors/traces, and never follow a redirect carrying that key.
    let mut encoded = String::new();
    for byte in key.as_bytes() {
        use std::fmt::Write;
        if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) {
            encoded.push(char::from(*byte));
        } else {
            write!(encoded, "%{byte:02X}").expect("writing a string");
        }
    }
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .context("ZeroSSL API connection failed")?;
    let mut drivers = tokio::task::JoinSet::new();
    drivers.spawn(async move {
        let _ = connection.await;
    });
    let request = http::Request::builder()
        .method(http::Method::POST)
        .uri(format!("/acme/eab-credentials?access_key={encoded}"))
        .header(http::header::HOST, "api.zerossl.com")
        .header(http::header::CONTENT_LENGTH, "0")
        .header(http::header::CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| anyhow::anyhow!("invalid ZeroSSL API request"))?;
    let response = sender
        .send_request(request)
        .await
        .context("ZeroSSL API request failed")?;
    if !response.status().is_success() {
        bail!("ZeroSSL API returned HTTP {}", response.status());
    }
    let data = Limited::new(response.into_body(), 16384)
        .collect()
        .await
        .map_err(|_| anyhow::anyhow!("ZeroSSL API response exceeded its limit or was interrupted"))?
        .to_bytes();
    #[derive(serde::Deserialize)]
    struct Credentials {
        success: bool,
        eab_kid: Option<String>,
        eab_hmac_key: Option<Secret>,
    }
    let result: Credentials = serde_json::from_slice(&data)
        .map_err(|_| anyhow::anyhow!("invalid ZeroSSL API response"))?;
    if !result.success {
        bail!("ZeroSSL rejected the EAB credential request");
    }
    let binding = ExternalAccountBinding {
        key_id: result
            .eab_kid
            .context("ZeroSSL did not return an EAB key ID")?,
        hmac_key: result
            .eab_hmac_key
            .context("ZeroSSL did not return an EAB HMAC key")?,
    };
    binding.key_bytes()?;
    Ok(binding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn api_request_encodes_credentials_and_validates_response_without_disclosing_keys() {
        for (status, body, success) in [
            (
                "200 OK",
                r#"{"success":true,"eab_kid":"account-1","eab_hmac_key":"aGVsbG8"}"#.to_owned(),
                true,
            ),
            (
                "200 OK",
                r#"{"success":false,"error":{"message":"private&key?"}}"#.into(),
                false,
            ),
            (
                "302 Found",
                r#"{"redirect":"https://other.test/"}"#.into(),
                false,
            ),
            ("200 OK", "x".repeat(16385), false),
            (
                "200 OK",
                r#"{"success":true,"eab_kid":"account-1","eab_hmac_key":"!invalid"}"#.into(),
                false,
            ),
        ] {
            let (client, mut server) = tokio::io::duplex(32768);
            let task = tokio::spawn(async move {
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    header.push(server.read_u8().await.unwrap());
                }
                let text = String::from_utf8(header).unwrap();
                assert!(text.starts_with(
                    "POST /acme/eab-credentials?access_key=private%26key%3F HTTP/1.1\r\n"
                ));
                assert!(text.to_ascii_lowercase().contains("host: api.zerossl.com"));
                server.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let result = request(client, &Secret::new("private&key?")).await;
            assert_eq!(result.is_ok(), success, "{result:?}");
            if let Ok(binding) = result {
                assert_eq!(binding.key_id, "account-1");
                assert_eq!(binding.key_bytes().unwrap(), b"hello");
            } else if let Err(error) = result {
                let shown = format!("{error:#}");
                assert!(!shown.contains("private") && !shown.contains("invalid\""));
            }
            task.await.unwrap();
        }
    }
}
