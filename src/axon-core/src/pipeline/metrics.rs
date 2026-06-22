//! Per-stage in-memory metrics for the streaming pipeline (CPT-AXO-054).
//!
//! Every stage worker pool shares one [`StageMetrics`] instance and updates it
//! lock-free via atomics. The metrics live in RAM only; export to CSV /
//! Prometheus is layered on top through [`StageSnapshot`].
//!
//! REQ-AXO-901608 extension : temporal metrics `t_recv` / `t_work` / `t_send`
//! distinguish **starvation** (high t_recv = upstream slow), **capacity**
//! (high t_work_ratio = stage saturated by work), and **backpressure** (high
//! t_send = downstream slow). The Goldratt-canonical drum detection rule is
//! `argmax(t_work_ratio)` where `t_work_ratio = t_work / (t_recv + t_work +
//! t_send)`. See CPT-AXO-90025 + DEC-AXO-901597 for the methodology.

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
///   `send().await` because the downstream channel was full. (The legacy
///   cross-pipeline A3→B1 `try_send`-drop case was retired with the demand-pull
///   handoff — REQ-AXO-901746.)
/// * `total_work_duration_us` — sum of per-item work durations in microseconds,
///   enabling running-mean latency without locking a histogram.
/// * `t_recv_total_us` — cumulative time workers spent awaiting `recv().await`
///   on the upstream channel. High value = **starvation** (upstream slow / no
///   material). REQ-AXO-901608.
/// * `t_send_total_us` — cumulative time workers spent awaiting `send().await`
///   on the downstream channel. High value = **backpressure** (downstream
///   slow / buffer full). REQ-AXO-901608.
#[derive(Debug)]
pub struct StageMetrics {
    name: &'static str,
    items_in_total: AtomicU64,
    items_out_total: AtomicU64,
    errors_total: AtomicU64,
    inflight: AtomicUsize,
    backpressure_blocks_total: AtomicU64,
    total_work_duration_us: AtomicU64,
    t_recv_total_us: AtomicU64,
    t_send_total_us: AtomicU64,
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
            t_recv_total_us: AtomicU64::new(0),
            t_send_total_us: AtomicU64::new(0),
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

    /// REQ-AXO-901608 — accumulate the duration a worker spent awaiting an
    /// item on the upstream `recv()`. High totals = starvation indicator.
    pub fn record_recv_wait(&self, duration_us: u64) {
        self.t_recv_total_us
            .fetch_add(duration_us, Ordering::Relaxed);
    }

    /// REQ-AXO-901608 — accumulate the duration a worker spent awaiting the
    /// downstream `send()`. High totals = backpressure indicator.
    pub fn record_send_wait(&self, duration_us: u64) {
        self.t_send_total_us
            .fetch_add(duration_us, Ordering::Relaxed);
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
            backpressure_blocks_total: self.backpressure_blocks_total.load(Ordering::Relaxed),
            mean_duration_us,
            t_recv_total_us: self.t_recv_total_us.load(Ordering::Relaxed),
            t_work_total_us: total_us,
            t_send_total_us: self.t_send_total_us.load(Ordering::Relaxed),
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
    /// REQ-AXO-901608 — total μs workers spent awaiting upstream `recv()`.
    pub t_recv_total_us: u64,
    /// REQ-AXO-901608 — total μs workers spent inside `work_fn` (= sum of
    /// `record_finished` durations). Mirror of `total_work_duration_us`.
    pub t_work_total_us: u64,
    /// REQ-AXO-901608 — total μs workers spent awaiting downstream `send()`.
    pub t_send_total_us: u64,
}

impl StageSnapshot {
    /// REQ-AXO-901608 / CPT-AXO-90025 — Goldratt-canonical drum indicator.
    /// Ratio of time the stage spent doing real work versus all time
    /// accounted for (recv + work + send). Returns `0.0` when no time was
    /// recorded yet (e.g. brand-new pipeline before first item). The stage
    /// with the maximum `t_work_ratio` across the pipeline is the drum.
    pub fn t_work_ratio(&self) -> f64 {
        let total = self.t_recv_total_us + self.t_work_total_us + self.t_send_total_us;
        if total == 0 {
            0.0
        } else {
            self.t_work_total_us as f64 / total as f64
        }
    }
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

    // REQ-AXO-901608 — temporal metrics (starvation / capacity / backpressure)

    #[test]
    fn temporal_metrics_default_to_zero_and_ratio_handles_empty_state() {
        let m = StageMetrics::new("A1");
        let snap = m.snapshot();
        assert_eq!(snap.t_recv_total_us, 0);
        assert_eq!(snap.t_work_total_us, 0);
        assert_eq!(snap.t_send_total_us, 0);
        assert_eq!(
            snap.t_work_ratio(),
            0.0,
            "ratio must be 0.0 when no time accounted yet (no division by zero)"
        );
    }

    #[test]
    fn temporal_metrics_accumulate_recv_send_independently_of_work() {
        let m = StageMetrics::new("A2");
        m.record_recv_wait(500);
        m.record_recv_wait(250);
        m.record_send_wait(100);
        m.record_started();
        m.record_finished(1000);
        let snap = m.snapshot();
        assert_eq!(snap.t_recv_total_us, 750);
        assert_eq!(snap.t_send_total_us, 100);
        assert_eq!(snap.t_work_total_us, 1000, "mirror of total_work_duration");
        // t_work_ratio = 1000 / (750 + 1000 + 100) = 1000 / 1850 ≈ 0.5405
        let r = snap.t_work_ratio();
        assert!(
            (r - 1000.0 / 1850.0).abs() < 1e-9,
            "expected ≈ {:.6}, got {:.6}",
            1000.0 / 1850.0,
            r
        );
    }

    #[test]
    fn temporal_metrics_drum_identification_via_max_t_work_ratio() {
        // Simulated post-bench scenario: A2 is the drum (high t_work_ratio),
        // A1 is starved (high t_recv), A3 is backpressured (high t_send).
        let a1 = StageMetrics::new("A1");
        a1.record_recv_wait(9_000);
        a1.record_started();
        a1.record_finished(500);
        a1.record_send_wait(500);

        let a2 = StageMetrics::new("A2");
        a2.record_recv_wait(100);
        a2.record_started();
        a2.record_finished(9_500);
        a2.record_send_wait(400);

        let a3 = StageMetrics::new("A3");
        a3.record_recv_wait(200);
        a3.record_started();
        a3.record_finished(800);
        a3.record_send_wait(9_000);

        let snaps = [a1.snapshot(), a2.snapshot(), a3.snapshot()];
        let drum = snaps
            .iter()
            .max_by(|a, b| {
                a.t_work_ratio()
                    .partial_cmp(&b.t_work_ratio())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();
        assert_eq!(drum.name, "A2", "A2 must be identified as the drum");

        // A1 is starved (high t_recv_ratio)
        let a1_snap = snaps[0];
        let a1_total = a1_snap.t_recv_total_us + a1_snap.t_work_total_us + a1_snap.t_send_total_us;
        let a1_recv_ratio = a1_snap.t_recv_total_us as f64 / a1_total as f64;
        assert!(a1_recv_ratio > 0.85, "A1 should be starved (>85%)");

        // A3 is backpressured (high t_send_ratio)
        let a3_snap = snaps[2];
        let a3_total = a3_snap.t_recv_total_us + a3_snap.t_work_total_us + a3_snap.t_send_total_us;
        let a3_send_ratio = a3_snap.t_send_total_us as f64 / a3_total as f64;
        assert!(a3_send_ratio > 0.85, "A3 should be backpressured (>85%)");
    }

    #[test]
    fn temporal_metrics_are_monotonic_under_repeated_record() {
        let m = StageMetrics::new("B2");
        let mut last_recv = 0u64;
        let mut last_send = 0u64;
        let mut last_work = 0u64;
        for i in 1..=20 {
            m.record_recv_wait(i);
            m.record_send_wait(i * 2);
            m.record_started();
            m.record_finished(i * 3);
            let snap = m.snapshot();
            assert!(snap.t_recv_total_us >= last_recv);
            assert!(snap.t_send_total_us >= last_send);
            assert!(snap.t_work_total_us >= last_work);
            last_recv = snap.t_recv_total_us;
            last_send = snap.t_send_total_us;
            last_work = snap.t_work_total_us;
        }
        let final_snap = m.snapshot();
        // Sum of 1..=20 = 210 ; *2 = 420 ; *3 = 630
        assert_eq!(final_snap.t_recv_total_us, 210);
        assert_eq!(final_snap.t_send_total_us, 420);
        assert_eq!(final_snap.t_work_total_us, 630);
    }
}
