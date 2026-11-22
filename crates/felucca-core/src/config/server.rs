//! Server configuration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::acl::AclMode;
use crate::error::ConfigError;
use crate::secret::Secret;
use crate::traffic::Quota;
use felucca_i18n::{Message, Value};

/// A whole server configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default)]
    pub node: Option<super::node::NodeConfig>,
    #[serde(default)]
    pub p2p: Option<super::peer::P2pServerConfig>,
    #[serde(default, rename = "share")]
    pub shares: HashMap<String, super::ShareConfig>,
    #[serde(default)]
    pub admin: Option<super::AdminConfig>,
    #[serde(default)]
    pub managed: HashMap<String, super::ManagedConfig>,
    #[serde(default, rename = "local")]
    pub locals: HashMap<String, super::LocalConfig>,
    #[serde(default)]
    pub ntp: Option<super::NtpConfig>,
    /// Atomic runtime snapshots. Absent keeps runtime state in memory.
    #[serde(default)]
    pub state: Option<StateConfig>,
    #[serde(default)]
    pub geo: crate::geodata::GeoFiles,
    #[serde(default)]
    pub acl_mode: AclMode,
    #[serde(default)]
    pub acl_rules: String,
    #[serde(default, rename = "user")]
    pub users: HashMap<String, UserEntry>,
    #[serde(default)]
    pub bridge: BridgeConfig,

    /// The HTTP/HTTPS reverse proxy. Absent means it does not run.
    #[serde(default)]
    pub http: Option<HttpProxyConfig>,

    #[serde(default)]
    pub log: LogConfig,

    /// Agents allowed to connect, keyed by their `key_id`.
    #[serde(default, rename = "client")]
    pub clients: HashMap<String, ClientEntry>,

    /// Ports a tunnel may ask for. Empty means any port above 1024.
    #[serde(default)]
    pub allowed_ports: Vec<PortRange>,
}

impl ServerConfig {
    /// Loads and validates a server config.
    pub fn from_file(path: &std::path::Path) -> Result<Self, ConfigError> {
        let mut config: Self = super::load(path)?;
        config.validate()?;
        Ok(config)
    }

    /// Checks what serde cannot.
    pub fn validate(&mut self) -> Result<(), ConfigError> {
        if let Some(p2p) = &self.p2p {
            p2p.validate()?;
            let bind = super::parse_bind(&p2p.bind)?;
            for other in [
                self.bridge.quic_bind.as_ref(),
                self.bridge.kcp_bind.as_ref(),
                self.http.as_ref().and_then(|h| h.http3_bind.as_ref()),
            ]
            .into_iter()
            .flatten()
            {
                if super::parse_bind(other)? == bind {
                    return Err(ConfigError::invalid("p2p requires its own UDP listener"));
                }
            }
        }
        if let Some(ntp) = &self.ntp {
            ntp.validate()?;
        }
        if self
            .state
            .as_ref()
            .is_some_and(|state| state.path.as_os_str().is_empty() || state.save_interval.is_zero())
        {
            return Err(ConfigError::invalid(
                "state needs a nonempty path and a positive save_interval",
            ));
        }
        let geo = crate::geodata::GeoResolver::new(&self.geo)?;
        crate::Acl::parse_with_geo(self.acl_mode, &self.acl_rules, &geo).map_err(|e| {
            ConfigError::invalid(Message::with(
                "global ACL: {reason}",
                [Value::nested("reason", e.message())],
            ))
        })?;
        for (id, user) in &self.users {
            if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
                return Err(ConfigError::invalid(
                    "user ids require 1..128 bytes without control characters",
                ));
            }
            crate::Acl::parse_with_geo(user.acl_mode, &user.acl_rules, &geo).map_err(|e| {
                ConfigError::invalid(Message::with(
                    "user {id}: {reason}",
                    [Value::debug("id", id), Value::nested("reason", e.message())],
                ))
            })?;
        }
        if let Some(admin) = &self.admin {
            admin.validate(&self.users)?;
        }
        if let Some(node) = &self.node {
            node.validate(&self.users)?;
            let Some(admin) = &self.admin else {
                return Err(ConfigError::invalid("node control requires admin"));
            };
            if node.state_path == admin.state_path
                || self
                    .state
                    .as_ref()
                    .is_some_and(|s| s.path == node.state_path)
            {
                return Err(ConfigError::invalid(
                    "node journal must have its own state path",
                ));
            }
        }
        self.bridge.validate()?;
        self.log.validate()?;
        if let Some(http) = &mut self.http {
            http.validate()?;
            if let (Some(kcp), Some(h3)) = (&self.bridge.kcp_bind, &http.http3_bind)
                && super::parse_bind(kcp)? == super::parse_bind(h3)?
            {
                return Err(ConfigError::invalid(
                    "KCP and HTTP/3 require different UDP listeners",
                ));
            }
            let bridge = super::parse_bind(&self.bridge.bind)?;
            if [&http.bind, &http.tls_bind]
                .into_iter()
                .flatten()
                .any(|bind| super::parse_bind(bind).is_ok_and(|addr| addr == bridge))
                && self.bridge.proxy_accept != http.proxy_accept
            {
                return Err(ConfigError::invalid(
                    "shared bridge/HTTP listeners must use identical proxy_accept policies",
                ));
            }
        }

        if self.clients.is_empty() && self.locals.is_empty() && self.admin.is_none() {
            // A server with no clients accepts nobody. Better to say so at
            // startup than to leave an operator watching failed handshakes.
            return Err(ConfigError::invalid(
                "no `[client.<id>]` entries, so no agent could authenticate",
            ));
        }
        for (id, client) in &self.clients {
            if id.starts_with("local.") {
                return Err(ConfigError::invalid(
                    "client ids beginning local. are reserved",
                ));
            }
            if client
                .user
                .as_ref()
                .is_some_and(|user| !self.users.contains_key(user))
            {
                return Err(ConfigError::invalid(Message::with(
                    "client {id} names an unknown user",
                    [Value::debug("id", id)],
                )));
            }
            if client.vkey.is_empty() {
                return Err(ConfigError::invalid(Message::with(
                    "client {id} has an empty vkey",
                    [Value::debug("id", id)],
                )));
            }
            crate::acl::Acl::parse_with_geo(client.acl_mode, &client.acl_rules, &geo).map_err(
                |e| {
                    ConfigError::invalid(Message::with(
                        "client {id}: {reason}",
                        [Value::debug("id", id), Value::nested("reason", e.message())],
                    ))
                },
            )?;
        }

        if self.shares.len() > 4096 {
            return Err(ConfigError::invalid("at most 4096 shares are supported"));
        }
        for (name, share) in &self.shares {
            if !super::local::valid_name(name) {
                return Err(ConfigError::invalid("invalid share name"));
            }
            share.validate(&geo)?;
            let local = share
                .client
                .strip_prefix("local.")
                .is_some_and(|name| self.locals.contains_key(name));
            if share.direct && (local || self.p2p.is_none()) {
                return Err(ConfigError::invalid(
                    "direct shares require p2p and an agent provider",
                ));
            }
            if !local && !self.clients.contains_key(&share.client) {
                return Err(ConfigError::invalid(
                    "share names an unknown provider client",
                ));
            }
            if share
                .allow_clients
                .iter()
                .any(|name| !self.clients.contains_key(name))
            {
                return Err(ConfigError::invalid(
                    "share names an unknown visitor identity",
                ));
            }
        }
        for (key, managed) in &mut self.managed {
            if !self.clients.contains_key(key) {
                return Err(ConfigError::invalid(Message::with(
                    "managed configuration names unknown client {key}",
                    [Value::debug("key", key)],
                )));
            }
            managed.validate(&geo)?;
        }
        for range in &self.allowed_ports {
            range.validate()?;
        }
        for (name, local) in &mut self.locals {
            if !super::local::valid_name(name) || name.len() > 122 {
                return Err(ConfigError::invalid(
                    "local group names require 1..122 ASCII letters, digits, dots, hyphens or underscores",
                ));
            }
            if local
                .user
                .as_ref()
                .is_some_and(|user| !self.users.contains_key(user))
            {
                return Err(ConfigError::invalid(Message::with(
                    "local {name} names an unknown user",
                    [Value::debug("name", name)],
                )));
            }
            local.validate(&geo)?;
        }
        Ok(())
    }

    /// Whether a tunnel may listen on `port`.
    pub fn port_allowed(&self, port: u16) -> bool {
        if port == 0 {
            return false;
        }
        if self.allowed_ports.is_empty() {
            // Binding a privileged port needs privileges the server should not
            // be running with, so the default range starts above them.
            return port > 1024;
        }
        self.allowed_ports.iter().any(|r| r.contains(port))
    }
}

/// An inclusive range of ports, or a single port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PortRange {
    Single(u16),
    Range { from: u16, to: u16 },
}

impl PortRange {
    fn validate(&self) -> Result<(), ConfigError> {
        match self {
            Self::Single(0) => Err(ConfigError::invalid("port 0 is not a port")),
            Self::Single(_) => Ok(()),
            Self::Range { from, to } if from > to || *from == 0 => Err(ConfigError::invalid(
                format!("{from}-{to} is not a usable port range"),
            )),
            Self::Range { .. } => Ok(()),
        }
    }

    pub fn contains(&self, port: u16) -> bool {
        match self {
            Self::Single(p) => *p == port,
            Self::Range { from, to } => (*from..=*to).contains(&port),
        }
    }
}

/// Where agents connect.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConfig {
    #[serde(default)]
    pub kcp_bind: Option<String>,
    #[serde(default)]
    pub kcp: super::KcpConfig,
    #[serde(default = "default_cert_reload_interval", with = "humantime_serde")]
    pub cert_reload_interval: Duration,
    #[serde(default)]
    pub proxy_accept: super::ProxyAccept,
    #[serde(default)]
    pub mux: super::MuxConfig,
    /// Address for plain TCP and, by protocol detection, TLS.
    #[serde(default = "default_bridge_bind")]
    pub bind: String,

    /// Address for QUIC, which needs its own UDP socket.
    #[serde(default)]
    pub quic_bind: Option<String>,

    /// Path at which a WebSocket transport is served.
    #[serde(default = "default_ws_path")]
    pub ws_path: String,

    /// The bridge's TLS identity. Generated and self-signed if absent.
    #[serde(default)]
    pub tls: Option<TlsConfig>,

    /// How long without a keepalive before an agent is considered gone.
    #[serde(default = "default_disconnect_timeout", with = "humantime_serde")]
    pub disconnect_timeout: Duration,

    /// Whether to reject an agent whose clock is far from ours.
    ///
    /// The handshake nonce already prevents replay, so this is about catching
    /// misconfiguration rather than about security.
    #[serde(default)]
    pub strict_clock: bool,
}

fn default_bridge_bind() -> String {
    "0.0.0.0:14028".to_owned()
}

fn default_ws_path() -> String {
    "/ws".to_owned()
}

fn default_disconnect_timeout() -> Duration {
    Duration::from_secs(30)
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            kcp_bind: None,
            kcp: Default::default(),
            cert_reload_interval: default_cert_reload_interval(),
            proxy_accept: Default::default(),
            mux: Default::default(),
            bind: default_bridge_bind(),
            quic_bind: None,
            ws_path: default_ws_path(),
            tls: None,
            disconnect_timeout: default_disconnect_timeout(),
            strict_clock: false,
        }
    }
}

impl BridgeConfig {
    fn validate(&mut self) -> Result<(), ConfigError> {
        self.kcp.validate()?;
        if let Some(bind) = &self.kcp_bind {
            let address = super::parse_bind(bind)?;
            if self
                .quic_bind
                .as_ref()
                .is_some_and(|quic| super::parse_bind(quic).ok() == Some(address))
            {
                return Err(ConfigError::invalid(
                    "KCP and QUIC require different UDP listeners",
                ));
            }
        }
        if self.cert_reload_interval.is_zero() {
            return Err(ConfigError::invalid(
                "bridge cert_reload_interval must be positive",
            ));
        }
        self.proxy_accept.acl()?;
        self.mux.validate()?;
        super::parse_bind(&self.bind)?;
        if let Some(q) = &self.quic_bind {
            super::parse_bind(q)?;
        }
        if !self.ws_path.starts_with('/') {
            return Err(ConfigError::invalid(Message::with(
                "`ws_path` must start with '/': {path}",
                [Value::debug("path", &self.ws_path)],
            )));
        }
        if self.disconnect_timeout.is_zero() {
            return Err(ConfigError::invalid(
                "`disconnect_timeout` of zero drops every agent",
            ));
        }
        Ok(())
    }
}

/// A certificate and key on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// The HTTP and HTTPS reverse proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpProxyConfig {
    /// Aggregate limits per configured hostname pattern, shared across paths.
    #[serde(default, rename = "host_limit")]
    pub host_limits: HashMap<String, crate::traffic::ResourceLimits>,
    /// Optional HTTP/3 UDP listener, using the same certificate resolver.
    #[serde(default)]
    pub http3_bind: Option<String>,
    #[serde(default)]
    pub proxy_accept: super::ProxyAccept,
    #[serde(default = "default_cert_reload_interval", with = "humantime_serde")]
    pub cert_reload_interval: Duration,
    /// Address for plaintext HTTP. `None` disables it.
    #[serde(default = "default_http_bind")]
    pub bind: Option<String>,

    /// Address for HTTPS. `None` disables it.
    #[serde(default = "default_https_bind")]
    pub tls_bind: Option<String>,

    /// Certificate served for a host with nothing more specific.
    #[serde(default)]
    pub default_tls: Option<TlsConfig>,

    /// Per-host TLS identities, selected using SNI.
    #[serde(default)]
    pub certificates: HashMap<String, TlsConfig>,

    /// Automatic certificate issuance.
    #[serde(default)]
    pub acme: Option<AcmeConfig>,

    /// Whether to add `X-Forwarded-For` and friends.
    #[serde(default = "default_true")]
    pub forwarded_headers: bool,

    /// How long to wait for a backend's response headers.
    #[serde(default = "default_response_timeout", with = "humantime_serde")]
    pub response_timeout: Duration,
}

fn default_http_bind() -> Option<String> {
    Some("0.0.0.0:80".to_owned())
}

fn default_https_bind() -> Option<String> {
    Some("0.0.0.0:443".to_owned())
}

fn default_true() -> bool {
    true
}

fn default_response_timeout() -> Duration {
    Duration::from_secs(100)
}

fn default_cert_reload_interval() -> Duration {
    Duration::from_secs(30)
}

impl Default for HttpProxyConfig {
    fn default() -> Self {
        Self {
            host_limits: HashMap::new(),
            http3_bind: None,
            proxy_accept: Default::default(),
            cert_reload_interval: default_cert_reload_interval(),
            bind: default_http_bind(),
            tls_bind: default_https_bind(),
            default_tls: None,
            certificates: HashMap::new(),
            acme: None,
            forwarded_headers: true,
            response_timeout: default_response_timeout(),
        }
    }
}

impl HttpProxyConfig {
    fn validate(&mut self) -> Result<(), ConfigError> {
        let mut host_limits = HashMap::new();
        for (host, limits) in &self.host_limits {
            let host = super::normalize_host_pattern(host)?;
            if host_limits.insert(host, *limits).is_some() {
                return Err(ConfigError::invalid(
                    "duplicate host_limit pattern after normalization",
                ));
            }
        }
        self.host_limits = host_limits;
        if self.http3_bind.as_ref().is_some_and(|bind| bind.is_empty()) {
            self.http3_bind = None;
        }
        if let Some(bind) = &self.http3_bind {
            super::parse_bind(bind)?;
        }
        self.proxy_accept.acl()?;
        if self.bind.as_ref().is_some_and(|b| b.is_empty()) {
            self.bind = None;
        }
        if self.tls_bind.as_ref().is_some_and(|b| b.is_empty()) {
            self.tls_bind = None;
        }
        if self.response_timeout.is_zero() || self.cert_reload_interval.is_zero() {
            return Err(ConfigError::invalid(
                "response_timeout and cert_reload_interval must be positive",
            ));
        }
        let mut certificates = HashMap::new();
        for (host, cert) in std::mem::take(&mut self.certificates) {
            let host = super::normalize_host_pattern(&host)?;
            if certificates.insert(host, cert).is_some() {
                return Err(ConfigError::invalid("duplicate certificate host"));
            }
        }
        self.certificates = certificates;
        if self.bind.is_none() && self.tls_bind.is_none() && self.http3_bind.is_none() {
            return Err(ConfigError::invalid(
                "the http proxy would serve nothing; configure bind, tls_bind or http3_bind",
            ));
        }
        if let Some(b) = &self.bind {
            super::parse_bind(b)?;
        }
        if let Some(b) = &self.tls_bind {
            super::parse_bind(b)?;
        }
        if let Some(acme) = &mut self.acme {
            acme.validate()?;
            if self.tls_bind.is_none() {
                return Err(ConfigError::invalid(
                    "ACME issues certificates for HTTPS, but `tls_bind` is disabled",
                ));
            }
            if self.bind.is_none() && acme.challenge == AcmeChallenge::Http01 {
                // An HTTP-01 challenge is served over plaintext port 80. Without
                // it, issuance would fail every time with a confusing error.
                return Err(ConfigError::invalid(
                    "ACME needs plaintext `bind` for the HTTP-01 challenge",
                ));
            }
        }
        Ok(())
    }
}

/// Automatic certificate issuance over ACME.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcmeConfig {
    /// Obtain ZeroSSL EAB credentials using this account API key.
    #[serde(default)]
    pub zerossl_api_key: Option<Secret>,
    #[serde(default)]
    pub challenge: AcmeChallenge,
    #[serde(default)]
    pub eab: Option<ExternalAccountBinding>,
    /// Contact address the CA requires.
    pub email: String,

    /// Where to keep the account key and issued certificates.
    ///
    /// Persisting matters: a restart that forgets the account re-registers, and
    /// re-issuing on every restart runs into the CA's rate limits fast.
    #[serde(default = "default_acme_dir")]
    pub cache_dir: PathBuf,

    /// The ACME directory URL. Defaults to Let's Encrypt production.
    #[serde(default = "default_acme_directory")]
    pub directory: String,

    /// Use the CA's staging environment, whose certificates are untrusted but
    /// whose rate limits are generous. Worth having while setting things up.
    #[serde(default)]
    pub staging: bool,

    /// Hostnames to obtain certificates for. Empty means every registered exact host.
    #[serde(default)]
    pub domains: Vec<String>,

    /// How long before expiry to renew.
    #[serde(default = "default_renew_before", with = "humantime_serde")]
    pub renew_before: Duration,

    /// Accept the CA's terms of service. Issuance cannot proceed without this,
    /// and the operator, not this program, is the one who agrees.
    #[serde(default)]
    pub accept_tos: bool,
}

fn default_acme_dir() -> PathBuf {
    PathBuf::from("acme")
}

fn default_acme_directory() -> String {
    "https://acme-v02.api.letsencrypt.org/directory".to_owned()
}

fn default_renew_before() -> Duration {
    // The runtime caps this at one third of the certificate's actual lifetime.
    Duration::from_secs(30 * 24 * 60 * 60)
}

impl AcmeConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if let Some(key) = &self.zerossl_api_key
            && (key.is_empty()
                || key.len() > 4096
                || self.eab.is_some()
                || self.staging
                || self.directory != "https://acme.zerossl.com/v2/DV90")
        {
            return Err(ConfigError::invalid(
                "zerossl_api_key requires the ZeroSSL production directory, a nonempty key, staging=false and no manual eab",
            ));
        }
        if let Some(binding) = &self.eab {
            binding.key_bytes()?;
        }
        if !self.email.contains('@') {
            return Err(ConfigError::invalid(Message::with(
                "`email` does not look like an address: {email}",
                [Value::debug("email", &self.email)],
            )));
        }
        if !self.accept_tos {
            return Err(ConfigError::invalid(
                "ACME requires agreeing to the CA's terms of service; set `accept_tos = true`",
            ));
        }
        if !self.directory.starts_with("https://") {
            return Err(ConfigError::invalid(
                "an ACME directory URL must be https, or the account key is exposed",
            ));
        }
        for domain in &self.domains {
            if super::normalize_host_pattern(domain)? != *domain || domain.starts_with("*.") {
                return Err(ConfigError::invalid(
                    "ACME challenges require lowercase non-wildcard domain names",
                ));
            }
        }
        if self.renew_before.is_zero() {
            return Err(ConfigError::invalid(
                "`renew_before` of zero would only renew after expiry",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcmeChallenge {
    #[default]
    #[serde(rename = "http-01")]
    Http01,
    #[serde(rename = "tls-alpn-01")]
    TlsAlpn01,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAccountBinding {
    pub key_id: String,
    pub hmac_key: Secret,
}
impl ExternalAccountBinding {
    pub fn key_bytes(&self) -> Result<Vec<u8>, ConfigError> {
        use base64::{
            Engine,
            engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
        };
        if self.key_id.is_empty() || self.hmac_key.is_empty() || self.hmac_key.len() > 4096 {
            return Err(ConfigError::invalid(
                "EAB needs a key_id and a base64url HMAC key",
            ));
        }
        URL_SAFE_NO_PAD
            .decode(self.hmac_key.as_bytes())
            .or_else(|_| URL_SAFE.decode(self.hmac_key.as_bytes()))
            .map_err(|_| ConfigError::invalid("invalid base64url EAB HMAC key"))
    }
}

/// An agent permitted to connect.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserEntry {
    #[serde(default)]
    pub limits: crate::traffic::ResourceLimits,
    #[serde(default)]
    pub acl_mode: AclMode,
    #[serde(default)]
    pub acl_rules: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateConfig {
    pub path: PathBuf,
    #[serde(default = "default_state_interval", with = "humantime_serde")]
    pub save_interval: Duration,
}
fn default_state_interval() -> Duration {
    Duration::from_secs(5)
}

/// An agent permitted to connect.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientEntry {
    #[serde(default = "client_enabled")]
    pub enabled: bool,
    /// Server-defined owner; agents cannot change this association.
    #[serde(default)]
    pub user: Option<String>,
    pub vkey: Secret,

    #[serde(default)]
    pub remark: String,

    /// Traffic allowance and expiry.
    #[serde(default)]
    pub quota: Quota,

    /// Bandwidth ceiling in bytes per second. `None` is unlimited.
    #[serde(default)]
    pub rate_limit: Option<u64>,

    /// Cap on simultaneous connections across this client's tunnels.
    #[serde(default)]
    pub max_connections: Option<usize>,

    /// Cap on tunnels this client may register.
    #[serde(default)]
    pub max_tunnels: Option<usize>,

    #[serde(default)]
    pub acl_mode: AclMode,

    #[serde(default)]
    pub acl_rules: String,

    /// Whether this client may register tunnels by pushing config.
    ///
    /// Off by default: an agent that can define its own tunnels can claim any
    /// allowed port, which is a decision for the server's operator.
    #[serde(default)]
    pub allow_config_push: bool,
}

fn client_enabled() -> bool {
    true
}

/// Where and how much to log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    #[serde(default)]
    pub rotation: Option<LogRotation>,
    /// `trace`, `debug`, `info`, `warn` or `error`.
    #[serde(default = "default_log_level")]
    pub level: String,

    /// File to write to, in addition to stderr.
    #[serde(default)]
    pub file: Option<PathBuf>,

    /// Whether to emit JSON rather than human-readable lines.
    #[serde(default)]
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogRotation {
    #[serde(default = "log_bytes")]
    pub max_bytes: u64,
    #[serde(default = "log_files")]
    pub max_files: usize,
    #[serde(default, with = "humantime_serde::option")]
    pub interval: Option<Duration>,
}
fn log_bytes() -> u64 {
    10 * 1024 * 1024
}
fn log_files() -> usize {
    5
}
impl Default for LogRotation {
    fn default() -> Self {
        Self {
            max_bytes: log_bytes(),
            max_files: log_files(),
            interval: None,
        }
    }
}
impl LogConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !matches!(
            self.level.as_str(),
            "trace" | "debug" | "info" | "warn" | "error" | "off"
        ) {
            return Err(ConfigError::invalid(
                "log level must be trace, debug, info, warn, error or off",
            ));
        }
        if let Some(rotation) = &self.rotation
            && (self.file.as_ref().is_none_or(|p| p.as_os_str().is_empty())
                || rotation.max_bytes < 65536
                || !(1..=128).contains(&rotation.max_files)
                || rotation
                    .interval
                    .is_some_and(|d| d < Duration::from_secs(1)))
        {
            return Err(ConfigError::invalid(
                "log rotation needs a file, max_bytes >= 65536, max_files 1..128 and interval >= 1s",
            ));
        }
        Ok(())
    }
}

fn default_log_level() -> String {
    "info".to_owned()
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            rotation: None,
            level: default_log_level(),
            file: None,
            json: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_rotation_requires_a_file_and_valid_bounds() {
        let mut config = LogConfig {
            rotation: Some(LogRotation::default()),
            ..Default::default()
        };
        assert!(config.validate().is_err());
        config.file = Some("server.log".into());
        config.validate().unwrap();
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<LogConfig>(&encoded).unwrap(), config);
        config.rotation.as_mut().unwrap().max_files = 0;
        assert!(config.validate().is_err());
        config.rotation.as_mut().unwrap().max_files = 5;
        config.rotation.as_mut().unwrap().interval = Some(Duration::ZERO);
        assert!(config.validate().is_err());
        config.rotation = None;
        config.level = "misspelled".into();
        assert!(config.validate().is_err());
    }

    /// The smallest server config that should load.
    const MINIMAL: &str = r#"
[client.agent-1]
vkey = "a-shared-secret"
"#;

    fn parse(text: &str) -> Result<ServerConfig, ConfigError> {
        let mut c: ServerConfig =
            toml::from_str(text).map_err(|e| ConfigError::invalid(e.to_string()))?;
        c.validate()?;
        Ok(c)
    }

    #[test]
    fn a_minimal_config_loads_with_usable_defaults() {
        let c = parse(MINIMAL).expect("should load");
        assert_eq!(c.bridge.bind, "0.0.0.0:14028");
        assert_eq!(c.bridge.ws_path, "/ws");
        assert!(c.http.is_none(), "the reverse proxy is opt-in");
        assert_eq!(c.clients.len(), 1);
        assert!(
            !c.clients["agent-1"].allow_config_push,
            "config push must be opt-in"
        );
    }

    #[test]
    fn a_vkey_never_appears_in_debug_output() {
        let c = parse(MINIMAL).unwrap();
        let shown = format!("{c:?}");
        assert!(
            !shown.contains("a-shared-secret"),
            "leaked a client vkey: {shown}"
        );
    }

    #[test]
    fn a_server_with_no_clients_is_refused() {
        // It would accept nobody; better to say so than to leave an operator
        // watching handshakes fail.
        let err = parse("[bridge]\nbind = \"0.0.0.0:14028\"\n").unwrap_err();
        assert!(err.to_string().contains("client"), "{err}");
    }

    #[test]
    fn a_client_with_an_empty_vkey_is_refused() {
        let err = parse("[client.agent-1]\nvkey = \"\"\n").unwrap_err();
        assert!(
            err.to_string().contains("agent-1"),
            "the error should name the client: {err}"
        );
    }

    #[test]
    fn a_bad_client_acl_fails_at_load() {
        let err = parse(
            "[client.agent-1]\nvkey = \"k\"\nacl_mode = \"allow\"\nacl_rules = \"nonsense\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("agent-1"), "{err}");
    }

    #[test]
    fn a_misspelled_key_is_an_error_rather_than_a_silent_default() {
        let err = toml::from_str::<ServerConfig>(&format!("{MINIMAL}\n[bridge]\nbnid = \"x\"\n"))
            .expect_err("a typo must not look like it worked");
        assert!(err.to_string().contains("bnid"), "{err}");
    }

    #[test]
    fn a_bare_port_is_accepted_as_a_bind() {
        let c = parse(&format!("{MINIMAL}\n[bridge]\nbind = \"9000\"\n")).expect("should load");
        assert_eq!(c.bridge.bind, "9000");
    }

    #[test]
    fn a_meaningless_bridge_bind_is_refused() {
        assert!(parse(&format!("{MINIMAL}\n[bridge]\nbind = \"not-an-address\"\n")).is_err());
    }

    #[test]
    fn a_relative_ws_path_is_refused() {
        let err = parse(&format!("{MINIMAL}\n[bridge]\nws_path = \"ws\"\n")).unwrap_err();
        assert!(err.to_string().contains("ws_path"), "{err}");
    }

    #[test]
    fn a_zero_disconnect_timeout_is_refused() {
        // It would consider every agent gone the moment it connected.
        let err = parse(&format!(
            "{MINIMAL}\n[bridge]\ndisconnect_timeout = \"0s\"\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("disconnect_timeout"), "{err}");
    }

    #[test]
    fn the_default_port_policy_excludes_privileged_ports() {
        // Binding below 1024 needs privileges the server should not hold.
        let c = parse(MINIMAL).unwrap();
        assert!(!c.port_allowed(0));
        assert!(!c.port_allowed(80));
        assert!(!c.port_allowed(1024));
        assert!(c.port_allowed(1025));
        assert!(c.port_allowed(8080));
    }

    #[test]
    fn an_explicit_port_list_replaces_the_default() {
        // `allowed_ports` is top-level, so it has to precede the client table:
        // TOML assigns a bare key to whichever table was opened last.
        let c = parse(&format!(
            "allowed_ports = [8080, {{ from = 9000, to = 9100 }}]\n{MINIMAL}"
        ))
        .expect("should load");

        assert!(c.port_allowed(8080));
        assert!(c.port_allowed(9000));
        assert!(c.port_allowed(9050));
        assert!(c.port_allowed(9100));
        assert!(!c.port_allowed(9101));
        assert!(!c.port_allowed(1025), "an explicit list is exhaustive");
    }

    #[test]
    fn an_inverted_port_range_is_refused() {
        let err = parse(&format!(
            "allowed_ports = [{{ from = 100, to = 10 }}]\n{MINIMAL}"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("range"), "{err}");
    }

    #[test]
    fn port_zero_in_a_range_is_refused() {
        for spec in ["[0]", "[{ from = 0, to = 10 }]"] {
            let err = parse(&format!("allowed_ports = {spec}\n{MINIMAL}")).unwrap_err();
            assert!(
                err.to_string().contains("port"),
                "should fail on the port, not on parsing: {err}"
            );
        }
    }

    #[test]
    fn an_http_proxy_serving_nothing_is_refused() {
        // Both listeners disabled means the proxy would accept no traffic at all.
        let mut c: ServerConfig = toml::from_str(&format!("{MINIMAL}\n[http]\n")).unwrap();
        let http = c.http.as_mut().unwrap();
        http.bind = None;
        http.tls_bind = None;

        let err = c.validate().unwrap_err();
        assert!(err.to_string().contains("serve nothing"), "{err}");
    }

    #[test]
    fn an_http_proxy_loads_with_both_listeners_by_default() {
        let c = parse(&format!("{MINIMAL}\n[http]\n")).expect("should load");
        let http = c.http.unwrap();
        assert_eq!(http.bind.as_deref(), Some("0.0.0.0:80"));
        assert_eq!(http.tls_bind.as_deref(), Some("0.0.0.0:443"));
        assert!(
            http.forwarded_headers,
            "real client IPs should reach backends by default"
        );
    }

    #[test]
    fn acme_loads_when_fully_specified() {
        let c = parse(&format!(
            r#"{MINIMAL}
[http]

[http.acme]
email = "ops@example.com"
accept_tos = true
domains = ["example.com"]
"#
        ))
        .expect("should load");

        let acme = c.http.unwrap().acme.unwrap();
        assert_eq!(acme.email, "ops@example.com");
        assert!(acme.directory.starts_with("https://"));
        assert!(!acme.renew_before.is_zero());
        assert_eq!(acme.domains, vec!["example.com"]);
    }

    #[test]
    fn acme_requires_agreeing_to_the_terms() {
        // The operator agrees, not this program.
        let err = parse(&format!(
            "{MINIMAL}\n[http]\n\n[http.acme]\nemail = \"ops@example.com\"\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("accept_tos"), "{err}");
    }

    #[test]
    fn acme_requires_a_contact_address() {
        let err = parse(&format!(
            "{MINIMAL}\n[http]\n\n[http.acme]\nemail = \"not-an-address\"\naccept_tos = true\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("email"), "{err}");
    }

    #[test]
    fn an_insecure_acme_directory_is_refused() {
        // Plaintext ACME would expose the account key.
        let err = parse(&format!(
            r#"{MINIMAL}
[http]

[http.acme]
email = "ops@example.com"
accept_tos = true
directory = "http://acme.example.com/directory"
"#
        ))
        .unwrap_err();
        assert!(err.to_string().contains("https"), "{err}");
    }

    #[test]
    fn acme_without_an_https_listener_is_refused() {
        // It would issue certificates nothing serves.
        let mut c: ServerConfig = toml::from_str(&format!(
            r#"{MINIMAL}
[http]

[http.acme]
email = "ops@example.com"
accept_tos = true
"#
        ))
        .unwrap();
        c.http.as_mut().unwrap().tls_bind = None;

        let err = c.validate().unwrap_err();
        assert!(err.to_string().contains("tls_bind"), "{err}");
    }

    #[test]
    fn acme_needs_plaintext_port_80_for_its_challenge() {
        // An HTTP-01 challenge is served over plaintext; without it issuance
        // would fail every time with a confusing error.
        let mut c: ServerConfig = toml::from_str(&format!(
            r#"{MINIMAL}
[http]

[http.acme]
email = "ops@example.com"
accept_tos = true
"#
        ))
        .unwrap();
        c.http.as_mut().unwrap().bind = None;

        let err = c.validate().unwrap_err();
        assert!(err.to_string().contains("HTTP-01"), "{err}");
    }

    #[test]
    fn zerossl_api_keys_are_restricted_to_the_correct_ca_and_redacted() {
        let text = format!(
            r#"{MINIMAL}
[http]
[http.acme]
email = "ops@example.com"
accept_tos = true
directory = "https://acme.zerossl.com/v2/DV90"
zerossl_api_key = "private-account-key"
"#
        );
        let config = parse(&text).unwrap();
        assert!(!format!("{config:?}").contains("private-account-key"));
        for text in [
            text.replace(
                "https://acme.zerossl.com/v2/DV90",
                "https://other.test/directory",
            ),
            text.replace("private-account-key", ""),
            format!("{text}\nstaging = true\n"),
            format!("{text}\n[http.acme.eab]\nkey_id='a'\nhmac_key='aGVsbG8'\n"),
        ] {
            assert!(
                parse(&text)
                    .unwrap_err()
                    .to_string()
                    .contains("zerossl_api_key")
            );
        }
    }

    #[test]
    fn tls_alpn_supports_https_only_and_validates_external_account_credentials() {
        let text = format!(
            r#"{MINIMAL}
[http]
bind = ""
tls_bind = "127.0.0.1:8443"
[http.acme]
email = "ops@example.com"
accept_tos = true
challenge = "tls-alpn-01"
[http.acme.eab]
key_id = "ca-account"
hmac_key = "aGVsbG8"
"#
        );
        let config = parse(&text).unwrap();
        let http = config.http.unwrap();
        assert!(http.bind.is_none());
        let acme = http.acme.unwrap();
        assert_eq!(acme.challenge, AcmeChallenge::TlsAlpn01);
        assert_eq!(acme.eab.unwrap().key_bytes().unwrap(), b"hello");
        assert!(
            parse(&text.replace("aGVsbG8", "!invalid"))
                .unwrap_err()
                .to_string()
                .contains("base64url")
        );
        assert!(
            parse(&text.replace("tls-alpn-01", "http-01"))
                .unwrap_err()
                .to_string()
                .contains("HTTP-01")
        );
    }

    #[test]
    fn a_client_quota_and_limits_load() {
        let c = parse(
            r#"
[client.agent-1]
vkey = "k"
rate_limit = 1048576
max_connections = 100
max_tunnels = 10
allow_config_push = true

[client.agent-1.quota]
flow_limit = 10737418240
expires_at = 1800000000
"#,
        )
        .expect("should load");

        let entry = &c.clients["agent-1"];
        assert_eq!(entry.rate_limit, Some(1024 * 1024));
        assert_eq!(entry.max_connections, Some(100));
        assert_eq!(entry.max_tunnels, Some(10));
        assert!(entry.allow_config_push);
        assert_eq!(entry.quota.flow_limit, Some(10 * 1024 * 1024 * 1024));
        assert_eq!(entry.quota.expires_at, Some(1_800_000_000));
    }

    #[test]
    fn several_clients_load_independently() {
        let c = parse(
            r#"
[client.agent-1]
vkey = "k1"

[client.agent-2]
vkey = "k2"
remark = "the other one"
"#,
        )
        .expect("should load");

        assert_eq!(c.clients.len(), 2);
        assert_eq!(c.clients["agent-2"].remark, "the other one");
        assert_ne!(c.clients["agent-1"].vkey, c.clients["agent-2"].vkey);
    }

    #[test]
    fn logging_defaults_are_sane() {
        let c = parse(MINIMAL).unwrap();
        assert_eq!(c.log.level, "info");
        assert!(c.log.file.is_none());
        assert!(!c.log.json, "human-readable output by default");
    }

    #[test]
    fn a_quic_bind_is_validated_when_present() {
        assert!(parse(&format!("{MINIMAL}\n[bridge]\nquic_bind = \"8025\"\n")).is_ok());
        assert!(parse(&format!("{MINIMAL}\n[bridge]\nquic_bind = \"nonsense\"\n")).is_err());
    }
}
