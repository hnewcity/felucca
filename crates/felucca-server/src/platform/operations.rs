use super::Node;
use crate::admin::{Error, Result, bad, forbidden, json, missing};
use crate::control::Principal;
use crate::control::resources;
use crate::{http_support::Body, resources::ResourceKey};
use felucca_core::{
    config::AdminRole,
    traffic::{ResourceLimits, TrafficStats},
};
use felucca_i18n::Message;
use http::{Request, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrafficItem {
    client: String,
    inlet_bytes: u64,
    export_bytes: u64,
}
impl Node {
    pub(super) async fn configuration_action(
        self: Arc<Self>,
        request: Request<Body>,
        principal: &Principal,
    ) -> Result<http::Response<Body>> {
        let path = request.uri().path().to_owned();
        if principal.role == AdminRole::Viewer {
            return Err(forbidden());
        }
        if path.ends_with("/sync") {
            resources::admin(principal)?;
        }
        let revision = crate::admin::api::expected_revision(request.headers())?;
        let key = crate::admin::api::idempotency_key(request.headers())?;
        let input: Value = crate::admin::api::body(request).await?;
        let digest = crate::control::credentials::digest(
            &serde_json::to_vec(&json!({"path":path,"body":input,"revision":revision}))
                .expect("operation JSON"),
        );
        let operation = crate::control::Operation {
            revision,
            key,
            digest,
        };
        if let Some(applied) = self.management.replay(principal, &operation)? {
            return Ok(json(StatusCode::OK, applied));
        }
        let command = if path.ends_with("/kick") {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Input {
                ids: Vec<String>,
            }
            let input: Input =
                serde_json::from_value(input).map_err(|_| bad("kick requires client ids"))?;
            crate::control::Command::Kick(input.ids)
        } else {
            if input.as_object().is_none_or(|map| !map.is_empty()) {
                return Err(bad("sync body must be an empty object"));
            }
            crate::control::Command::Sync
        };
        let applied = self.management.apply(principal, operation, command).await?;
        Ok(json(StatusCode::OK, applied))
    }
    pub(super) async fn traffic(
        self: Arc<Self>,
        request: Request<Body>,
        principal: &Principal,
    ) -> Result<http::Response<Body>> {
        if principal.role == AdminRole::Viewer {
            return Err(forbidden());
        }
        let revision = crate::admin::api::expected_revision(request.headers())?;
        let key = crate::admin::api::idempotency_key(request.headers())?;
        let input: Value = crate::admin::api::body(request).await?;
        let digest = crate::admin::credentials::digest(
            &serde_json::to_vec(&json!({"traffic":input,"revision":revision}))
                .expect("traffic JSON"),
        );
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            items: Vec<TrafficItem>,
        }
        let input: Input = serde_json::from_value(input)
            .map_err(|_| bad("traffic requires unsigned per-client byte deltas"))?;
        if input.items.is_empty() || input.items.len() > 128 {
            return Err(bad("traffic requires 1..128 items"));
        }
        let principal = principal.clone();
        let node = self.clone();
        // Keep the revision lease until the blocking disk transaction completes,
        // even if the HTTP/RPC request or connector is dropped.
        self.tasks.spawn(async move {
            let daemon = node.daemon.upgrade().ok_or_else(||bad("daemon unavailable"))?;
            let _guard = tokio::select! {
                biased;
                () = node.cancel.cancelled() => return Err(forbidden()),
                guard = daemon.revision_guard(revision) => guard.map_err(|_|Error(StatusCode::CONFLICT,"configuration changed; reload and retry".into()))?,
            };
            let config = daemon.config().1;
            if config.state.is_none() { return Err(bad("traffic import requires state.path for durable accounting")); }
            let mut deltas: BTreeMap<ResourceKey, (ResourceLimits, TrafficStats)> = BTreeMap::new();
            let clients:HashSet<_>=input.items.iter().map(|item|item.client.clone()).collect();
            let affected = input.items.len();
            for item in input.items {
                let client = config.clients.get(&item.client).ok_or_else(missing)?;
                if !resources::visible(&principal,client.user.as_deref()) { return Err(forbidden()); }
                let stats = TrafficStats{inlet:item.inlet_bytes, export:item.export_bytes};
                let limits = ResourceLimits{quota:client.quota,rate_limit:client.rate_limit,max_connections:client.max_connections};
                add(&mut deltas,ResourceKey::Client{name:item.client},limits,stats)?;
                if let Some(user) = &client.user {
                    add(&mut deltas, ResourceKey::User{name:user.clone()},config.users[user].limits,stats)?;
                }
            }
            let store = daemon.server().registry().store.clone();
            let replayed = tokio::task::spawn_blocking(move || store.import(principal.account,key,digest,deltas)).await
                .map_err(|_|bad("traffic import worker failed"))?
                .map_err(|e| {
                    let message: Message = e.to_string().into();
                    Error(
                        if e.kind() == std::io::ErrorKind::InvalidInput {
                            StatusCode::BAD_REQUEST
                        } else {
                            StatusCode::SERVICE_UNAVAILABLE
                        },
                        message,
                    )
                })?;
            if !replayed {node.report_traffic(&config,clients);}
            Ok(json(StatusCode::OK,json!({"revision":revision,"affected":affected,"replayed":replayed,"durable":true})))
        }).await.map_err(|_|bad("traffic import failed"))?
    }
}
fn add(
    deltas: &mut BTreeMap<ResourceKey, (ResourceLimits, TrafficStats)>,
    key: ResourceKey,
    limits: ResourceLimits,
    stats: TrafficStats,
) -> Result<()> {
    let entry = deltas
        .entry(key)
        .or_insert((limits, TrafficStats::default()));
    entry.1.inlet = entry
        .1
        .inlet
        .checked_add(stats.inlet)
        .ok_or_else(|| bad("traffic batch overflows"))?;
    entry.1.export = entry
        .1
        .export
        .checked_add(stats.export)
        .ok_or_else(|| bad("traffic batch overflows"))?;
    Ok(())
}
