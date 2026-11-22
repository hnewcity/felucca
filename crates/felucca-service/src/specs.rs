use felucca_core::config::TunnelMode;
use felucca_proto::TunnelSpec;
/// The tunnel definitions to register with the server.
pub fn specs(tunnels: &[felucca_core::config::TunnelConfig]) -> Vec<TunnelSpec> {
    tunnels
        .iter()
        .enumerate()
        .filter(|(_, t)| t.enabled)
        .map(|(index, t)| TunnelSpec {
            public: t.public,
            file: t.mode == TunnelMode::File,
            tunnel_id: t.id.unwrap_or(index as u32 + 1),
            mode: match t.mode {
                TunnelMode::File if t.exposes_http() => felucca_proto::TunnelMode::Http,
                TunnelMode::File => felucca_proto::TunnelMode::Tcp,
                TunnelMode::Tcp => felucca_proto::TunnelMode::Tcp,
                TunnelMode::Udp => felucca_proto::TunnelMode::Udp,
                // The reverse proxy routes by host on the server; from the
                // tunnel's point of view it is a stream of TCP connections.
                TunnelMode::Http => felucca_proto::TunnelMode::Http,
                TunnelMode::Socks5 => felucca_proto::TunnelMode::Socks5,
                TunnelMode::HttpProxy => felucca_proto::TunnelMode::HttpProxy,
                TunnelMode::Transparent => felucca_proto::TunnelMode::Transparent,
                TunnelMode::MixedProxy => felucca_proto::TunnelMode::MixedProxy,
            },
            port: t.port,
            bind: t.bind.clone(),
            target: None,
            remark: t.remark.clone(),
            hosts: t.hosts.clone(),
            acl_mode: t.acl_mode,
            acl_rules: t.acl_rules.clone(),
            options: t.options.clone(),
        })
        .collect()
}
