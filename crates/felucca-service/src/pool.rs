//! Backend selection.
//!
//! A tunnel may name several backends. Choosing between them lives on the agent
//! because only the agent probes them: a server picking the target would always
//! be a probe interval behind, and would have to carry health state for machines
//! it cannot reach.

use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use felucca_core::config::{Backend, LoadBalance};
use parking_lot::Mutex;

/// One backend and what we know about it.
#[derive(Debug)]
pub struct Member {
    pub addr: String,
    pub weight: u32,
    pub tls: bool,
    pub trust: felucca_core::TrustPolicy,
    /// Whether probes currently say this backend is usable.
    healthy: std::sync::atomic::AtomicBool,
    /// Consecutive probe outcomes, for the eviction and restore thresholds.
    streak: Mutex<Streak>,
    /// Live connections, for least-connections selection.
    live: AtomicUsize,
}

#[derive(Debug, Default)]
struct Streak {
    failures: u32,
    successes: u32,
    last_probe_ms: u64,
    last_success: Option<bool>,
}

impl Member {
    pub fn diagnostic(&self) -> felucca_proto::diagnostics::BackendHealth {
        let streak = self.streak.lock();
        felucca_proto::diagnostics::BackendHealth {
            addr: self.addr.clone(),
            healthy: self.is_healthy(),
            connections: self.live_connections() as u64,
            last_probe_ms: streak.last_probe_ms,
            last_success: streak.last_success,
            failures: streak.failures,
            successes: streak.successes,
        }
    }
    fn new(backend: &Backend) -> Self {
        Self {
            addr: backend.addr.clone(),
            weight: backend.weight.max(1),
            tls: backend.tls,
            trust: backend.trust.clone(),
            // Assumed up until a probe says otherwise: starting down would
            // black-hole traffic for the first probe interval after startup.
            healthy: std::sync::atomic::AtomicBool::new(true),
            streak: Mutex::new(Streak::default()),
            live: AtomicUsize::new(0),
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    pub fn live_connections(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }

    /// Records a successful probe, returning whether this restored the backend.
    pub fn probe_succeeded(&self, needed: u32) -> bool {
        let mut s = self.streak.lock();
        s.last_probe_ms = probe_time();
        s.last_success = Some(true);
        s.failures = 0;
        s.successes = s.successes.saturating_add(1);
        if self.is_healthy() || s.successes < needed.max(1) {
            return false;
        }
        s.successes = 0;
        self.healthy.store(true, Ordering::Relaxed);
        true
    }

    /// Records a failed probe, returning whether this evicted the backend.
    pub fn probe_failed(&self, needed: u32) -> bool {
        let mut s = self.streak.lock();
        s.last_probe_ms = probe_time();
        s.last_success = Some(false);
        s.successes = 0;
        s.failures = s.failures.saturating_add(1);
        if !self.is_healthy() || s.failures < needed.max(1) {
            return false;
        }
        s.failures = 0;
        self.healthy.store(false, Ordering::Relaxed);
        true
    }
}

fn probe_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Holds a backend's connection slot for as long as the connection lives.
#[derive(Debug)]
pub struct Lease {
    member: Arc<Member>,
}

impl Lease {
    /// Where to connect.
    pub fn addr(&self) -> &str {
        &self.member.addr
    }

    pub fn trust(&self) -> &felucca_core::TrustPolicy {
        &self.member.trust
    }

    /// Whether to speak TLS to it.
    pub fn tls(&self) -> bool {
        self.member.tls
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.member.live.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The backends behind one tunnel.
#[derive(Debug)]
pub struct Pool {
    members: Vec<Arc<Member>>,
    strategy: LoadBalance,
    /// Cursor for the round-robin strategies.
    cursor: AtomicU64,
    /// Reserve a least-connections slot before another selector observes it.
    selection: Mutex<()>,
}

impl Pool {
    pub fn new(backends: &[Backend], strategy: LoadBalance) -> Self {
        Self {
            members: backends.iter().map(|b| Arc::new(Member::new(b))).collect(),
            strategy,
            cursor: AtomicU64::new(0),
            selection: Mutex::new(()),
        }
    }

    pub fn members(&self) -> &[Arc<Member>] {
        &self.members
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Healthy backends right now.
    pub fn healthy_count(&self) -> usize {
        self.members.iter().filter(|m| m.is_healthy()).count()
    }

    /// Picks a backend for a connection from `client`.
    ///
    /// When every backend is marked down, one is returned anyway. A health check
    /// can be wrong — a probe port closed while the service is fine, a checker
    /// misconfigured after a deploy — and taking the whole tunnel offline on the
    /// checker's word turns a monitoring fault into an outage. The connection may
    /// fail, which is a better answer than refusing to try.
    pub fn select(&self, client: Option<IpAddr>) -> Option<Lease> {
        if self.members.is_empty() {
            return None;
        }
        let _selection =
            (self.strategy == LoadBalance::LeastConnections).then(|| self.selection.lock());

        let healthy: Vec<&Arc<Member>> = self.members.iter().filter(|m| m.is_healthy()).collect();
        let candidates: Vec<&Arc<Member>> = if healthy.is_empty() {
            tracing::warn!(
                backends = self.members.len(),
                "every backend is marked unhealthy; trying one anyway"
            );
            self.members.iter().collect()
        } else {
            healthy
        };

        let chosen = match self.strategy {
            LoadBalance::RoundRobin => {
                let n = self.cursor.fetch_add(1, Ordering::Relaxed);
                candidates[(n % candidates.len() as u64) as usize]
            }
            LoadBalance::Weighted => {
                // Walk a virtual ring of `weight` slots per backend. Use a
                // 64-bit cursor because the sum of valid u32 weights can
                // exceed u32::MAX.
                let total: u64 = candidates.iter().map(|m| u64::from(m.weight)).sum();
                let mut point = self.cursor.fetch_add(1, Ordering::Relaxed) % total.max(1);
                let mut pick = candidates[0];
                for m in &candidates {
                    if point < u64::from(m.weight) {
                        pick = m;
                        break;
                    }
                    point -= u64::from(m.weight);
                }
                pick
            }
            LoadBalance::LeastConnections => candidates
                .iter()
                .min_by_key(|m| m.live_connections())
                .copied()
                .expect("candidates is non-empty"),
            LoadBalance::SourceHash => {
                // Hash the address so one client keeps landing on one backend.
                // Falls back to round robin when the peer address is unknown,
                // which is better than sending every such connection to one place.
                match client {
                    Some(ip) => {
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        ip.hash(&mut h);
                        candidates[(h.finish() % candidates.len() as u64) as usize]
                    }
                    None => {
                        let n = self.cursor.fetch_add(1, Ordering::Relaxed);
                        candidates[(n % candidates.len() as u64) as usize]
                    }
                }
            }
            // Configured order, so the rest are standby rather than in rotation.
            LoadBalance::Failover => candidates[0],
        };

        chosen.live.fetch_add(1, Ordering::Relaxed);
        Some(Lease {
            member: Arc::clone(chosen),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_selection_does_not_wrap_before_the_end_of_a_large_ring() {
        let pool = Pool::new(
            &[
                Backend {
                    weight: u32::MAX,
                    ..Backend::new("first:80")
                },
                Backend {
                    weight: u32::MAX,
                    ..Backend::new("second:80")
                },
            ],
            LoadBalance::Weighted,
        );
        pool.cursor.store(u32::MAX.into(), Ordering::Relaxed);
        assert_eq!(pool.select(None).unwrap().addr(), "second:80");
        assert_eq!(pool.select(None).unwrap().addr(), "second:80");
    }

    fn backends(specs: &[(&str, u32)]) -> Vec<Backend> {
        specs
            .iter()
            .map(|(addr, weight)| Backend {
                addr: (*addr).to_owned(),
                weight: *weight,
                tls: false,
                trust: Default::default(),
            })
            .collect()
    }

    fn pool(specs: &[(&str, u32)], strategy: LoadBalance) -> Pool {
        Pool::new(&backends(specs), strategy)
    }

    /// Which backend each of `n` selections landed on.
    fn picks(pool: &Pool, n: usize) -> Vec<String> {
        (0..n)
            .map(|_| {
                let lease = pool.select(None).expect("should pick");
                let addr = lease.addr().to_owned();
                // Drop each lease so live counts do not accumulate.
                drop(lease);
                addr
            })
            .collect()
    }

    #[test]
    fn an_empty_pool_selects_nothing() {
        assert!(pool(&[], LoadBalance::RoundRobin).select(None).is_none());
    }

    #[test]
    fn backends_start_healthy() {
        // Starting down would black-hole traffic until the first probe lands.
        let p = pool(&[("a:1", 1), ("b:1", 1)], LoadBalance::RoundRobin);
        assert_eq!(p.healthy_count(), 2);
    }

    #[test]
    fn round_robin_cycles_evenly() {
        let p = pool(
            &[("a:1", 1), ("b:1", 1), ("c:1", 1)],
            LoadBalance::RoundRobin,
        );
        let got = picks(&p, 6);
        assert_eq!(got, vec!["a:1", "b:1", "c:1", "a:1", "b:1", "c:1"]);
    }

    #[test]
    fn weighted_selection_matches_the_ratio_and_interleaves() {
        let p = pool(&[("a:1", 3), ("b:1", 1)], LoadBalance::Weighted);
        let got = picks(&p, 8);
        assert_eq!(got.iter().filter(|x| *x == "a:1").count(), 6);
        assert_eq!(got.iter().filter(|x| *x == "b:1").count(), 2);
        // A block of three in a row would be a burst; the ring interleaves.
        assert_eq!(&got[..4], &["a:1", "a:1", "a:1", "b:1"]);
    }

    #[test]
    fn least_connections_prefers_the_idle_backend() {
        let p = pool(&[("a:1", 1), ("b:1", 1)], LoadBalance::LeastConnections);

        // Hold two connections on whichever comes first.
        let first = p.select(None).unwrap();
        let held = first.addr().to_owned();
        let second = p.select(None).unwrap();
        assert_ne!(second.addr(), held, "the second should go to the idle one");

        drop(second);
        let third = p.select(None).unwrap();
        assert_ne!(third.addr(), held, "the freed backend is now the idle one");
    }

    #[test]
    fn concurrent_least_connections_reserves_each_selected_backend() {
        let p = pool(
            &[("a:1", 1), ("b:1", 1), ("c:1", 1)],
            LoadBalance::LeastConnections,
        );
        let gate = std::sync::Barrier::new(3);
        let largest_spread = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for worker in 0..3 {
                let (p, gate, largest_spread) = (&p, &gate, &largest_spread);
                scope.spawn(move || {
                    for _ in 0..300 {
                        gate.wait();
                        let lease = p.select(None).unwrap();
                        gate.wait();
                        if worker == 0 {
                            let counts: Vec<_> =
                                p.members().iter().map(|m| m.live_connections()).collect();
                            largest_spread.fetch_max(
                                counts.iter().max().unwrap() - counts.iter().min().unwrap(),
                                Ordering::Relaxed,
                            );
                        }
                        gate.wait();
                        drop(lease);
                    }
                });
            }
        });
        assert_eq!(largest_spread.load(Ordering::Relaxed), 0);
        assert!(p.members().iter().all(|m| m.live_connections() == 0));
    }

    #[test]
    fn source_hash_is_stable_per_client() {
        let p = pool(
            &[("a:1", 1), ("b:1", 1), ("c:1", 1)],
            LoadBalance::SourceHash,
        );
        let ip: IpAddr = "203.0.113.7".parse().unwrap();

        let first = p.select(Some(ip)).unwrap().addr().to_owned();
        for _ in 0..20 {
            assert_eq!(
                p.select(Some(ip)).unwrap().addr(),
                first,
                "should be sticky"
            );
        }
    }

    #[test]
    fn source_hash_spreads_different_clients() {
        let p = pool(
            &[("a:1", 1), ("b:1", 1), ("c:1", 1)],
            LoadBalance::SourceHash,
        );
        let mut seen = std::collections::HashSet::new();
        for i in 0..60u8 {
            let ip: IpAddr = format!("198.51.100.{i}").parse().unwrap();
            seen.insert(p.select(Some(ip)).unwrap().addr().to_owned());
        }
        assert!(
            seen.len() > 1,
            "every client landed on one backend: {seen:?}"
        );
    }

    #[test]
    fn source_hash_without_a_peer_address_falls_back_to_rotation() {
        // Sending every anonymous connection to one backend would be worse.
        let p = pool(&[("a:1", 1), ("b:1", 1)], LoadBalance::SourceHash);
        let got = picks(&p, 4);
        assert_eq!(got, vec!["a:1", "b:1", "a:1", "b:1"]);
    }

    #[test]
    fn failover_uses_the_first_healthy_backend() {
        let p = pool(&[("primary:1", 1), ("standby:1", 1)], LoadBalance::Failover);
        assert_eq!(picks(&p, 3), vec!["primary:1", "primary:1", "primary:1"]);

        // Evict the primary; the standby takes over.
        p.members()[0].probe_failed(1);
        assert_eq!(picks(&p, 2), vec!["standby:1", "standby:1"]);
    }

    #[test]
    fn an_unhealthy_backend_is_skipped() {
        let p = pool(&[("a:1", 1), ("b:1", 1)], LoadBalance::RoundRobin);
        p.members()[0].probe_failed(1);
        assert_eq!(p.healthy_count(), 1);

        for addr in picks(&p, 5) {
            assert_eq!(addr, "b:1", "a dead backend must not take its turn");
        }
    }

    #[test]
    fn a_backend_needs_repeated_failures_to_be_evicted() {
        // One dropped packet must not take a healthy backend out of rotation.
        let p = pool(&[("a:1", 1)], LoadBalance::RoundRobin);
        let m = &p.members()[0];

        assert!(!m.probe_failed(3));
        assert!(!m.probe_failed(3));
        assert!(m.is_healthy(), "still in rotation after two failures");
        assert!(m.probe_failed(3), "the third failure evicts it");
        assert!(!m.is_healthy());
    }

    #[test]
    fn a_success_resets_the_failure_streak() {
        let p = pool(&[("a:1", 1)], LoadBalance::RoundRobin);
        let m = &p.members()[0];

        m.probe_failed(3);
        m.probe_failed(3);
        m.probe_succeeded(1);
        // The streak restarted, so two more failures are not enough.
        assert!(!m.probe_failed(3));
        assert!(!m.probe_failed(3));
        assert!(m.is_healthy());
    }

    #[test]
    fn a_recovered_backend_needs_repeated_successes() {
        let p = pool(&[("a:1", 1)], LoadBalance::RoundRobin);
        let m = &p.members()[0];
        m.probe_failed(1);
        assert!(!m.is_healthy());

        assert!(!m.probe_succeeded(2), "one success is not enough");
        assert!(
            !m.is_healthy(),
            "a flapping backend stays out until it settles"
        );
        assert!(m.probe_succeeded(2), "the second restores it");
        assert!(m.is_healthy());
    }

    #[test]
    fn a_failure_resets_the_recovery_streak() {
        let p = pool(&[("a:1", 1)], LoadBalance::RoundRobin);
        let m = &p.members()[0];
        m.probe_failed(1);

        m.probe_succeeded(3);
        m.probe_succeeded(3);
        m.probe_failed(1);
        // Recovery has to start over, so two successes are again not enough.
        assert!(!m.probe_succeeded(3));
        assert!(!m.probe_succeeded(3));
        assert!(!m.is_healthy());
    }

    #[test]
    fn all_backends_down_still_yields_one() {
        // A wrong health check must not turn a monitoring fault into an outage.
        let p = pool(&[("a:1", 1), ("b:1", 1)], LoadBalance::RoundRobin);
        for m in p.members() {
            m.probe_failed(1);
        }
        assert_eq!(p.healthy_count(), 0);
        assert!(
            p.select(None).is_some(),
            "refusing every connection is worse than trying"
        );
    }

    #[test]
    fn a_lease_releases_its_slot_when_dropped() {
        let p = pool(&[("a:1", 1)], LoadBalance::LeastConnections);
        let m = Arc::clone(&p.members()[0]);
        assert_eq!(m.live_connections(), 0);

        let lease = p.select(None).unwrap();
        assert_eq!(m.live_connections(), 1);
        drop(lease);
        assert_eq!(m.live_connections(), 0);
    }

    #[tokio::test]
    async fn a_lease_survives_a_task_that_unwinds() {
        // A task boundary rather than `catch_unwind`: it is the shape the real
        // failure takes, and `Pool` holds a lock so it is not `RefUnwindSafe`.
        let p = Arc::new(pool(&[("a:1", 1)], LoadBalance::LeastConnections));
        let m = Arc::clone(&p.members()[0]);

        let task = tokio::spawn({
            let p = Arc::clone(&p);
            async move {
                let _lease = p.select(None).expect("should pick");
                panic!("a forwarding task failing mid-copy");
            }
        });
        assert!(task.await.is_err(), "the task should have panicked");
        assert_eq!(m.live_connections(), 0, "the slot must not leak");
    }

    #[test]
    fn a_zero_weight_is_floored_rather_than_making_a_backend_unreachable() {
        // Config validation rejects this, but a pool built in code should not
        // silently produce a backend that can never be picked.
        let p = Pool::new(
            &[Backend {
                addr: "a:1".into(),
                weight: 0,
                tls: false,
                trust: Default::default(),
            }],
            LoadBalance::Weighted,
        );
        assert!(p.select(None).is_some());
    }

    #[test]
    fn tls_and_address_travel_with_the_lease() {
        let p = Pool::new(
            &[Backend {
                addr: "secure:443".into(),
                weight: 1,
                tls: true,
                trust: Default::default(),
            }],
            LoadBalance::RoundRobin,
        );
        let lease = p.select(None).unwrap();
        assert_eq!(lease.addr(), "secure:443");
        assert!(lease.tls());
    }
}
