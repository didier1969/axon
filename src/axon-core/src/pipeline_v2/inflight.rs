//! REQ-AXO-901903 — pipeline-A in-flight memory budget.
//!
//! Restores the byte-budget half of the legacy Memory Shield (commit 67e39321
//! "buffer pool cap 1GB / Ghost Mode sub-1GB"), which the streaming
//! pipeline_v2 migration dropped along with the per-file size cap.
//!
//! The pipeline-A channels (A1→A2, A2→A3) are bounded by item *count*, not by
//! *bytes*: each in-flight `PreparedFile`/`ParsedFile` carries the full source
//! text (up to the 5 MB parse cap), so a burst of large files — or simply
//! scaling the worker pools — lets RAM grow unbounded. Observed: the graph
//! indexer reached 17 GB anon-rss and was OOM-killed at full throughput.
//!
//! This is a single global byte counter, not an RAII permit: the pivot structs
//! derive `Clone`, so a non-`Clone` `OwnedSemaphorePermit` field cannot be
//! threaded through them without a wider refactor. Instead A1 `admit`s a file's
//! `size_bytes` when it enters the pipeline and A3 `release`s it once the batch
//! is persisted (the content is dropped there); the A2 panic path releases too.
//! The backlog claim feeder consults [`over_budget`] and backs off, so total
//! in-flight content settles around the budget regardless of worker-pool size.
//! Balanced on `size_bytes` (carried unchanged A1→A2→A3) so admit/release never
//! drift; `release` saturates so a stray double-release can't underflow.

use std::sync::atomic::{AtomicU64, Ordering};

static IN_FLIGHT_BYTES: AtomicU64 = AtomicU64::new(0);

/// Default in-flight content budget (bytes) — the legacy 1 GB "Ghost Mode"
/// buffer cap. Override via `AXON_A_INFLIGHT_BUDGET_BYTES`.
pub const INFLIGHT_BUDGET_BYTES_DEFAULT: u64 = 1024 * 1024 * 1024;

pub fn budget_bytes() -> u64 {
    std::env::var("AXON_A_INFLIGHT_BUDGET_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(INFLIGHT_BUDGET_BYTES_DEFAULT)
}

/// Account `bytes` as entering pipeline-A RAM (called once per file at A1).
pub fn admit(bytes: u64) {
    IN_FLIGHT_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

/// Account `bytes` as leaving pipeline-A RAM (A3 batch commit, or A2 panic).
/// Saturating: a stray/duplicate release can never underflow the counter.
pub fn release(bytes: u64) {
    let mut cur = IN_FLIGHT_BYTES.load(Ordering::Relaxed);
    loop {
        let next = cur.saturating_sub(bytes);
        match IN_FLIGHT_BYTES.compare_exchange_weak(
            cur,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => cur = observed,
        }
    }
}

pub fn current_bytes() -> u64 {
    IN_FLIGHT_BYTES.load(Ordering::Relaxed)
}

/// True when in-flight content has reached the budget — the backlog claim
/// feeder must stop admitting new files until pipeline A drains.
pub fn over_budget() -> bool {
    current_bytes() >= budget_bytes()
}

/// RAII budget guard. `new` admits `bytes`; `drop` releases them — exactly
/// once, on whatever path the owning [`PreparedFile`]/[`ParsedFile`] is
/// dropped (A3 commit, A1→A2 dedup-skip, channel-send-failure, panic unwind).
/// This is what makes the accounting leak-proof: there is no manual release to
/// forget. The pivot structs hold it as `Option<Arc<InflightGuard>>` so they
/// stay `Clone` (an `Arc` clone shares one guard → released when the last
/// reference drops). Charged on `content.len()` (actual RAM), so skipped files
/// that carry empty content cost nothing. NOT `Clone` itself: duplicating a
/// guard would release bytes that were never admitted.
#[derive(Debug, PartialEq, Eq)]
pub struct InflightGuard {
    bytes: u64,
}

impl InflightGuard {
    /// Admit `bytes` and return a guard that releases them on drop.
    pub fn new(bytes: u64) -> std::sync::Arc<Self> {
        admit(bytes);
        std::sync::Arc::new(Self { bytes })
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        release(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_release_balance_and_saturate() {
        let start = current_bytes();
        admit(1000);
        assert_eq!(current_bytes(), start + 1000);
        release(400);
        assert_eq!(current_bytes(), start + 600);
        // Over-release saturates at 0, never underflows.
        release(u64::MAX);
        assert_eq!(current_bytes(), 0);
    }

    #[test]
    fn guard_releases_on_drop_including_clones() {
        let start = current_bytes();
        let g = InflightGuard::new(500);
        assert_eq!(current_bytes(), start + 500);
        let g2 = g.clone(); // Arc clone — shares ONE guard, no extra admit
        assert_eq!(current_bytes(), start + 500, "Arc clone must not double-admit");
        drop(g);
        assert_eq!(current_bytes(), start + 500, "still held by g2");
        drop(g2);
        assert_eq!(current_bytes(), start, "released once the last Arc drops");
    }

    #[test]
    fn budget_env_override_and_default() {
        assert_eq!(INFLIGHT_BUDGET_BYTES_DEFAULT, 1024 * 1024 * 1024);
        // An invalid / zero override falls back to the default.
        std::env::set_var("AXON_A_INFLIGHT_BUDGET_BYTES", "0");
        assert_eq!(budget_bytes(), INFLIGHT_BUDGET_BYTES_DEFAULT);
        std::env::set_var("AXON_A_INFLIGHT_BUDGET_BYTES", "2048");
        assert_eq!(budget_bytes(), 2048);
        std::env::remove_var("AXON_A_INFLIGHT_BUDGET_BYTES");
    }
}
