//! Host-wide measurements, sampled once per management listener rather than per
//! browser. History is bounded and survives forwarding configuration reloads.
use super::{Principal, Result, resources};
use parking_lot::RwLock;
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{Disks, Networks, System};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const COLLECTOR_RESTART_DELAY: Duration = Duration::from_secs(60);
const RETENTION: Duration = Duration::from_secs(3600);
const MAX_SAMPLES: usize = 721;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Host {
    name: Option<String>,
    os: Option<String>,
    architecture: String,
    cpu_cores: usize,
    disk_mount: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Sample {
    pub(crate) timestamp_ms: u64,
    cpu_percent: Option<f32>,
    memory_total_bytes: Option<u64>,
    memory_used_bytes: Option<u64>,
    memory_percent: Option<f64>,
    swap_used_bytes: Option<u64>,
    disk_total_bytes: Option<u64>,
    disk_used_bytes: Option<u64>,
    disk_percent: Option<f64>,
    network_rx_bytes_per_sec: Option<f64>,
    network_tx_bytes_per_sec: Option<f64>,
    uptime_seconds: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct Snapshot {
    host: Option<Host>,
    samples: Vec<Sample>,
    sample_interval_ms: u64,
    retention_seconds: u64,
    started_at_ms: u64,
    server_time_ms: u64,
    sampling_failed: bool,
}

struct State {
    host: Option<Host>,
    // Monotonic times enforce retention even if the host wall clock changes.
    samples: VecDeque<(Instant, Sample)>,
    sampling_failed: bool,
}

pub(crate) struct HostMetrics {
    started_at_ms: u64,
    state: RwLock<State>,
}

impl HostMetrics {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            started_at_ms: now_ms(),
            state: RwLock::new(State {
                host: None,
                samples: VecDeque::new(),
                sampling_failed: false,
            }),
        })
    }

    pub(crate) fn start(self: &Arc<Self>, cancel: CancellationToken, tasks: &TaskTracker) {
        let this = self.clone();
        tasks.spawn(async move {
            // A panic inside an OS query loses the collector it moved into the
            // blocking task. Rebuild it after a pause instead of leaving the
            // monitor empty until the management listener restarts.
            while !this.sample_until_failure(&cancel).await {
                this.state.write().sampling_failed = true;
                tracing::warn!("host metrics sampling failed; restarting the collector");
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(COLLECTOR_RESTART_DELAY) => {}
                }
            }
        });
    }

    /// Samples until cancelled (`true`) or until an OS query fails (`false`).
    async fn sample_until_failure(&self, cancel: &CancellationToken) -> bool {
        // OS queries never block a Tokio worker or an HTTP request.
        let initialized = tokio::task::spawn_blocking(|| {
            let mut collector = Collector::new();
            let sample = collector.sample(false);
            (collector, sample)
        })
        .await;
        let Ok((mut collector, first)) = initialized else {
            return false;
        };
        if cancel.is_cancelled() {
            return true;
        }
        self.record(collector.host(), first);
        let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                _ = interval.tick() => {}
            }
            let next = tokio::task::spawn_blocking(move || {
                let sample = collector.sample(true);
                (collector, sample)
            })
            .await;
            let Ok((returned, sample)) = next else {
                return false;
            };
            collector = returned;
            if cancel.is_cancelled() {
                break;
            }
            self.record(collector.host(), sample);
        }
        true
    }

    fn record(&self, host: Host, sample: Sample) {
        let now = Instant::now();
        let mut state = self.state.write();
        state.host = Some(host);
        state.sampling_failed = false;
        // If NTP moves the wall clock backwards, start a new timeline instead
        // of returning duplicate/non-monotonic x coordinates to the browser.
        if state
            .samples
            .back()
            .is_some_and(|(_, previous)| sample.timestamp_ms <= previous.timestamp_ms)
        {
            state.samples.clear();
        }
        state.samples.push_back((now, sample));
        prune(&mut state.samples, now);
    }

    pub(crate) fn snapshot(&self, principal: &Principal, window: Duration) -> Result<Snapshot> {
        // Host metrics include all tenants' machine usage, so tenant operators
        // and viewers must never receive them, including through platform RPC.
        resources::admin(principal)?;
        let now = Instant::now();
        let state = self.state.read();
        Ok(Snapshot {
            host: state.host.clone(),
            samples: state
                .samples
                .iter()
                .filter(|(at, _)| now.saturating_duration_since(*at) <= window.min(RETENTION))
                .map(|(_, sample)| sample.clone())
                .collect(),
            sample_interval_ms: SAMPLE_INTERVAL.as_millis() as u64,
            retention_seconds: RETENTION.as_secs(),
            started_at_ms: self.started_at_ms,
            server_time_ms: now_ms(),
            sampling_failed: state.sampling_failed,
        })
    }
}

fn prune(samples: &mut VecDeque<(Instant, Sample)>, now: Instant) {
    while samples.len() > MAX_SAMPLES
        || samples
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > RETENTION)
    {
        samples.pop_front();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn percent(used: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

#[derive(Clone, Copy)]
struct Counters {
    rx: u64,
    tx: u64,
}

fn network_rate(
    previous: &HashMap<String, Counters>,
    current: &HashMap<String, Counters>,
    elapsed: Duration,
) -> (Option<f64>, Option<f64>) {
    if elapsed.is_zero() {
        return (None, None);
    }
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut valid = false;
    for (name, next) in current {
        if let Some(old) = previous.get(name)
            && let (Some(incoming), Some(outgoing)) =
                (next.rx.checked_sub(old.rx), next.tx.checked_sub(old.tx))
        {
            rx = rx.saturating_add(incoming);
            tx = tx.saturating_add(outgoing);
            valid = true;
        }
    }
    (
        valid.then(|| rx as f64 / elapsed.as_secs_f64()),
        valid.then(|| tx as f64 / elapsed.as_secs_f64()),
    )
}

struct Collector {
    system: System,
    disks: Disks,
    networks: Networks,
    previous_network: HashMap<String, Counters>,
    previous_time: Instant,
    disk_path: Option<PathBuf>,
    executable: PathBuf,
}

impl Collector {
    fn new() -> Self {
        Self {
            system: System::new(),
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new(),
            previous_network: HashMap::new(),
            previous_time: Instant::now(),
            disk_path: None,
            executable: std::env::current_exe().unwrap_or_default(),
        }
    }

    fn host(&self) -> Host {
        Host {
            name: System::host_name(),
            os: System::long_os_version(),
            architecture: std::env::consts::ARCH.to_owned(),
            cpu_cores: self.system.cpus().len(),
            disk_mount: self
                .disk_path
                .as_ref()
                .map(|path| path.display().to_string()),
        }
    }

    fn sample(&mut self, warmed: bool) -> Sample {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        let now = Instant::now();
        let current: HashMap<_, _> = self
            .networks
            .iter()
            .filter(|(name, _)| {
                let lower = name.to_ascii_lowercase();
                lower != "lo" && lower != "lo0" && !lower.contains("loopback")
            })
            .map(|(name, network)| {
                (
                    name.clone(),
                    Counters {
                        rx: network.total_received(),
                        tx: network.total_transmitted(),
                    },
                )
            })
            .collect();
        let (rx, tx) = if warmed {
            network_rate(
                &self.previous_network,
                &current,
                now.saturating_duration_since(self.previous_time),
            )
        } else {
            (None, None)
        };
        self.previous_network = current;
        self.previous_time = now;
        // Report the filesystem containing the executable, not a sum of
        // mountpoints (APFS volumes/bind mounts can share the same capacity).
        let disk = self
            .disks
            .list_mut()
            .iter_mut()
            .filter(|disk| self.executable.starts_with(disk.mount_point()))
            .max_by_key(|disk| disk.mount_point().components().count())
            .filter(|disk| disk.total_space() > 0);
        let (disk_total, disk_used) = match disk {
            Some(disk) => {
                if !disk.refresh() {
                    self.disk_path = None;
                    return self.finish_sample(warmed, rx, tx, None, None);
                }
                self.disk_path = Some(disk.mount_point().to_owned());
                (
                    Some(disk.total_space()),
                    Some(disk.total_space().saturating_sub(disk.available_space())),
                )
            }
            _ => (None, None),
        };
        self.finish_sample(warmed, rx, tx, disk_total, disk_used)
    }

    fn finish_sample(
        &self,
        warmed: bool,
        rx: Option<f64>,
        tx: Option<f64>,
        disk_total: Option<u64>,
        disk_used: Option<u64>,
    ) -> Sample {
        let memory_total = self.system.total_memory();
        let memory_used = self.system.used_memory().min(memory_total);
        let cpu = self.system.global_cpu_usage();
        Sample {
            timestamp_ms: now_ms(),
            cpu_percent: (warmed && !self.system.cpus().is_empty() && cpu.is_finite())
                .then(|| cpu.clamp(0.0, 100.0)),
            memory_total_bytes: (memory_total > 0).then_some(memory_total),
            memory_used_bytes: (memory_total > 0).then_some(memory_used),
            memory_percent: percent(memory_used, memory_total),
            swap_used_bytes: (memory_total > 0).then(|| self.system.used_swap()),
            disk_total_bytes: disk_total,
            disk_used_bytes: disk_used,
            disk_percent: disk_total
                .zip(disk_used)
                .and_then(|(total, used)| percent(used, total)),
            network_rx_bytes_per_sec: rx,
            network_tx_bytes_per_sec: tx,
            uptime_seconds: System::uptime(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use felucca_core::config::AdminRole;

    fn principal(role: AdminRole) -> Principal {
        Principal {
            account: "test".into(),
            role,
            user: None,
        }
    }

    #[test]
    fn only_admins_can_read_host_wide_history() {
        let metrics = HostMetrics::new();
        for role in [AdminRole::Viewer, AdminRole::Operator] {
            assert!(metrics.snapshot(&principal(role), RETENTION).is_err());
        }
        assert!(
            metrics
                .snapshot(&principal(AdminRole::Admin), RETENTION)
                .is_ok()
        );
    }

    #[test]
    fn history_is_bounded_by_age_and_sample_count() {
        let now = Instant::now();
        let mut samples = VecDeque::new();
        samples.push_back((now - RETENTION - Duration::from_secs(1), Sample::default()));
        samples.push_back((now, Sample::default()));
        prune(&mut samples, now);
        assert_eq!(samples.len(), 1);
        for _ in 0..MAX_SAMPLES + 10 {
            samples.push_back((now, Sample::default()));
        }
        prune(&mut samples, now);
        assert_eq!(samples.len(), MAX_SAMPLES);
    }

    #[test]
    fn network_rates_use_elapsed_time_and_ignore_reset_or_new_interfaces() {
        let previous = HashMap::from([
            ("eth0".into(), Counters { rx: 100, tx: 200 }),
            ("reset".into(), Counters { rx: 500, tx: 500 }),
        ]);
        let current = HashMap::from([
            ("eth0".into(), Counters { rx: 600, tx: 1200 }),
            ("reset".into(), Counters { rx: 5, tx: 5 }),
            (
                "new".into(),
                Counters {
                    rx: 9_000_000,
                    tx: 9_000_000,
                },
            ),
        ]);
        assert_eq!(
            network_rate(&previous, &current, Duration::from_secs(5)),
            (Some(100.0), Some(200.0))
        );
        assert_eq!(
            network_rate(&HashMap::new(), &current, Duration::from_secs(5)),
            (None, None)
        );
        assert_eq!(
            network_rate(&previous, &current, Duration::ZERO),
            (None, None)
        );
    }

    #[test]
    fn clock_rollback_starts_a_new_ordered_timeline() {
        let metrics = HostMetrics::new();
        let host = Host {
            name: None,
            os: None,
            architecture: "test".into(),
            cpu_cores: 1,
            disk_mount: None,
        };
        metrics.record(
            host.clone(),
            Sample {
                timestamp_ms: 100,
                ..Sample::default()
            },
        );
        metrics.record(
            host,
            Sample {
                timestamp_ms: 50,
                ..Sample::default()
            },
        );
        let snapshot = metrics
            .snapshot(&principal(AdminRole::Admin), RETENTION)
            .unwrap();
        assert_eq!(snapshot.samples.len(), 1);
        assert_eq!(snapshot.samples[0].timestamp_ms, 50);
    }
}
