//! Prometheus text exposition with the same tenant boundary as the dashboard.
use super::{auth::Principal, resources::visible};
use crate::{Server, resources::ResourceKey};
use felucca_core::ServerConfig;
use std::fmt::Write;

fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}
pub(super) async fn render(
    server: &Server,
    config: &ServerConfig,
    principal: &Principal,
    revision: u64,
    healthy: bool,
) -> String {
    let mut output = String::from(
        "# HELP felucca_up Whether the forwarding daemon is healthy.\n# TYPE felucca_up gauge\n\
         # HELP felucca_config_revision Persisted configuration revision.\n# TYPE felucca_config_revision gauge\n\
         # HELP felucca_client_online Whether a configured node has a live bridge.\n# TYPE felucca_client_online gauge\n\
         # HELP felucca_client_enabled Whether a configured node is enabled.\n# TYPE felucca_client_enabled gauge\n\
         # HELP felucca_connections Active node admission permits.\n# TYPE felucca_connections gauge\n\
         # HELP felucca_traffic_bytes_total Accounted plaintext bytes by node and direction.\n# TYPE felucca_traffic_bytes_total counter\n\
         # HELP felucca_route_ready Whether a tunnel is registered.\n# TYPE felucca_route_ready gauge\n\
         # HELP felucca_tunnel_bytes_total Accounted plaintext bytes by tunnel and direction.\n# TYPE felucca_tunnel_bytes_total counter\n",
    );
    writeln!(output, "felucca_up {}", u8::from(healthy)).unwrap();
    writeln!(output, "felucca_config_revision {revision}").unwrap();
    let nodes: Vec<_> = config
        .clients
        .iter()
        .filter(|(_, node)| visible(principal, node.user.as_deref()))
        .map(|(key, node)| (key.clone(), node.user.as_deref(), node.enabled))
        .chain(
            config
                .locals
                .iter()
                .filter(|(_, node)| visible(principal, node.user.as_deref()))
                .map(|(key, node)| (format!("local.{key}"), node.user.as_deref(), true)),
        )
        .collect();
    let routes = server.tunnel_specs().await;
    for (key, user, enabled) in nodes {
        let labels = format!(
            "client=\"{}\",user=\"{}\"",
            label(&key),
            label(user.unwrap_or(""))
        );
        let node = server.registry().get(&key);
        writeln!(
            output,
            "felucca_client_online{{{labels}}} {}",
            u8::from(node.is_some())
        )
        .unwrap();
        writeln!(
            output,
            "felucca_client_enabled{{{labels}}} {}",
            u8::from(enabled)
        )
        .unwrap();
        writeln!(
            output,
            "felucca_connections{{{labels}}} {}",
            node.as_ref().map_or(0, |node| node.live_connections())
        )
        .unwrap();
        let stats = server
            .registry()
            .store
            .stats(&ResourceKey::Client { name: key.clone() })
            .unwrap_or_default();
        for (direction, bytes) in [("inlet", stats.inlet), ("export", stats.export)] {
            writeln!(
                output,
                "felucca_traffic_bytes_total{{{labels},direction=\"{direction}\"}} {bytes}"
            )
            .unwrap();
        }
        for (_, route) in routes.iter().filter(|(client, _)| client == &key) {
            let labels = format!("{labels},tunnel=\"{}\"", route.tunnel_id);
            writeln!(output, "felucca_route_ready{{{labels}}} 1").unwrap();
            let stats = server
                .registry()
                .store
                .stats(&ResourceKey::Tunnel {
                    client: key.clone(),
                    id: route.tunnel_id,
                })
                .unwrap_or_default();
            for (direction, bytes) in [("inlet", stats.inlet), ("export", stats.export)] {
                writeln!(
                    output,
                    "felucca_tunnel_bytes_total{{{labels},direction=\"{direction}\"}} {bytes}"
                )
                .unwrap();
            }
        }
    }
    output
}
