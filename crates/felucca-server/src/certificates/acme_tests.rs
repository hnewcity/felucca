//! A local ACME authority exercises the real order driver and HTTP-01 handler.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use instant_acme::{BodyWrapper, BytesResponse, HttpClient};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

#[derive(Default)]
struct State {
    validated: bool,
    challenge_checked: bool,
    certificate: Option<String>,
}

struct Authority {
    state: Arc<Mutex<State>>,
    proxy: Arc<ReverseProxy>,
    http_port: u16,
    fail_validation: bool,
    kind: AcmeChallenge,
}

fn order(status: &str) -> Value {
    json!({
        "status": status,
        "authorizations": ["https://ca.test/auth/1"],
        "finalize": "https://ca.test/finalize/1",
        "certificate": (status == "valid").then_some("https://ca.test/certificate/1")
    })
}

impl HttpClient for Authority {
    fn request(
        &self,
        request: http::Request<BodyWrapper<Bytes>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>,
    > {
        let state = self.state.clone();
        let proxy = self.proxy.clone();
        let port = self.http_port;
        let fail = self.fail_validation;
        let kind = self.kind;
        let challenge_type = match kind {
            AcmeChallenge::Http01 => "http-01",
            AcmeChallenge::TlsAlpn01 => "tls-alpn-01",
        };
        Box::pin(async move {
            let path = request.uri().path().to_owned();
            let body = request.into_body().collect().await.unwrap().to_bytes();
            let payload: Value = if body.is_empty() {
                Value::Null
            } else {
                let jws: Value = serde_json::from_slice(&body).unwrap();
                let payload = URL_SAFE_NO_PAD
                    .decode(jws["payload"].as_str().unwrap())
                    .unwrap();
                if payload.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&payload).unwrap()
                }
            };
            let mut response =
                http::Response::builder().header("Replay-Nonce", uuid::Uuid::new_v4().to_string());
            let result = match path.as_str() {
                "/directory" => json!({
                    "newNonce": "https://ca.test/nonce",
                    "newAccount": "https://ca.test/account",
                    "newOrder": "https://ca.test/order"
                }),
                "/nonce" => Value::Null,
                "/account" => {
                    assert_eq!(payload["termsOfServiceAgreed"], true);
                    use hmac::{KeyInit, Mac};
                    let binding = &payload["externalAccountBinding"];
                    let protected = binding["protected"].as_str().unwrap();
                    let embedded = binding["payload"].as_str().unwrap();
                    let header: Value =
                        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(protected).unwrap())
                            .unwrap();
                    assert_eq!(header["kid"], "test-key");
                    assert_eq!(header["alg"], "HS256");
                    assert_eq!(header["url"], "https://ca.test/account");
                    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&[7; 32]).unwrap();
                    mac.update(format!("{protected}.{embedded}").as_bytes());
                    mac.verify_slice(
                        &URL_SAFE_NO_PAD
                            .decode(binding["signature"].as_str().unwrap())
                            .unwrap(),
                    )
                    .unwrap();
                    response = response
                        .status(201)
                        .header("Location", "https://ca.test/account/1");
                    json!({"status": "valid"})
                }
                "/order" => {
                    assert_eq!(payload["identifiers"][0]["value"], "example.test");
                    response = response
                        .status(201)
                        .header("Location", "https://ca.test/order/1");
                    order("pending")
                }
                "/auth/1" => json!({
                    "status": "pending",
                    "identifier": {"type": "dns", "value": "example.test"},
                    "challenges": [{
                        "type": challenge_type, "url": "https://ca.test/challenge/1",
                        "token": "test-token", "status": "pending"
                    }]
                }),
                "/challenge/1" => {
                    // Verify exactly the key authorization published by the driver,
                    // through the real network listener before accepting validation.
                    let expected = proxy
                        .challenges
                        .read()
                        .get(&("example.test".into(), "test-token".into()))
                        .unwrap()
                        .clone();
                    assert!(expected.starts_with("test-token."));
                    if kind == AcmeChallenge::Http01 {
                        let mut io = tokio::net::TcpStream::connect(("127.0.0.1", port))
                            .await
                            .unwrap();
                        io.write_all(b"GET /.well-known/acme-challenge/test-token HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n").await.unwrap();
                        let mut received = String::new();
                        io.read_to_string(&mut received).await.unwrap();
                        assert!(received.starts_with("HTTP/1.1 200"), "{received}");
                        assert_eq!(received.split_once("\r\n\r\n").unwrap().1, expected);
                    } else {
                        use sha2::Digest;
                        let mut config = rustls::ClientConfig::builder_with_protocol_versions(&[
                            &rustls::version::TLS13,
                        ])
                        .dangerous()
                        .with_custom_certificate_verifier(Arc::new(ChallengeVerifier))
                        .with_no_client_auth();
                        config.alpn_protocols = vec![b"acme-tls/1".to_vec()];
                        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
                            .await
                            .unwrap();
                        let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                            .connect("example.test".try_into().unwrap(), tcp)
                            .await
                            .unwrap();
                        assert_eq!(
                            tls.get_ref().1.alpn_protocol(),
                            Some(b"acme-tls/1".as_slice())
                        );
                        let leaf = &tls.get_ref().1.peer_certificates().unwrap()[0];
                        let (_, cert) = X509Certificate::from_der(leaf.as_ref()).unwrap();
                        assert!(
                            cert.subject_alternative_name()
                                .unwrap()
                                .unwrap()
                                .value
                                .general_names
                                .iter()
                                .any(|name| matches!(
                                    name,
                                    x509_parser::extensions::GeneralName::DNSName("example.test")
                                ))
                        );
                        let extension = cert
                            .extensions()
                            .iter()
                            .find(|extension| extension.oid.to_id_string() == "1.3.6.1.5.5.7.1.31")
                            .unwrap();
                        assert!(extension.critical);
                        let mut digest = vec![4, 32]; // DER OCTET STRING
                        digest.extend_from_slice(&sha2::Sha256::digest(expected.as_bytes()));
                        assert_eq!(extension.value, digest);
                    }
                    state.lock().challenge_checked = true;
                    if fail {
                        response = response.status(400);
                        json!({"type": "urn:ietf:params:acme:error:unauthorized", "detail": "simulated validation failure", "status": 400})
                    } else {
                        state.lock().validated = true;
                        json!({"type": challenge_type, "url": "https://ca.test/challenge/1", "token": "test-token", "status": "valid"})
                    }
                }
                "/order/1" => {
                    let state = state.lock();
                    assert!(state.validated);
                    order(if state.certificate.is_some() {
                        "valid"
                    } else {
                        "ready"
                    })
                }
                "/finalize/1" => {
                    assert!(state.lock().validated);
                    let csr = URL_SAFE_NO_PAD
                        .decode(payload["csr"].as_str().unwrap())
                        .unwrap();
                    let params =
                        rcgen::CertificateSigningRequestParams::from_der(&csr.into()).unwrap();
                    let issuer = rcgen::Issuer::new(
                        rcgen::CertificateParams::new(vec!["local-test-ca".into()]).unwrap(),
                        rcgen::KeyPair::generate().unwrap(),
                    );
                    state.lock().certificate = Some(params.signed_by(&issuer).unwrap().pem());
                    order("valid")
                }
                "/certificate/1" => {
                    return Ok(response
                        .body(Full::new(Bytes::from(
                            state.lock().certificate.clone().unwrap(),
                        )))
                        .unwrap()
                        .into());
                }
                other => panic!("unexpected ACME request {other}"),
            };
            Ok(response
                .body(Full::new(Bytes::from(serde_json::to_vec(&result).unwrap())))
                .unwrap()
                .into())
        })
    }
}

async fn exercise(fail: bool, kind: AcmeChallenge) {
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let proxy = Arc::new(
        ReverseProxy::new(felucca_core::config::HttpProxyConfig {
            bind: (kind == AcmeChallenge::Http01).then(|| format!("127.0.0.1:{port}")),
            tls_bind: (kind == AcmeChallenge::TlsAlpn01).then(|| format!("127.0.0.1:{port}")),
            ..Default::default()
        })
        .unwrap(),
    );
    let cancel = CancellationToken::new();
    proxy.start(cancel.clone()).await.unwrap();
    let state = Arc::new(Mutex::new(State::default()));
    let (account, credentials) = Account::builder_with_http(Box::new(Authority {
        state: state.clone(),
        proxy: proxy.clone(),
        http_port: port,
        fail_validation: fail,
        kind,
    }))
    .create(
        &NewAccount {
            contact: &["mailto:owner@example.test"],
            terms_of_service_agreed: true,
            only_return_existing: false,
        },
        "https://ca.test/directory".into(),
        Some(&instant_acme::ExternalAccountKey::new(
            "test-key".into(),
            &[7; 32],
        )),
    )
    .await
    .unwrap();
    // The serialized credentials must survive a process restart.
    let credentials: AccountCredentials =
        serde_json::from_slice(&serde_json::to_vec(&credentials).unwrap()).unwrap();
    drop(account);
    let account = Account::builder_with_http(Box::new(Authority {
        state: state.clone(),
        proxy: proxy.clone(),
        http_port: port,
        fail_validation: fail,
        kind,
    }))
    .from_credentials(credentials)
    .await
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        issue(&account, proxy.clone(), "example.test", kind),
    )
    .await
    .unwrap();
    assert!(
        state.lock().challenge_checked,
        "must reach real challenge validation"
    );
    assert!(
        proxy.challenges.read().is_empty(),
        "tokens must be removed on both success and error"
    );
    assert!(
        proxy.tls_challenges.read().is_empty(),
        "temporary TLS challenge certificates must be removed"
    );
    if fail {
        assert!(result.is_err());
        assert!(state.lock().certificate.is_none());
    } else {
        let certificate = result.unwrap();
        let identity =
            Identity::from_pem(certificate.chain.as_bytes(), certificate.key.as_bytes()).unwrap();
        assert!(renewal_at(&identity, "example.test", Duration::from_secs(30 * 86400)).is_ok());
        // Validates that the certificate returned by the CA matches the CSR's key.
        proxy
            .certificates
            .insert("example.test", &identity)
            .unwrap();
        assert!(state.lock().validated);
    }
    cancel.cancel();
}

/// The local CA validates the ACME extension above. Verify CertificateVerify
/// with its actual public key without webpki rejecting that critical extension.
#[derive(Debug)]
struct ChallengeVerifier;
impl rustls::client::danger::ServerCertVerifier for ChallengeVerifier {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("test CA requires TLS 1.3".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        let (_, cert) = X509Certificate::from_der(cert.as_ref()).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;
        let provider = rustls::crypto::ring::default_provider();
        let algorithm = provider
            .signature_verification_algorithms
            .mapping
            .iter()
            .find(|(scheme, _)| *scheme == dss.scheme)
            .unwrap()
            .1[0];
        algorithm
            .verify_signature(
                cert.public_key().subject_public_key.data.as_ref(),
                message,
                dss.signature(),
            )
            .map_err(|_| {
                rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature)
            })?;
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}

#[tokio::test]
async fn http01_order_finalizes_a_real_csr_and_installs_a_matching_certificate() {
    exercise(false, AcmeChallenge::Http01).await;
}

#[tokio::test]
async fn a_failed_authorization_removes_the_published_token() {
    exercise(true, AcmeChallenge::Http01).await;
}

#[tokio::test]
async fn tls_alpn_orders_validate_the_critical_identifier_over_a_real_tls_connection() {
    exercise(false, AcmeChallenge::TlsAlpn01).await;
}

#[tokio::test]
async fn a_failed_tls_alpn_order_removes_its_temporary_certificate() {
    exercise(true, AcmeChallenge::TlsAlpn01).await;
}
