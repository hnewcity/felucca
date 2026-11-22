//! HTTP callback delivery: durable before send, stable IDs across retries.
use super::{Node, Target, store};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use std::{io, sync::Arc, time::Duration};

pub(super) fn signature(
    key: &felucca_core::Secret,
    timestamp: u64,
    id: uuid::Uuid,
    body: &[u8],
) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    let mut context = ring::hmac::Context::with_key(&key);
    context.update(format!("felucca-callback-v1\n{timestamp}\n{id}\n").as_bytes());
    context.update(body);
    let mut result = String::from("v1=");
    for byte in context.sign().as_ref() {
        use std::fmt::Write;
        write!(result, "{byte:02x}").expect("hex string");
    }
    result
}
type HttpClient = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Full<Bytes>,
>;
fn client() -> io::Result<HttpClient> {
    let tls = felucca_transport::Verification::WebPki {
        server_name: String::new(),
    }
    .rustls_client_config()
    .map_err(io::Error::other)?;
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    Ok(
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(https),
    )
}
async fn deliver(
    client: &HttpClient,
    node: &Node,
    target: &Target,
    delivery: &store::Delivery,
) -> Result<u16, (Option<u16>, &'static str)> {
    let state = node.store.snapshot();
    let envelope = serde_json::json!({"version":1,"node":node.id,"epoch":state.epoch,"id":delivery.id,"event":delivery.event});
    let (body, headers) = target
        .delivery
        .content
        .render(&envelope)
        .map_err(|_| (None, "callback template failed"))?;
    post(
        client,
        target,
        delivery.id,
        serde_json::to_vec(&body).expect("bounded callback"),
        headers,
    )
    .await
}
async fn post(
    client: &HttpClient,
    target: &Target,
    id: uuid::Uuid,
    body: Vec<u8>,
    headers: std::collections::BTreeMap<String, String>,
) -> Result<u16, (Option<u16>, &'static str)> {
    let timestamp = store::now();
    let tag = signature(&target.delivery.signing_key, timestamp, id, &body);
    let mut request = http::Request::post(&target.delivery.url)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-felucca-event-id", id.to_string())
        .header("x-felucca-timestamp", timestamp.to_string())
        .header("x-felucca-signature", tag)
        .body(Full::new(Bytes::from(body)))
        .map_err(|_| (None, "invalid callback request"))?;
    request
        .headers_mut()
        .get_mut("x-felucca-signature")
        .expect("signature")
        .set_sensitive(true);
    for (name, value) in headers {
        request.headers_mut().insert(
            http::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| (None, "invalid callback header"))?,
            http::HeaderValue::from_str(&value).map_err(|_| (None, "invalid callback header"))?,
        );
    }
    let response = client
        .request(request)
        .await
        .map_err(|_| (None, "callback connection failed"))?;
    let status = response.status();
    // Discard only a bounded acknowledgement body; never retain/log remote data.
    let mut response = response.into_body();
    let mut count = 0;
    while let Some(frame) = response.frame().await {
        let Ok(frame) = frame else {
            break;
        };
        if let Ok(data) = frame.into_data() {
            count += data.len();
        }
        if count > 4096 {
            break;
        }
    }
    if status.is_success() {
        Ok(status.as_u16())
    } else {
        Err((
            Some(status.as_u16()),
            "callback returned a non-success status",
        ))
    }
}
/// Traffic reports intentionally have no retry/outbox. Each target sends one at
/// a time, and a slow consumer skips reports once its bounded subscription lags.
pub(super) async fn reports(node: Arc<Node>, target: Target) {
    let mut reports = node.reports.subscribe();
    let Ok(client) = client() else {
        return;
    };
    loop {
        let report = tokio::select! {
            ()=node.cancel.cancelled()=>return,
            report=reports.recv()=>match report {
                Ok(report)=>report,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>continue,
                Err(_)=>return,
            }
        };
        if target.user.is_some() && target.user != report.user {
            continue;
        }
        let event = serde_json::to_value(report).expect("report");
        if !target.delivery.selector.matches(&event) {
            continue;
        }
        let id = uuid::Uuid::new_v4();
        let envelope = serde_json::json!({"version":1,"node":node.id,"epoch":node.store.snapshot().epoch,"id":id,"ephemeral":true,"event":event});
        let Ok((body, headers)) = target.delivery.content.render(&envelope) else {
            continue;
        };
        let body = serde_json::to_vec(&body).expect("traffic report");
        tokio::select! {
            ()=node.cancel.cancelled()=>return,
            _=tokio::time::timeout(target.delivery.timeout,post(&client,&target,id,body,headers))=>{},
        }
    }
}
pub(super) async fn run(node: Arc<Node>, target: Target) {
    let client = match client() {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error,"callback HTTP client unavailable");
            return;
        }
    };
    let generation = target.generation();
    loop {
        let due = node
            .store
            .snapshot()
            .queues
            .get(&target.id)
            .filter(|q| q.generation == generation)
            .and_then(|q| {
                q.pending
                    .iter()
                    .filter(|d| !d.failed)
                    .min_by_key(|d| d.next_at_ms)
            })
            .cloned();
        let wait = due.as_ref().map_or(Duration::from_secs(1), |d| {
            Duration::from_millis(d.next_at_ms.saturating_sub(store::now()))
        });
        if !wait.is_zero() {
            tokio::select! { ()=node.cancel.cancelled()=>return, ()=node.wake.notified()=>{}, ()=tokio::time::sleep(wait.min(Duration::from_secs(1)))=>{} }
            continue;
        }
        let Some(due) = due else {
            continue;
        };
        let id = target.id.clone();
        let begun = node
            .store
            .update(move |state| {
                let current = state
                    .queues
                    .get_mut(&id)
                    .filter(|q| q.generation == generation)
                    .and_then(|q| q.pending.iter_mut().find(|d| d.id == due.id && !d.failed));
                let Some(current) = current else {
                    return Ok(None);
                };
                current.attempts = current.attempts.saturating_add(1);
                current.run = current
                    .run
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("delivery run exhausted"))?;
                Ok(Some(current.clone()))
            })
            .await;
        let delivery = match begun {
            Ok(Some(delivery)) => delivery,
            Ok(None) => continue,
            Err(error) => {
                tracing::error!(%error,target=target.id,"callback journal update failed");
                tokio::select! { ()=node.cancel.cancelled()=>return, ()=tokio::time::sleep(Duration::from_secs(1))=>{} }
                continue;
            }
        };
        let result = tokio::select! {
            ()=node.cancel.cancelled()=>return,
            result=tokio::time::timeout(target.delivery.timeout,deliver(&client,&node,&target,&delivery))=>
                result.unwrap_or(Err((None,"callback timed out"))),
        };
        let target_id = target.id.clone();
        let owner = target.user.clone();
        let succeeded = result.is_ok();
        let target = target.clone();
        let recorded = node
            .store
            .update(move |state| {
                let Some(queue) = state
                    .queues
                    .get_mut(&target.id)
                    .filter(|q| q.generation == generation)
                else {
                    return Ok(false);
                };
                let Some(index) = queue.pending.iter().position(|d| d.id == delivery.id) else {
                    return Ok(false);
                };
                // A manual action racing delivery may have reset the attempt. Do not
                // let an old outcome overwrite a newer replay decision.
                if queue.pending[index].run != delivery.run {
                    return Ok(false);
                }
                match result {
                    Ok(_) => {
                        queue.pending.remove(index);
                        queue.counters.delivered = queue.counters.delivered.saturating_add(1);
                        queue.counters.last_delivered_ms = Some(store::now());
                    }
                    Err((status, error)) => {
                        let current = &mut queue.pending[index];
                        current.last_status = status;
                        current.last_error = Some(error.into());
                        if current.attempts <= target.delivery.retry_max {
                            current.next_at_ms = store::now()
                                .saturating_add(target.delivery.retry_backoff.as_millis() as u64);
                        } else {
                            queue.counters.failed = queue.counters.failed.saturating_add(1);
                            if target.delivery.queue_max == 0 {
                                queue.pending.remove(index);
                            } else {
                                current.failed = true;
                            }
                        }
                    }
                }
                Ok(true)
            })
            .await;
        match recorded {
            Ok(true) => {
                node.notify_live(
                    "callbacks_queue.updated",
                    owner.as_deref(),
                    Some(&target_id),
                );
                node.notify_live(
                    if succeeded {
                        "webhook.delivery_succeeded"
                    } else {
                        "webhook.delivery_failed"
                    },
                    owner.as_deref(),
                    Some(&target_id),
                );
            }
            Ok(false) => {}
            Err(error) => {
                tracing::error!(%error,"callback result could not be persisted; delivery may be repeated after restart")
            }
        }
    }
}
