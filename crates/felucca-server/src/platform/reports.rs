//! Imported-traffic notifications are ephemeral and never fill the durable outbox.
use super::{Node, store};
use crate::resources::ResourceKey;
use felucca_core::{ServerConfig, traffic::TrafficStats};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(super) struct Report {
    pub resource: &'static str,
    pub action: &'static str,
    pub kind: &'static str,
    pub client: String,
    pub user: Option<String>,
    pub traffic: TrafficStats,
    pub timestamp_ms: u64,
}
impl Node {
    pub(super) fn report_traffic(
        &self,
        config: &ServerConfig,
        clients: impl IntoIterator<Item = String>,
    ) {
        let Some(settings) = config.node.as_ref() else {
            return;
        };
        if settings.traffic_report_interval.is_none() && settings.traffic_report_step_bytes == 0 {
            return;
        }
        let Some(daemon) = self.daemon.upgrade() else {
            return;
        };
        let server = daemon.server();
        let mut progress = self.report_progress.lock();
        progress.retain(|client, _| config.clients.contains_key(client));
        for id in clients {
            let Some(client) = config
                .clients
                .get(&id)
                .filter(|c| c.quota.flow_limit.is_some())
            else {
                continue;
            };
            let Some(traffic) = server
                .registry()
                .store
                .stats(&ResourceKey::Client { name: id.clone() })
            else {
                continue;
            };
            let total = traffic.total();
            let due = progress.get(&id).is_none_or(|(last, previous)| {
                settings
                    .traffic_report_interval
                    .is_some_and(|d| last.elapsed() >= d)
                    || settings.traffic_report_step_bytes > 0
                        && total.saturating_sub(*previous) >= settings.traffic_report_step_bytes
            });
            if due {
                progress.insert(id.clone(), (tokio::time::Instant::now(), total));
                let _ = self.reports.send(Report {
                    resource: "client",
                    action: "reported",
                    kind: "client.traffic.reported",
                    client: id,
                    user: client.user.clone(),
                    traffic,
                    timestamp_ms: store::now(),
                });
            }
        }
    }
}
