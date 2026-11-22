use super::{Node, store, targets};
use crate::admin::{Error, Result, bad, forbidden, json};
use crate::control::Principal;
use crate::control::resources;
use crate::http_support::Body;
use felucca_core::config::AdminRole;
use http::{Request, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashSet, sync::Arc};

fn conflict() -> Error {
    Error(
        StatusCode::CONFLICT,
        "configuration changed; reload and retry".into(),
    )
}
impl Node {
    pub(crate) async fn api(
        self: Arc<Self>,
        request: Request<Body>,
        principal: &Principal,
        authorized_revision: u64,
    ) -> Result<http::Response<Body>> {
        let (revision, config) = self
            .config()
            .map_err(|_| bad("node control is unavailable"))?;
        if revision != authorized_revision {
            return Err(conflict());
        }
        let path = request.uri().path();
        let state = self.store.snapshot();
        if request.method() == http::Method::GET {
            match path {
                "/api/node/status" | "/api/node/registration" => {
                    let reverse = self.reverse.read();
                    let platforms:Vec<_> = config.platforms.iter().filter(|(id,p)| principal.role==AdminRole::Admin ||
                        (p.user==principal.user && principal.account.strip_prefix("@platform/").is_none_or(|own|own==*id)))
                        .map(|(id,p)|json!({"id":id,"enabled":p.enabled,"scope":p.scope,"user":p.user,"mode":p.mode,
                            "reverse":reverse.get(id).map(|r|&r.status)})).collect();
                    return Ok(json(
                        StatusCode::OK,
                        json!({"node":self.id,"boot":self.boot,"started_ms":self.started,
                        "version":1,"revision":revision,"epoch":state.epoch,"current":state.next-1,
                        "oldest":state.history.front().map(|event|event.sequence),"changes_window":config.changes_window,
                        "platforms":platforms,"journal_error":*self.journal_error.read()}),
                    ));
                }
                "/api/node/usage" => {
                    return Ok(json(StatusCode::OK, self.usage(principal)?));
                }
                "/api/node/changes" => {
                    let mut after = 0u64;
                    let mut limit = 100usize;
                    let mut keys = HashSet::new();
                    for pair in request
                        .uri()
                        .query()
                        .unwrap_or("")
                        .split('&')
                        .filter(|p| !p.is_empty())
                    {
                        let (key, value) = pair
                            .split_once('=')
                            .ok_or_else(|| bad("invalid changes query"))?;
                        if !keys.insert(key) {
                            return Err(bad("duplicate changes query"));
                        }
                        match key {
                            "after" => {
                                let decoded = percent_encoding::percent_decode_str(value)
                                    .decode_utf8()
                                    .map_err(|_| bad("invalid cursor"))?;
                                let (epoch, sequence) = decoded
                                    .split_once(':')
                                    .ok_or_else(|| bad("cursor requires epoch:sequence"))?;
                                let epoch: uuid::Uuid =
                                    epoch.parse().map_err(|_| bad("invalid cursor epoch"))?;
                                after = sequence
                                    .parse()
                                    .map_err(|_| bad("invalid cursor sequence"))?;
                                if epoch != state.epoch {
                                    return Ok(json(
                                        StatusCode::GONE,
                                        json!({"reset_required":true,"epoch":state.epoch,"current":state.next-1}),
                                    ));
                                }
                            }
                            "limit" => {
                                limit = value.parse().map_err(|_| bad("invalid changes limit"))?;
                                if !(1..=512).contains(&limit) {
                                    return Err(bad("changes limit must be 1..512"));
                                }
                            }
                            _ => return Err(bad("unknown changes query")),
                        }
                    }
                    if !keys.contains("after") {
                        after = state.history.front().map_or(0, |event| event.sequence - 1);
                    }
                    if after >= state.next
                        || state
                            .history
                            .front()
                            .is_some_and(|e| after.saturating_add(1) < e.sequence)
                    {
                        return Ok(json(
                            StatusCode::GONE,
                            json!({"reset_required":true,"epoch":state.epoch,"current":state.next-1}),
                        ));
                    }
                    let events: Vec<_> = state
                        .history
                        .iter()
                        .filter(|e| e.sequence > after)
                        .filter(|e| {
                            e.visible(
                                principal.user.as_deref(),
                                principal.role == AdminRole::Admin,
                            )
                        })
                        .take(limit)
                        .collect();
                    let end = events.last().map_or(state.next - 1, |e| e.sequence);
                    return Ok(json(
                        StatusCode::OK,
                        json!({"epoch":state.epoch,"events":events,"cursor":format!("{}:{end}",state.epoch),"current":state.next-1}),
                    ));
                }
                "/api/node/callbacks" => {
                    let mut offset = 0usize;
                    let mut limit = 100usize;
                    let mut selected = None;
                    let mut keys = HashSet::new();
                    for pair in request
                        .uri()
                        .query()
                        .unwrap_or("")
                        .split('&')
                        .filter(|p| !p.is_empty())
                    {
                        let (key, value) = pair
                            .split_once('=')
                            .ok_or_else(|| bad("invalid callback query"))?;
                        if !keys.insert(key) {
                            return Err(bad("duplicate callback query"));
                        }
                        match key {
                            "offset" => {
                                offset = value.parse().map_err(|_| bad("invalid offset"))?
                            }
                            "limit" => {
                                limit = value.parse().map_err(|_| bad("invalid limit"))?;
                                if !(1..=512).contains(&limit) {
                                    return Err(bad("limit must be 1..512"));
                                }
                            }
                            "target" => {
                                selected = Some(
                                    percent_encoding::percent_decode_str(value)
                                        .decode_utf8()
                                        .map_err(|_| bad("invalid target"))?
                                        .into_owned(),
                                )
                            }
                            _ => return Err(bad("unknown callback query")),
                        }
                    }
                    let mut budget = limit;
                    let mut skip = offset;
                    let mut total = 0usize;
                    let queues:Vec<_>=targets(&config).into_values().filter(|t|t.visible(principal)).filter_map(|target|{
                        if selected.as_ref().is_some_and(|id|id!=&target.id){return None;}
                        let queue=state.queues.get(&target.id)?;
                        total+=queue.pending.len();
                        let start=skip.min(queue.pending.len());skip-=start;
                        let items:Vec<_>=queue.pending.iter().skip(start).take(budget).collect();
                        budget-=items.len();
                        Some(json!({"target":target.id,"enabled":target.enabled,"total":queue.pending.len(),"items":items,"counters":queue.counters}))
                    }).collect();
                    return Ok(json(
                        StatusCode::OK,
                        json!({"revision":revision,"queues":queues,"total":total,"offset":offset,"limit":limit,
                            "next_offset":(offset.saturating_add(limit)<total).then_some(offset.saturating_add(limit))}),
                    ));
                }
                _ => return Err(crate::admin::missing()),
            }
        }
        if request.method() != http::Method::POST || principal.role == AdminRole::Viewer {
            return Err(forbidden());
        }
        if matches!(path, "/api/node/kick" | "/api/node/sync") {
            return self.configuration_action(request, principal).await;
        }
        if path == "/api/node/traffic" {
            return self.traffic(request, principal).await;
        }
        let action = match path {
            "/api/node/callbacks/replay" => "replay",
            "/api/node/callbacks/delete" => "delete",
            _ => return Err(crate::admin::missing()),
        }
        .to_owned();
        let expected = crate::admin::api::expected_revision(request.headers())?;
        if expected != revision {
            return Err(conflict());
        }
        let key = crate::admin::api::idempotency_key(request.headers())?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            ids: Vec<String>,
        }
        let input: Input = crate::admin::api::body(request).await?;
        if input.ids.is_empty()
            || input.ids.len() > 128
            || input.ids.iter().any(|id| id.len() > 128)
            || input.ids.iter().collect::<HashSet<_>>().len() != input.ids.len()
        {
            return Err(bad("action requires 1..128 unique ids"));
        }
        let digest = crate::admin::credentials::digest(
            &serde_json::to_vec(&json!({"action":action,"ids":input.ids,"revision":expected}))
                .expect("action digest"),
        );
        let node = self.clone();
        let principal = principal.clone();
        // A dropped HTTP/WS response cannot cancel a committed replay/delete/kick.
        let task = self.tasks.spawn(async move {
            node.action(principal, expected, key, digest, action, input.ids)
                .await
        });
        task.await.map_err(|_| bad("node action failed"))?
    }
    pub(super) fn usage(&self, principal: &Principal) -> Result<serde_json::Value> {
        let daemon = self
            .daemon
            .upgrade()
            .ok_or_else(|| bad("daemon unavailable"))?;
        let (revision, config, server) = daemon.view();
        let clients:Vec<_>=config.clients.iter().filter(|(_,c)|resources::visible(principal,c.user.as_deref())).map(|(id,c)|{
            let online=server.registry().get(id);
            json!({"id":id,"user":c.user,"enabled":c.enabled,"online":online.is_some(),"connections":online.as_ref().map_or(0,|c|c.live_connections()),
                "traffic":server.registry().store.stats(&crate::resources::ResourceKey::Client{name:id.clone()})})
        }).collect();
        Ok(
            json!({"node":self.id,"revision":revision,"timestamp_ms":store::now(),"clients":clients}),
        )
    }
    async fn action(
        self: Arc<Self>,
        principal: Principal,
        revision: u64,
        key: String,
        digest: [u8; 32],
        action: String,
        ids: Vec<String>,
    ) -> Result<http::Response<Body>> {
        let daemon = self
            .daemon
            .upgrade()
            .ok_or_else(|| bad("daemon unavailable"))?;
        let _guard = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(forbidden()),
            guard = daemon.revision_guard(revision) => guard.map_err(|_| conflict())?,
        };
        let config = daemon.config().1;
        let actor = principal.account.clone();
        let targets = targets(
            config
                .node
                .as_ref()
                .ok_or_else(|| bad("node unavailable"))?,
        );
        let visible: HashSet<_> = targets
            .into_values()
            .filter(|t| t.visible(&principal))
            .map(|t| t.id)
            .collect();
        let (affected, replayed) = self
            .store
            .update(move |state| {
                if let Some(previous) = state
                    .actions
                    .iter()
                    .find(|a| a.actor == actor && a.key == key)
                {
                    if previous.digest != digest {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "idempotency key names another action",
                        ));
                    }
                    return Ok((previous.affected, true));
                }
                let affected = {
                    let wanted: HashSet<uuid::Uuid> = ids
                        .iter()
                        .map(|id| {
                            id.parse()
                                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
                        })
                        .collect::<std::io::Result<_>>()?;
                    let mut found = HashSet::new();
                    for (id, queue) in &state.queues {
                        for item in queue.pending.iter().filter(|d| wanted.contains(&d.id)) {
                            if !visible.contains(id) {
                                return Err(std::io::ErrorKind::PermissionDenied.into());
                            }
                            found.insert(item.id);
                        }
                    }
                    if found != wanted {
                        return Err(std::io::ErrorKind::NotFound.into());
                    }
                    for queue in state.queues.values_mut() {
                        if action == "delete" {
                            queue.pending.retain(|d| !wanted.contains(&d.id));
                        } else {
                            for item in queue.pending.iter_mut().filter(|d| wanted.contains(&d.id))
                            {
                                item.failed = false;
                                item.attempts = 0;
                                item.run = item.run.checked_add(1).ok_or_else(|| {
                                    std::io::Error::other("delivery run exhausted")
                                })?;
                                item.next_at_ms = store::now();
                                item.last_error = None;
                                item.last_status = None;
                                queue.counters.replayed = queue.counters.replayed.saturating_add(1);
                            }
                        }
                    }
                    wanted.len()
                };
                state.actions.push_back(store::Action {
                    actor,
                    key,
                    digest,
                    timestamp_ms: store::now(),
                    affected,
                });
                while state.actions.len() > 256 {
                    state.actions.pop_front();
                }
                Ok((affected, false))
            })
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::PermissionDenied => forbidden(),
                std::io::ErrorKind::NotFound => crate::admin::missing(),
                _ => bad("node action could not be committed"),
            })?;
        self.wake.notify_waiters();
        if !replayed {
            self.notify_live("callbacks_queue.updated", principal.user.as_deref(), None);
        }
        Ok(json(
            StatusCode::OK,
            json!({"revision":revision,"affected":affected,"replayed":replayed,"durable":true}),
        ))
    }
}
