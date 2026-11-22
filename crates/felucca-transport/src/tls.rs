//! TLS for tunnel connections.
//!
//! The tunnel handshake in `felucca-proto` authenticates both endpoints with an
//! HMAC over the verify key, so TLS here is about confidentiality: keeping a
//! passive observer from reading tunnel traffic. That does not make certificate
//! checking optional. Verification is what stops an active attacker from
//! *reaching* the handshake at all, and defence that rests on a single layer is
//! one bug away from nothing.
//!
//! Most nps deployments use a self-signed bridge certificate, which no public CA
//! will vouch for. Rather than tell those operators to disable verification —
//! which is how "temporarily insecure" becomes permanent — [`Verification::Pinned`]
//! matches the certificate against a fingerprint they can read off their own
//! server.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::error::TransportError;

/// A certificate chain and its private key, in DER form.
#[derive(Debug, Clone)]
pub struct Identity {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: Arc<PrivateKeyDer<'static>>,
}

impl Identity {
    /// Loads a PEM certificate chain and key from disk.
    pub fn from_pem_files(cert: &Path, key: &Path) -> Result<Self, TransportError> {
        let cert_pem = std::fs::read(cert).map_err(|e| {
            TransportError::certificate(format!("cannot read {}: {e}", cert.display()))
        })?;
        let key_pem = std::fs::read(key).map_err(|e| {
            TransportError::certificate(format!("cannot read {}: {e}", key.display()))
        })?;
        Self::from_pem(&cert_pem, &key_pem)
    }

    /// Parses a PEM certificate chain and key.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, TransportError> {
        let chain: Vec<_> = rustls_pemfile::certs(&mut &*cert_pem)
            .collect::<Result<_, _>>()
            .map_err(|e| TransportError::certificate(format!("bad certificate PEM: {e}")))?;
        if chain.is_empty() {
            return Err(TransportError::certificate("no certificate found in PEM"));
        }

        let key = rustls_pemfile::private_key(&mut &*key_pem)
            .map_err(|e| TransportError::certificate(format!("bad key PEM: {e}")))?
            .ok_or_else(|| TransportError::certificate("no private key found in PEM"))?;

        Ok(Self {
            chain,
            key: Arc::new(key),
        })
    }

    /// Generates a self-signed certificate for `names`.
    ///
    /// Convenience for a first run, so a fresh install has a working TLS bridge
    /// before anyone has arranged a real certificate. Clients still have to pin
    /// it — see [`Identity::fingerprint`].
    pub fn self_signed(names: Vec<String>) -> Result<Self, TransportError> {
        let generated = rcgen::generate_simple_self_signed(names)
            .map_err(|e| TransportError::certificate(format!("cannot generate a cert: {e}")))?;
        Self::from_pem(
            generated.cert.pem().as_bytes(),
            generated.signing_key.serialize_pem().as_bytes(),
        )
    }

    /// SHA-256 fingerprint of the end-entity certificate, as lowercase hex.
    ///
    /// This is the value an operator pins on the agent side.
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.chain[0])
    }

    /// A `rustls` server configuration serving this identity.
    pub fn server_config(&self) -> Result<Arc<ServerConfig>, TransportError> {
        let key = self.key.clone_key();
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(self.chain.clone(), key)
            .map_err(|e| TransportError::tls(format!("cannot build a server config: {e}")))?;
        Ok(Arc::new(config))
    }

    /// A TLS acceptor serving this identity.
    pub fn acceptor(&self) -> Result<TlsAcceptor, TransportError> {
        Ok(TlsAcceptor::from(self.server_config()?))
    }
}

/// The process-wide crypto provider, installing one if nothing has yet.
///
/// `CryptoProvider::get_default` returns `None` until something builds a rustls
/// config, because that is when rustls installs the provider its crate features
/// name. A custom verifier is constructed *before* the config it belongs to, so
/// reading the default here finds nothing — and an agent that only ever builds
/// client configs would fail on its first connection. Installing explicitly
/// removes the ordering dependency: after `install_default` returns, a provider
/// is present whether this call or a concurrent one put it there.
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    if let Some(installed) = rustls::crypto::CryptoProvider::get_default() {
        return installed.clone();
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    rustls::crypto::CryptoProvider::get_default()
        .expect("install_default leaves a provider installed, even when it loses a race")
        .clone()
}

/// SHA-256 fingerprint of a DER certificate, as lowercase hex.
pub fn fingerprint_of(cert: &CertificateDer<'_>) -> String {
    let digest = Sha256::digest(cert.as_ref());
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        // Cannot fail: writing to a String.
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// How an agent decides whether to trust the server it reached.
#[derive(Debug, Clone)]
pub enum Verification {
    /// Check against the platform's public CA roots. Right for a server with a
    /// certificate from a real CA.
    WebPki { server_name: String },

    /// Accept exactly one certificate, named by its SHA-256 fingerprint.
    ///
    /// The answer for a self-signed bridge: as strong as CA verification against
    /// an active attacker, without needing a CA. The fingerprint is compared
    /// case-insensitively so an operator can paste it in either case.
    Pinned { fingerprint: String },

    /// Trust anything. An active attacker in the network path can read and
    /// modify TLS traffic; the tunnel's own mutual handshake is then the only
    /// thing keeping them out.
    ///
    /// Named `Insecure` rather than something softer because a caller writing
    /// this into a config should have to type the word.
    Insecure,
}

impl Verification {
    /// The name to send as SNI and check the certificate against.
    ///
    /// `fallback` is the host that was dialled, used when the policy does not
    /// name one itself.
    pub fn server_name_for(&self, fallback: &str) -> String {
        match self {
            Self::WebPki { server_name } => server_name.clone(),
            // The peer may still key its certificate selection on SNI, so send
            // the host we dialled even when we are not checking the name.
            _ => fallback.to_owned(),
        }
    }

    fn build(&self) -> Result<Arc<ClientConfig>, TransportError> {
        Ok(Arc::new(self.client_config(rustls::DEFAULT_VERSIONS)?))
    }

    /// An owned client configuration for HTTP connectors and other TLS users.
    pub fn rustls_client_config(&self) -> Result<ClientConfig, TransportError> {
        self.client_config(rustls::DEFAULT_VERSIONS)
    }

    /// A client configuration restricted to TLS 1.3.
    ///
    /// QUIC mandates TLS 1.3 and rejects a configuration that also offers 1.2,
    /// so the version list cannot be left at the default. Returns an owned config
    /// because quinn needs to set ALPN on it before converting.
    pub fn rustls_client_config_tls13(&self) -> Result<ClientConfig, TransportError> {
        self.client_config(&[&rustls::version::TLS13])
    }

    fn client_config(
        &self,
        versions: &[&'static rustls::SupportedProtocolVersion],
    ) -> Result<ClientConfig, TransportError> {
        let _ = provider();
        let config = match self {
            Self::WebPki { .. } => {
                let roots = RootCertStore {
                    roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
                };
                ClientConfig::builder_with_protocol_versions(versions)
                    .with_root_certificates(roots)
                    .with_no_client_auth()
            }
            Self::Pinned { fingerprint } => {
                let verifier = PinnedVerifier::new(fingerprint)?;
                ClientConfig::builder_with_protocol_versions(versions)
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(verifier))
                    .with_no_client_auth()
            }
            Self::Insecure => {
                tracing::warn!(
                    "TLS certificate verification is disabled; an attacker in the network \
                     path can read this tunnel's traffic"
                );
                ClientConfig::builder_with_protocol_versions(versions)
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(NoVerification::new()))
                    .with_no_client_auth()
            }
        };
        Ok(config)
    }

    /// A connector implementing this policy.
    pub fn connector(&self) -> Result<TlsConnector, TransportError> {
        Ok(TlsConnector::from(self.build()?))
    }
}

/// Connects to `addr` and completes a TLS handshake under `policy`.
pub async fn connect(
    addr: &str,
    policy: &Verification,
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, TransportError> {
    connect_with_alpn(addr, policy, &[]).await
}

pub async fn connect_with_alpn(
    addr: &str,
    policy: &Verification,
    alpn: &[&[u8]],
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, TransportError> {
    let tcp = crate::tcp::connect(addr).await?;

    // Strip the port: a certificate names a host, not an endpoint.
    let host = addr
        .rsplit_once(':')
        .map_or(addr, |(h, _)| h)
        .trim_matches(['[', ']']);
    handshake(tcp, host, policy, alpn).await
}

/// Dial an already resolved and policy-checked address, retaining the DNS name
/// for SNI and certificate verification.
pub async fn connect_to(
    address: std::net::SocketAddr,
    host: &str,
    policy: &Verification,
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, TransportError> {
    connect_to_with_alpn(address, host, policy, &[]).await
}

pub async fn connect_to_with_alpn(
    address: std::net::SocketAddr,
    host: &str,
    policy: &Verification,
    alpn: &[&[u8]],
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, TransportError> {
    let tcp = crate::tcp::connect(&address.to_string()).await?;
    handshake(tcp, host, policy, alpn).await
}

/// Complete TLS over an already connected socket or upstream CONNECT stream.
pub async fn handshake<T>(
    tcp: T,
    host: &str,
    policy: &Verification,
    alpn: &[&[u8]],
) -> Result<tokio_rustls::client::TlsStream<T>, TransportError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let name = policy.server_name_for(host);
    let server_name = ServerName::try_from(name.clone())
        .map_err(|_| TransportError::tls(format!("{name:?} is not a valid server name")))?
        .to_owned();

    let mut config = policy.client_config(rustls::DEFAULT_VERSIONS)?;
    config.alpn_protocols = alpn.iter().map(|value| value.to_vec()).collect();
    let stream = TlsConnector::from(Arc::new(config))
        .connect(server_name, tcp)
        .await
        .map_err(|e| TransportError::tls(format!("handshake with {host} failed: {e}")))?;
    if alpn == [b"h2".as_slice()] && stream.get_ref().1.alpn_protocol() != Some(b"h2") {
        return Err(TransportError::tls("backend did not negotiate h2"));
    }
    Ok(stream)
}

/// Verifies that the peer presents one specific certificate.
#[derive(Debug)]
struct PinnedVerifier {
    /// Lowercase hex, so comparison needs no per-connection allocation.
    want: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl PinnedVerifier {
    fn new(fingerprint: &str) -> Result<Self, TransportError> {
        // Accept the separator styles `openssl x509 -fingerprint` and friends
        // emit, so an operator can paste what their tooling printed.
        let want: String = fingerprint
            .chars()
            .filter(|c| !matches!(c, ':' | ' ' | '-'))
            .flat_map(char::to_lowercase)
            .collect();

        if want.len() != 64 || !want.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(TransportError::certificate(format!(
                "a pinned fingerprint must be 64 hex characters (SHA-256), got {} characters",
                want.len()
            )));
        }
        Ok(Self {
            want,
            provider: provider(),
        })
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // The pin replaces name and chain checking: it identifies one exact key,
        // which is a stronger statement than "some CA vouches for this name".
        // Expiry is deliberately not enforced — a pinned self-signed certificate
        // outliving its notAfter should not take a tunnel down.
        let got = fingerprint_of(end_entity);
        if got == self.want {
            Ok(ServerCertVerified::assertion())
        } else {
            tracing::warn!(expected = %self.want, got = %got, "pinned certificate mismatch");
            Err(rustls::Error::General(
                "certificate does not match the pinned fingerprint".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // Delegated to rustls: pinning changes which certificate we accept, not
        // whether the peer proves it holds the matching private key. Skipping
        // this would let anyone replay a captured certificate.
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Accepts any certificate. See [`Verification::Insecure`].
#[derive(Debug)]
struct NoVerification {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl NoVerification {
    fn new() -> Self {
        Self {
            provider: provider(),
        }
    }
}

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn identity() -> Identity {
        Identity::self_signed(vec!["localhost".to_owned()]).expect("generation should work")
    }

    /// Serves one TLS connection, echoing what it reads. Returns the port.
    async fn serve(identity: &Identity) -> u16 {
        let acceptor = identity.acceptor().unwrap();
        let listener = crate::tcp::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let Ok((tcp, _)) = crate::tcp::accept(&listener).await else {
                return;
            };
            let Ok(mut tls) = acceptor.accept(tcp).await else {
                return;
            };
            let mut buf = [0u8; 5];
            if tls.read_exact(&mut buf).await.is_ok() {
                let _ = tls.write_all(&buf).await;
                let _ = tls.flush().await;
            }
        });
        port
    }

    #[test]
    fn a_verifier_can_be_built_before_any_rustls_config_exists() {
        // The bug this guards: reading the lazily-installed default provider
        // before anything has built a config found nothing, so an agent whose
        // first action was to build a pinned client config failed to start.
        assert!(
            !provider()
                .signature_verification_algorithms
                .supported_schemes()
                .is_empty()
        );
    }

    #[test]
    fn a_self_signed_identity_has_a_usable_fingerprint() {
        let id = identity();
        let fp = id.fingerprint();
        assert_eq!(fp.len(), 64, "SHA-256 hex is 64 characters: {fp}");
        assert!(
            fp.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn two_generated_identities_differ() {
        assert_ne!(identity().fingerprint(), identity().fingerprint());
    }

    #[test]
    fn an_identity_round_trips_through_pem() {
        let id = identity();
        let server = id.server_config();
        assert!(server.is_ok(), "{:?}", server.err());
    }

    #[test]
    fn a_pem_without_a_certificate_is_refused() {
        let err = Identity::from_pem(b"not pem at all", b"nor this").expect_err("must refuse");
        assert!(matches!(err, TransportError::Certificate(_)), "{err}");
    }

    #[test]
    fn a_certificate_without_its_key_is_refused() {
        let id = identity();
        let cert_pem = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            "AAAA".repeat(16)
        );
        // Well-formed PEM framing, but no key alongside it.
        let err = Identity::from_pem(cert_pem.as_bytes(), b"").expect_err("must refuse");
        assert!(matches!(err, TransportError::Certificate(_)), "{err}");
        drop(id);
    }

    #[test]
    fn a_fingerprint_is_accepted_in_the_shapes_tools_print() {
        let bare = "a".repeat(64);
        let colons = (0..32).map(|_| "AA").collect::<Vec<_>>().join(":");

        for fp in [bare, colons] {
            // `openssl x509 -fingerprint` prints uppercase hex with colons.
            assert!(
                Verification::Pinned {
                    fingerprint: fp.clone()
                }
                .connector()
                .is_ok(),
                "{fp} should be accepted"
            );
        }
    }

    #[test]
    fn a_malformed_fingerprint_is_rejected_at_config_time() {
        // Better to fail when the config is read than on first connection.
        for bad in ["", "abc", &"a".repeat(63), &"a".repeat(65), &"z".repeat(64)] {
            // `TlsConnector` is not Debug, so match rather than unwrap.
            let policy = Verification::Pinned {
                fingerprint: bad.to_owned(),
            };
            match policy.connector() {
                Err(TransportError::Certificate(_)) => {}
                Err(other) => panic!("{bad:?}: wrong error {other}"),
                Ok(_) => panic!("{bad:?} should have been rejected"),
            }
        }
    }

    #[tokio::test]
    async fn a_pinned_client_reaches_the_matching_server() {
        let id = identity();
        let port = serve(&id).await;

        let policy = Verification::Pinned {
            fingerprint: id.fingerprint(),
        };
        let mut tls = connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .expect("the pin matches, so this must connect");

        tls.write_all(b"hello").await.unwrap();
        let mut back = [0u8; 5];
        tls.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"hello");
    }

    #[tokio::test]
    async fn a_pinned_client_refuses_a_different_certificate() {
        // The attack this defends against: something else answering on the
        // bridge's address with its own valid certificate.
        let server_id = identity();
        let port = serve(&server_id).await;

        let policy = Verification::Pinned {
            fingerprint: identity().fingerprint(),
        };
        let err = connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .expect_err("a mismatched pin must not connect");
        assert!(matches!(err, TransportError::Tls(_)), "{err}");
    }

    #[tokio::test]
    async fn an_uppercase_pin_matches_too() {
        let id = identity();
        let port = serve(&id).await;

        let policy = Verification::Pinned {
            fingerprint: id.fingerprint().to_uppercase(),
        };
        assert!(
            connect(&format!("127.0.0.1:{port}"), &policy).await.is_ok(),
            "case should not matter for a hex fingerprint"
        );
    }

    #[tokio::test]
    async fn insecure_verification_accepts_an_unknown_certificate() {
        let id = identity();
        let port = serve(&id).await;

        let mut tls = connect(&format!("127.0.0.1:{port}"), &Verification::Insecure)
            .await
            .expect("insecure mode accepts anything");
        tls.write_all(b"plain").await.unwrap();
        let mut back = [0u8; 5];
        tls.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"plain");
    }

    #[tokio::test]
    async fn webpki_verification_rejects_a_self_signed_certificate() {
        // The reason `Pinned` exists: a self-signed bridge cert cannot pass CA
        // verification, and the answer is a pin rather than switching checking off.
        let id = identity();
        let port = serve(&id).await;

        let policy = Verification::WebPki {
            server_name: "localhost".to_owned(),
        };
        let err = connect(&format!("127.0.0.1:{port}"), &policy)
            .await
            .expect_err("no CA vouches for a self-signed cert");
        assert!(matches!(err, TransportError::Tls(_)), "{err}");
    }

    #[tokio::test]
    async fn a_bracketed_ipv6_address_is_parsed_for_sni() {
        // The host has to be extracted without leaving brackets behind, or
        // ServerName parsing rejects it before any socket is touched.
        let err = connect("[::1]:1", &Verification::Insecure)
            .await
            .expect_err("nothing listens");
        assert!(
            matches!(err, TransportError::Connect { .. }),
            "should fail connecting, not parsing: {err}"
        );
    }
}
