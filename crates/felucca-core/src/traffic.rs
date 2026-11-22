//! Traffic accounting.
//!
//! Byte counters live behind atomics so the data path can update them without
//! taking a lock. The Go original guards every `Flow` with a `sync.RWMutex` and
//! pays for it on each copy; here a forwarding loop only ever does relaxed
//! `fetch_add`.
//!
//! Direction is named from the tunnel's point of view: `inlet` is what entered
//! the server from the public side, `export` is what the server sent back out.
//! These are the same names the persisted JSON uses, so a migrated deployment
//! keeps reporting the same numbers.

use std::sync::atomic::{AtomicU64, Ordering};

/// A snapshot of a [`TrafficCounter`], cheap to copy and safe to serialize.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficStats {
    #[serde(default, rename = "inlet_bytes")]
    pub inlet: u64,
    #[serde(default, rename = "export_bytes")]
    pub export: u64,
}

impl TrafficStats {
    pub fn total(&self) -> u64 {
        self.inlet.saturating_add(self.export)
    }
}

/// Live byte counters for one client, tunnel or host.
#[derive(Debug, Default)]
pub struct TrafficCounter {
    inlet: AtomicU64,
    export: AtomicU64,
}

impl TrafficCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores counters from a persisted snapshot.
    pub fn from_stats(stats: TrafficStats) -> Self {
        Self {
            inlet: AtomicU64::new(stats.inlet),
            export: AtomicU64::new(stats.export),
        }
    }

    /// Records `n` bytes arriving from the public side.
    pub fn add_inlet(&self, n: u64) {
        let _ = self
            .inlet
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(n))
            });
    }

    /// Records `n` bytes leaving toward the public side.
    pub fn add_export(&self, n: u64) {
        let _ = self
            .export
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(n))
            });
    }

    pub fn stats(&self) -> TrafficStats {
        TrafficStats {
            inlet: self.inlet.load(Ordering::Relaxed),
            export: self.export.load(Ordering::Relaxed),
        }
    }

    pub fn total(&self) -> u64 {
        self.stats().total()
    }

    /// Zeroes both counters, returning what they held.
    pub fn reset(&self) -> TrafficStats {
        TrafficStats {
            inlet: self.inlet.swap(0, Ordering::Relaxed),
            export: self.export.swap(0, Ordering::Relaxed),
        }
    }
}

/// Why a tunnel is refusing new connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaState {
    Ok,
    /// The byte allowance is used up.
    FlowExhausted,
    /// The subscription end date has passed.
    Expired,
}

impl QuotaState {
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// A byte allowance and an expiry date, either of which may be absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quota {
    /// Total bytes (in + out) permitted; `None` means unmetered.
    #[serde(default)]
    pub flow_limit: Option<u64>,
    /// Unix seconds after which the tunnel stops accepting; `None` never expires.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// Limits shared by all connections belonging to one resource.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    #[serde(default)]
    pub quota: Quota,
    /// Bytes per second; absent or zero means unlimited.
    #[serde(default)]
    pub rate_limit: Option<u64>,
    /// Absent means unlimited; zero deliberately disables admission.
    #[serde(default)]
    pub max_connections: Option<usize>,
}

impl Quota {
    pub const UNLIMITED: Self = Self {
        flow_limit: None,
        expires_at: None,
    };

    /// Checks `used` bytes and the wall clock `now` (unix seconds) against this
    /// quota. Expiry is reported before exhaustion, since it is the more
    /// actionable message when both apply.
    pub fn check(&self, used: u64, now: i64) -> QuotaState {
        if self.expires_at.is_some_and(|at| now >= at) {
            return QuotaState::Expired;
        }
        if self.flow_limit.is_some_and(|limit| used >= limit) {
            return QuotaState::FlowExhausted;
        }
        QuotaState::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate_by_direction() {
        let c = TrafficCounter::new();
        c.add_inlet(100);
        c.add_inlet(20);
        c.add_export(7);
        assert_eq!(
            c.stats(),
            TrafficStats {
                inlet: 120,
                export: 7
            }
        );
        assert_eq!(c.total(), 127);
    }

    #[test]
    fn restored_counters_saturate_instead_of_resetting_usage_on_overflow() {
        let counters = TrafficCounter::from_stats(TrafficStats {
            inlet: u64::MAX - 1,
            export: u64::MAX - 2,
        });
        counters.add_inlet(10);
        counters.add_export(20);
        assert_eq!(
            counters.stats(),
            TrafficStats {
                inlet: u64::MAX,
                export: u64::MAX
            }
        );
    }

    #[test]
    fn reset_returns_and_clears() {
        let c = TrafficCounter::new();
        c.add_inlet(5);
        c.add_export(6);
        assert_eq!(
            c.reset(),
            TrafficStats {
                inlet: 5,
                export: 6
            }
        );
        assert_eq!(c.stats(), TrafficStats::default());
    }

    #[test]
    fn round_trips_through_a_snapshot() {
        let c = TrafficCounter::from_stats(TrafficStats {
            inlet: 9,
            export: 3,
        });
        c.add_inlet(1);
        assert_eq!(
            c.stats(),
            TrafficStats {
                inlet: 10,
                export: 3
            }
        );
    }

    #[test]
    fn snapshot_uses_the_persisted_field_names() {
        let json = serde_json::to_string(&TrafficStats {
            inlet: 1,
            export: 2,
        })
        .unwrap();
        assert!(json.contains("inlet_bytes"), "{json}");
        assert!(json.contains("export_bytes"), "{json}");
    }

    #[test]
    fn unlimited_quota_always_passes() {
        assert!(Quota::UNLIMITED.check(u64::MAX, i64::MAX).is_ok());
    }

    #[test]
    fn flow_limit_trips_at_the_boundary() {
        let q = Quota {
            flow_limit: Some(1000),
            expires_at: None,
        };
        assert!(q.check(999, 0).is_ok());
        assert_eq!(q.check(1000, 0), QuotaState::FlowExhausted);
        assert_eq!(q.check(4000, 0), QuotaState::FlowExhausted);
    }

    #[test]
    fn expiry_trips_at_the_boundary_and_outranks_flow() {
        let q = Quota {
            flow_limit: Some(10),
            expires_at: Some(500),
        };
        assert!(q.check(0, 499).is_ok());
        assert_eq!(q.check(0, 500), QuotaState::Expired);
        // Both conditions true: expiry is the one reported.
        assert_eq!(q.check(999, 501), QuotaState::Expired);
    }

    #[test]
    fn total_saturates_instead_of_wrapping() {
        let s = TrafficStats {
            inlet: u64::MAX,
            export: 5,
        };
        assert_eq!(s.total(), u64::MAX);
    }
}
