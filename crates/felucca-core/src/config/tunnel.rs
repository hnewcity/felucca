//! Tunnel definitions and normalization shared by agents, local services and managed configuration.
use super::{Backend, HealthCheck, HealthCheckKind, LoadBalance};
use crate::AclMode;
use crate::ConfigError;
use felucca_i18n::{Message, Value};
use serde::{Deserialize, Serialize};
pub(super) fn normalize_tunnels(
    source: &mut Vec<TunnelConfig>,
    mappings: &mut Vec<super::PortMapping>,
    geo: &crate::geodata::GeoResolver,
    allow_virtual: bool,
) -> Result<(), ConfigError> {
    normalize_with_files(source, mappings, geo, allow_virtual, false)
}

pub(super) fn normalize_with_files(
    source: &mut Vec<TunnelConfig>,
    mappings: &mut Vec<super::PortMapping>,
    geo: &crate::geodata::GeoResolver,
    allow_virtual: bool,
    remote_files: bool,
) -> Result<(), ConfigError> {
    let mut tunnels = source.clone();
    for mapping in mappings.iter() {
        tunnels.extend(mapping.expand_with_geo(geo)?);
        if tunnels.len() > 4096 {
            return Err(ConfigError::invalid(
                "at most 4096 expanded tunnels are supported",
            ));
        }
    }
    for t in &mut tunnels {
        t.validate_kind(geo, allow_virtual, remote_files)?;
    }
    if tunnels.len() > 4096 {
        return Err(ConfigError::invalid("at most 4096 tunnels are supported"));
    }
    let mut ids = std::collections::HashSet::new();
    for tunnel in &tunnels {
        if let Some(id) = tunnel.id
            && (id == 0 || !ids.insert(id))
        {
            return Err(ConfigError::invalid(
                "tunnel ids must be nonzero and unique",
            ));
        }
    }
    let mut next = 1u32;
    for tunnel in &mut tunnels {
        if tunnel.id.is_none() {
            while ids.contains(&next) {
                next += 1;
            }
            tunnel.id = Some(next);
            ids.insert(next);
        }
    }

    // Two tunnels on one port would race for the listener, and which one won
    // would depend on start order.
    let mut ports: Vec<u16> = tunnels
        .iter()
        .filter(|t| t.enabled && t.public && !t.exposes_http())
        .map(|t| t.port)
        .collect();
    ports.sort_unstable();
    if let Some(dup) = ports.windows(2).find(|w| w[0] == w[1]) {
        return Err(ConfigError::invalid(Message::with(
            "two tunnels both ask for port {port}",
            [Value::new("port", dup[0])],
        )));
    }
    *source = tunnels;
    mappings.clear();
    Ok(())
}

/// What a tunnel does with traffic on its public port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelMode {
    /// HTTP/WebDAV served directly from an agent-owned directory.
    File,
    /// Forward raw TCP to a backend.
    Tcp,
    /// Forward UDP datagrams to a backend.
    Udp,
    /// Reverse-proxy HTTP, routed by host.
    Http,
    /// Serve SOCKS5, target chosen per request.
    Socks5,
    /// Serve an HTTP forward proxy.
    HttpProxy,
    /// Serve SOCKS5 and HTTP proxy on one port, detected per connection.
    MixedProxy,
    /// Recover the original destination from the server kernel.
    #[serde(alias = "tcptrans")]
    Transparent,
}

impl TunnelMode {
    /// Whether this mode forwards to configured backends, as opposed to choosing
    /// a target from each request.
    pub fn needs_backends(self) -> bool {
        matches!(self, Self::Tcp | Self::Udp | Self::Http)
    }
}

/// One tunnel an agent asks the server to expose.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelConfig {
    /// Private services are registered but do not bind a public listener.
    #[serde(default = "enabled")]
    pub public: bool,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Stable identity for runtime state; set explicitly when reordering config.
    #[serde(default)]
    pub id: Option<u32>,
    #[serde(default)]
    pub file: Option<super::FileConfig>,
    pub mode: TunnelMode,

    /// Public port on the server.
    #[serde(default)]
    pub port: u16,

    /// Address to bind on the server. `None` means every interface.
    #[serde(default)]
    pub bind: Option<String>,

    /// Where the agent sends traffic. Several entries are load balanced.
    #[serde(default, rename = "backend")]
    pub backends: Vec<Backend>,

    /// How to spread traffic across backends.
    #[serde(default)]
    pub balance: LoadBalance,

    /// Active health checking. Without it a dead backend keeps taking its share.
    #[serde(default)]
    pub health: Option<HealthCheck>,

    /// Hostnames this tunnel serves, for `http` mode.
    #[serde(default)]
    pub hosts: Vec<String>,

    /// Who may connect, by source address.
    #[serde(default)]
    pub acl_mode: AclMode,

    /// ACL rules, comma- or newline-separated.
    #[serde(default)]
    pub acl_rules: String,

    /// A note for the operator; carried through to the dashboard.
    #[serde(default)]
    pub remark: String,

    #[serde(default)]
    pub options: super::RouteOptions,

    /// Checked against the original name AND each resolved address.
    #[serde(default)]
    pub target_policy: crate::access::TargetPolicy,
}

fn enabled() -> bool {
    true
}

impl TunnelConfig {
    pub fn exposes_http(&self) -> bool {
        self.mode == TunnelMode::Http || (self.mode == TunnelMode::File && !self.hosts.is_empty())
    }
    pub(super) fn validate(
        &mut self,
        geo: &crate::geodata::GeoResolver,
    ) -> Result<(), ConfigError> {
        self.validate_kind(geo, false, false)
    }
    fn validate_kind(
        &mut self,
        geo: &crate::geodata::GeoResolver,
        allow_virtual: bool,
        remote_files: bool,
    ) -> Result<(), ConfigError> {
        match (&self.file, self.mode) {
            (Some(file), TunnelMode::File) => {
                if remote_files {
                    file.validate_options()?;
                    super::managed::file_capability(&file.root)?;
                } else {
                    file.validate()?;
                }
                if self.target_policy != crate::access::TargetPolicy::default()
                    || self.options.proxy_protocol != super::ProxyVersion::Off
                    || self.options.http.tls_mode != super::route::TlsMode::Http
                    || self.options.auth.required()
                {
                    return Err(ConfigError::invalid(
                        "file tunnels use file.auth, cannot select network targets, emit PROXY headers or carry raw TLS",
                    ));
                }
            }
            (None, TunnelMode::File) => {
                return Err(ConfigError::invalid("file mode requires [tunnel.file]"));
            }
            (Some(_), _) => return Err(ConfigError::invalid("tunnel.file requires file mode")),
            _ => {}
        }
        self.options.validate()?;
        if self.mode == TunnelMode::Udp
            && (self.options.proxy_protocol == super::ProxyVersion::V1
                || self.options.proxy_accept.mode != super::ProxyMode::Off)
        {
            return Err(ConfigError::invalid(
                "UDP supports sending PROXY v2; receiving stream headers and v1 are not supported",
            ));
        }
        if self.exposes_http() && self.options.proxy_accept.mode != super::ProxyMode::Off {
            return Err(ConfigError::invalid(
                "HTTP receives PROXY headers through server http.proxy_accept",
            ));
        }
        if self.options.http.tls_mode != super::route::TlsMode::Http && self.options.auth.required()
        {
            return Err(ConfigError::invalid(
                "raw TLS routes cannot perform HTTP authentication",
            ));
        }
        if self.options.http.tls_mode == super::route::TlsMode::Passthrough
            && self.backends.iter().any(|b| b.tls)
        {
            return Err(ConfigError::invalid(
                "TLS passthrough backends already receive TLS records; backend.tls must be false",
            ));
        }
        crate::access::TargetAcl::compile_with_geo(&self.target_policy, geo)?;
        if self.options.auth.required()
            && !matches!(
                self.mode,
                TunnelMode::Http
                    | TunnelMode::HttpProxy
                    | TunnelMode::Socks5
                    | TunnelMode::MixedProxy
            )
        {
            return Err(ConfigError::invalid(
                "authentication needs an HTTP or SOCKS5 tunnel",
            ));
        }
        if !self.exposes_http() && self.options.http != super::HttpRouteOptions::default() {
            return Err(ConfigError::invalid(
                "HTTP route options need an http tunnel",
            ));
        }
        if !self.public
            && (self.port != 0
                || self.bind.is_some()
                || self.exposes_http()
                || !self.hosts.is_empty()
                || self.options.proxy_accept.mode != super::ProxyMode::Off)
        {
            return Err(ConfigError::invalid(
                "private tunnels require port 0, no bind or HTTP host routing, and no ingress PROXY parsing; use private TCP for an HTTP backend",
            ));
        }
        if self.public && self.port == 0 {
            return Err(ConfigError::invalid("port 0 would bind an arbitrary port"));
        }

        if self.options.transparent_tproxy && self.mode != TunnelMode::Transparent {
            return Err(ConfigError::invalid(
                "transparent_tproxy only applies to transparent TCP",
            ));
        }
        if self.mode == TunnelMode::Transparent
            && (!self.public
                || self.options.auth.required()
                || self.options.proxy_accept.mode != super::ProxyMode::Off)
        {
            return Err(ConfigError::invalid(
                "transparent TCP requires a public listener, no proxy accounts and no ingress PROXY parsing",
            ));
        }
        if self.mode.needs_backends() {
            if self.backends.is_empty() {
                return Err(ConfigError::invalid(Message::with(
                    "tunnel on port {port} is {mode} and needs at least one backend",
                    [
                        Value::new("port", self.port),
                        Value::debug("mode", self.mode),
                    ],
                )));
            }
            for b in &self.backends {
                if super::local::VirtualTarget::parse(&b.addr)?.is_some() {
                    if !allow_virtual
                        || b.tls
                        || self.target_policy != crate::access::TargetPolicy::default()
                        || self
                            .health
                            .as_ref()
                            .is_some_and(|h| h.kind != HealthCheckKind::None)
                    {
                        return Err(ConfigError::invalid(
                            "virtual targets require server local routes, use destination ACLs and cannot have backend TLS or network health probes",
                        ));
                    }
                } else {
                    super::validate_endpoint(&b.addr)?;
                }
                b.trust.validate()?;
                if !b.addr.contains(':') {
                    return Err(ConfigError::invalid(Message::with(
                        "backend {addr} needs a port",
                        [Value::debug("addr", &b.addr)],
                    )));
                }
                if b.weight == 0 {
                    // A zero weight never gets picked, which reads as a typo
                    // rather than as "disable this backend".
                    return Err(ConfigError::invalid(Message::with(
                        "backend {addr} has weight 0 and would never be used",
                        [Value::debug("addr", &b.addr)],
                    )));
                }
            }
        } else if !self.backends.is_empty() {
            return Err(ConfigError::invalid(Message::with(
                "tunnel on port {port} is {mode}, which picks its own target, so backends \
                 would be ignored",
                [
                    Value::new("port", self.port),
                    Value::debug("mode", self.mode),
                ],
            )));
        }

        if self.mode == TunnelMode::Http && self.hosts.is_empty() {
            return Err(ConfigError::invalid(Message::with(
                "http tunnel on port {port} needs at least one host",
                [Value::new("port", self.port)],
            )));
        }
        if !self.exposes_http() && !self.hosts.is_empty() {
            return Err(ConfigError::invalid(Message::with(
                "`hosts` only applies to an http tunnel, but port {port} is {mode}",
                [
                    Value::new("port", self.port),
                    Value::debug("mode", self.mode),
                ],
            )));
        }

        for host in &mut self.hosts {
            *host = super::normalize_host_pattern(host)?;
        }
        if self.mode == TunnelMode::Udp && self.backends.iter().any(|b| b.tls) {
            return Err(ConfigError::invalid("UDP backends cannot use TLS"));
        }

        // UDP is connectionless, so an inline TCP or HTTP probe says nothing
        // about it. Failing here beats silently never marking a backend down.
        if self.mode == TunnelMode::Udp
            && self
                .health
                .as_ref()
                .is_some_and(|h| h.kind != HealthCheckKind::None)
        {
            return Err(ConfigError::invalid(Message::with(
                "port {port}: udp tunnels cannot be health checked",
                [Value::new("port", self.port)],
            )));
        }

        // Checked here so an unusable bind fails at startup rather than being
        // refused by the server minutes later.
        if self.public {
            super::parse_bind_host(self.bind.as_deref(), self.port)?;
        }

        // Parsed here so a bad rule fails at startup rather than on the first
        // connection that would have been filtered.
        crate::acl::Acl::parse_with_geo(self.acl_mode, &self.acl_rules, geo)
            .map_err(|e| ConfigError::invalid(e.to_string()))?;

        if let Some(h) = &mut self.health {
            if !self.mode.needs_backends() && h.kind != HealthCheckKind::None {
                return Err(ConfigError::invalid(
                    "dynamic proxy targets cannot have backend health checks",
                ));
            }
            h.validate()?;
        }
        Ok(())
    }
}
