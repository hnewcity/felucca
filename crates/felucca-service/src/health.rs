//! Independent, bounded probes; dropping the owning JoinSet stops every probe.
use crate::pool::Member;
use felucca_core::config::{HealthCheck, HealthCheckKind};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn monitor(
    network: Arc<felucca_transport::network::Network>,
    member: Arc<Member>,
    health: HealthCheck,
    policy: Arc<felucca_core::access::TargetAcl>,
    http2: bool,
) {
    let mut ticks = tokio::time::interval(health.interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        let up = tokio::time::timeout(
            health.timeout,
            probe(&network, &member, &health, &policy, http2),
        )
        .await
        .unwrap_or(false);
        if up {
            if member.probe_succeeded(health.successes_to_restore) {
                tracing::info!(backend = %member.addr, "backend restored");
            }
        } else if member.probe_failed(health.failures_to_evict) {
            tracing::warn!(backend = %member.addr, "backend evicted");
        }
    }
}

async fn probe(
    network: &felucca_transport::network::Network,
    member: &Member,
    health: &HealthCheck,
    policy: &felucca_core::access::TargetAcl,
    http2: bool,
) -> bool {
    if health.kind == HealthCheckKind::None {
        return true;
    }
    let Ok(mut io) = crate::dial::connect_protocol(
        network,
        &member.addr,
        member.tls,
        &member.trust,
        policy,
        http2,
    )
    .await
    else {
        return false;
    };
    if health.kind == HealthCheckKind::Tcp {
        return true;
    }
    if http2 {
        let Ok((mut sender, connection)) = hyper::client::conn::http2::handshake(
            hyper_util::rt::TokioExecutor::new(),
            hyper_util::rt::TokioIo::new(io),
        )
        .await
        else {
            return false;
        };
        let mut driver = tokio::task::JoinSet::new();
        driver.spawn(async move {
            let _ = connection.await;
        });
        let Ok(request) = http::Request::builder()
            .uri(format!(
                "{}://{}{}",
                if member.tls { "https" } else { "http" },
                member.addr,
                health.path
            ))
            .body(http_body_util::Empty::<bytes::Bytes>::new())
        else {
            return false;
        };
        return sender
            .send_request(request)
            .await
            .is_ok_and(|response| health.accepts(response.status().as_u16()));
    }
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        health.path, member.addr
    );
    if io.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut head = Vec::new();
    while head.len() < 16 * 1024 {
        let Ok(b) = io.read_u8().await else {
            return false;
        };
        head.push(b);
        if head.ends_with(b"\r\n\r\n") {
            let mut headers = [httparse::EMPTY_HEADER; 64];
            let mut response = httparse::Response::new(&mut headers);
            return response.parse(&head).is_ok_and(|s| s.is_complete())
                && response.code.is_some_and(|status| health.accepts(status));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pool;
    use felucca_core::config::{Backend, LoadBalance};
    use std::{
        sync::atomic::{AtomicU16, Ordering},
        time::Duration,
    };

    #[tokio::test]
    async fn http_probes_evict_and_restore_a_real_backend() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let status = Arc::new(AtomicU16::new(503));
        let pool = Pool::new(
            &[Backend::new(listener.local_addr().unwrap().to_string())],
            LoadBalance::Failover,
        );
        let mut tasks = tokio::task::JoinSet::new();
        let reply_status = status.clone();
        tasks.spawn(async move {
            loop {
                let (mut io, _) = listener.accept().await.unwrap();
                let mut request = [0; 1024];
                let _ = io.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 {} Probe\r\nContent-Length: 0\r\n\r\n",
                    reply_status.load(Ordering::Relaxed)
                );
                let _ = io.write_all(response.as_bytes()).await;
            }
        });
        let member = pool.members()[0].clone();
        tasks.spawn(monitor(
            Arc::new(Default::default()),
            member.clone(),
            HealthCheck {
                kind: HealthCheckKind::Http,
                interval: Duration::from_millis(30),
                timeout: Duration::from_millis(20),
                failures_to_evict: 2,
                successes_to_restore: 2,
                ..Default::default()
            },
            Arc::new(Default::default()),
            false,
        ));
        tokio::time::timeout(Duration::from_secs(3), async {
            while member.is_healthy() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            status.store(204, Ordering::Relaxed);
            while !member.is_healthy() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("probes must change real pool state");
    }
}
