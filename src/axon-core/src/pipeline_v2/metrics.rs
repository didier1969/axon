//! Per-stage in-memory metrics for the streaming pipeline (CPT-AXO-054).
//!
//! Every stage worker pool shares one [`StageMetrics`] instance and updates it
//! lock-free via atomics. The metrics live in RAM only; export to CSV /
//! Prometheus is layered on top through [`StageSnapshot`].

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// In-RAM, lock-free counters for a single pipeline stage.
///
/// Each counter has a precise semantic that maps to a metric the operator can
/// reason about:
///
/// * `items_in_total` — cumulative items that started this stage.
/// * `items_out_total` — cumulative items that finished this stage successfully.
/// * `errors_total` — cumulative items whose `work` closure returned `Err(_)`.
/// * `inflight` — items currently being processed.
/// * `backpressure_blocks_total` — bumped each time a worker had to wait on
///   `send().await` because the downstream channel was full, OR a non-blocking
///   `try_send` was dropped (cross-pipeline A3→B1 case).
/// * `total_work_duration_us` — sum of per-item work durations in microseconds,
///   enabling running-mean latency without locking a histogram.
#[derive(Debug)]
pub struct StageMetrics {
    name: &'static str,
    items_in_total: AtomicU64,
    items_out_total: AtomicU64,
    errors_total: AtomicU64,
    inflight: AtomicUsize,
    backpressure_blocks_total: AtomicU64,
    total_work_duration_us: AtomicU64,
}

impl StageMetrics {
    /// Build a fresh metrics instance for the given stage name.
    ///
    /// `name` is the canonical stage identifier (`"A1"`, `"A2"`, …, `"B3"`).
    pub fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            items_in_total: AtomicU64::new(0),
            items_out_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            inflight: AtomicUsize::new(0),
            backpressure_blocks_total: AtomicU64::new(0),
            total_work_duration_us: AtomicU64::new(0),
        })
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn record_started(&self) {
        self.items_in_total.fetch_add(1, Ordering::Relaxed);
        self.inflight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_finished(&self, duration_us: u64) {
        self.items_out_total.fetch_add(1, Ordering::Relaxed);
        self.total_work_duration_us
            .fetch_add(duration_us, Ordering::Relaxed);
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn record_backpressure_block(&self) {
        self.backpressure_blocks_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> StageSnapshot {
        let items_out = self.items_out_total.load(Ordering::Relaxed);
        let total_us = self.total_work_duration_us.load(Ordering::Relaxed);
        let mean_duration_us = if items_out == 0 {
            0
        } else {
            total_us / items_out
        };
        StageSnapshot {
            name: self.name,
            items_in_total: self.items_in_total.load(Ordering::Relaxed),
            items_out_total: items_out,
            errors_total: self.errors_total.load(Ordering::Relaxed),
            inflight: self.inflight.load(Ordering::Relaxed),
            backpressure_blocks_total: self
                .backpressure_blocks_total
                .load(Ordering::Relaxed),
            mean_duration_us,
        }
    }
}

/// Immutable point-in-time view of a stage's counters.
///
/// Produced by [`StageMetrics::snapshot`]. Cheap to copy and serialise for CSV
/// or Prometheus export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageSnapshot {
    pub name: &'static str,
    pub items_in_total: u64,
    pub items_out_total: u64,
    pub errors_total: u64,
    pub inflight: usize,
    pub backpressure_blocks_total: u64,
    pub mean_duration_us: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_metrics_track_in_out_and_inflight_counters() {
        let m = StageMetrics::new("A1");
        m.record_started();
        m.record_started();
        m.record_finished(150);
        m.record_finished(250);
        let snap = m.snapshot();
        assert_eq!(snap.items_in_total, 2);
        assert_eq!(snap.items_out_total, 2);
        assert_eq!(snap.inflight, 0);
        assert_eq!(snap.errors_total, 0);
        assert_eq!(snap.mean_duration_us, 200);
        assert_eq!(snap.name, "A1");
    }

    #[test]
    fn stage_metrics_record_error_decrements_inflight() {
        let m = StageMetrics::new("A2");
        m.record_started();
        m.record_error();
        let snap = m.snapshot();
        assert_eq!(snap.items_in_total, 1);
        assert_eq!(snap.items_out_total, 0);
        assert_eq!(snap.errors_total, 1);
        assert_eq!(snap.inflight, 0);
        assert_eq!(snap.mean_duration_us, 0);
    }

    #[test]
    fn stage_metrics_backpressure_block_counter_independent_of_lifecycle() {
        let m = StageMetrics::new("A3");
        m.record_backpressure_block();
        m.record_backpressure_block();
        m.record_backpressure_block();
        let snap = m.snapshot();
        assert_eq!(snap.backpressure_blocks_total, 3);
        assert_eq!(snap.items_in_total, 0);
    }
}
