use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Reservations include unused advertised credit, not just buffered data.
#[derive(Debug)]
pub(crate) struct WindowBudget {
    limit: u64,
    used: AtomicU64,
}
impl WindowBudget {
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            used: AtomicU64::new(0),
        })
    }
    pub fn take(&self, amount: u32) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(u64::from(amount))
                    .filter(|total| *total <= self.limit)
            })
            .is_ok()
    }
    pub fn release(&self, amount: u32) {
        self.used.fetch_sub(u64::from(amount), Ordering::AcqRel);
    }
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }
}
