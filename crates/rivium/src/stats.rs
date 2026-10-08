//! Rivium's own counters.

use std::sync::atomic::{AtomicU64, Ordering};

/// In-process restarts, counted by the hosts.
pub(crate) static RESTARTS: AtomicU64 = AtomicU64::new(0);
/// Faults of services, counted by the hosts.
pub(crate) static FAULTS: AtomicU64 = AtomicU64::new(0);

/// A snapshot of Rivium's own counters, which only grow. Services can show them on a status
/// endpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stats {
    /// Bytes written to the log files.
    pub log_bytes: u64,
    /// Failed writes, renames and compressions of the log files.
    pub log_failures: u64,
    /// Log files deleted to keep the log directory within its budget.
    pub log_deleted: u64,
    /// Restarts of the services in the same process.
    pub restarts: u64,
    /// Services that failed after they were ready.
    pub faults: u64,
}

/// Rivium's counters now.
#[must_use]
pub fn stats() -> Stats {
    let [log_bytes, log_failures, log_deleted] = crate::log::counters();
    Stats {
        log_bytes,
        log_failures,
        log_deleted,
        restarts: RESTARTS.load(Ordering::Relaxed),
        faults: FAULTS.load(Ordering::Relaxed),
    }
}
