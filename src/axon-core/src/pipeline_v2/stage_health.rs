//! Process-global pipeline-stage health signal for self-diagnosis
//! (REQ-AXO-902047, PIL-AXO-9006).
//!
//! Bridges a stage's error state to two consumers without threading handles
//! through the pipeline wiring:
//!   1. the vector sorted-drain's **systemic-failure backoff** — when the B3
//!      persist stage is failing every batch (e.g. a corrupt index, a schema
//!      mismatch), re-embedding work that cannot be written is wasted CPU; the
//!      drain backs off instead of spinning at hundreds of % CPU (the
//!      REQ-AXO-902046 incident),
//!   2. future cross-process publication (slice 2) so `embedding_status` /
//!      `pipeline_health` can surface the real error to an LLM in one call.
//!
//! In-RAM, lock-free for the hot counters; the last-error text is behind a
//! `Mutex` updated only on the (rare, by design) error path, deduplicated by
//! message signature so a 7000×-repeated error is one record with a count, not
//! a log flood.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Deduplicated record of the most recent distinct error a stage produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StageErrorRecord {
    /// Full error text (anyhow alternate Display — the whole `caused by` chain
    /// on one line, so the root PG/SQLSTATE detail is preserved, not masked).
    pub message: String,
    /// How many consecutive times THIS exact message repeated.
    pub count: u64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

/// Health signal for one persist stage (currently B3 — the embedding writer).
#[derive(Debug, Default)]
pub struct StageHealth {
    consecutive_failures: AtomicU64,
    total_failures: AtomicU64,
    total_successes: AtomicU64,
    last_error: Mutex<Option<StageErrorRecord>>,
}

impl StageHealth {
    /// Record a failure. Returns the new consecutive-failure count so the
    /// caller can throttle its log (e.g. warn on 1 + every Nth). Dedupes the
    /// stored `last_error` by message signature.
    pub fn record_failure(&self, message: impl Into<String>, now_ms: i64) -> u64 {
        let n = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        self.total_failures.fetch_add(1, Ordering::Relaxed);
        let msg = message.into();
        if let Ok(mut guard) = self.last_error.lock() {
            match guard.as_mut() {
                Some(rec) if rec.message == msg => {
                    rec.count = rec.count.saturating_add(1);
                    rec.last_seen_ms = now_ms;
                }
                _ => {
                    *guard = Some(StageErrorRecord {
                        message: msg,
                        count: 1,
                        first_seen_ms: now_ms,
                        last_seen_ms: now_ms,
                    });
                }
            }
        }
        n
    }

    /// Record a successful batch — resets the consecutive-failure counter so a
    /// transient blip does not latch the backoff.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.total_successes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }

    pub fn total_failures(&self) -> u64 {
        self.total_failures.load(Ordering::Relaxed)
    }

    pub fn total_successes(&self) -> u64 {
        self.total_successes.load(Ordering::Relaxed)
    }

    /// True once the stage has failed `threshold` times in a row with no
    /// intervening success — i.e. the failure is systemic (not a single poison
    /// row) and the upstream drain should back off.
    pub fn is_systemically_failing(&self, threshold: u64) -> bool {
        self.consecutive_failures() >= threshold
    }

    pub fn last_error(&self) -> Option<StageErrorRecord> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }

    /// REQ-AXO-902047 slice 1b — flat, owned snapshot of the health counters
    /// for cross-process publication (the indexer captures this on every
    /// heartbeat tick and UPSERTs it so the brain's `embedding_status` reads
    /// the real B3 error state in one MCP call, no log access).
    pub fn snapshot(&self) -> StageHealthSnapshot {
        StageHealthSnapshot {
            consecutive_failures: self.consecutive_failures(),
            total_failures: self.total_failures(),
            total_successes: self.total_successes(),
            last_error: self.last_error(),
        }
    }
}

/// REQ-AXO-902047 slice 1b — owned, serializable snapshot of a [`StageHealth`].
/// Decoupled from the atomics so it can cross the process boundary (heartbeat
/// UPSERT) and be compared in tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageHealthSnapshot {
    pub consecutive_failures: u64,
    pub total_failures: u64,
    pub total_successes: u64,
    pub last_error: Option<StageErrorRecord>,
}

impl StageHealthSnapshot {
    /// True once consecutive failures reach the systemic threshold — the same
    /// verdict the drain uses to back off, surfaced to readers as DEGRADED.
    pub fn is_systemically_failing(&self, threshold: u64) -> bool {
        self.consecutive_failures >= threshold
    }

    /// Error rate over the lifetime of the process: failures / (failures +
    /// successes). Returns 0.0 when no batch has been attempted yet.
    pub fn error_rate(&self) -> f64 {
        let total = self.total_failures + self.total_successes;
        if total == 0 {
            0.0
        } else {
            self.total_failures as f64 / total as f64
        }
    }
}

/// Consecutive-failure count at which B3 is judged systemically broken and the
/// drain backs off. 8 batches (~tens of seconds at production cadence) is long
/// enough to rule out a single transient flush, short enough to stop the CPU
/// hemorrhage quickly.
pub const B3_SYSTEMIC_FAILURE_THRESHOLD: u64 = 8;

static B3_HEALTH: OnceLock<StageHealth> = OnceLock::new();

/// Process-global B3 (embedding persist) health signal.
pub fn b3_health() -> &'static StageHealth {
    B3_HEALTH.get_or_init(StageHealth::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_failure_increments_consecutive_and_dedupes_same_message() {
        let h = StageHealth::default();
        assert_eq!(h.record_failure("boom", 10), 1);
        assert_eq!(h.record_failure("boom", 20), 2);
        assert_eq!(h.record_failure("boom", 30), 3);
        assert_eq!(h.consecutive_failures(), 3);
        assert_eq!(h.total_failures(), 3);
        let rec = h.last_error().unwrap();
        assert_eq!(rec.message, "boom");
        assert_eq!(rec.count, 3, "same message must dedupe into one record");
        assert_eq!(rec.first_seen_ms, 10);
        assert_eq!(rec.last_seen_ms, 30);
    }

    #[test]
    fn distinct_message_replaces_last_error_record() {
        let h = StageHealth::default();
        h.record_failure("missing chunk number 0 for toast value (XX001)", 1);
        h.record_failure("different vector dimensions 1024 and 0", 2);
        let rec = h.last_error().unwrap();
        assert_eq!(rec.message, "different vector dimensions 1024 and 0");
        assert_eq!(rec.count, 1);
    }

    #[test]
    fn record_success_resets_consecutive_but_keeps_totals() {
        let h = StageHealth::default();
        h.record_failure("x", 1);
        h.record_failure("x", 2);
        assert!(h.is_systemically_failing(2));
        h.record_success();
        assert_eq!(h.consecutive_failures(), 0);
        assert!(!h.is_systemically_failing(2));
        assert_eq!(h.total_failures(), 2);
        assert_eq!(h.total_successes(), 1);
    }

    #[test]
    fn snapshot_mirrors_live_counters_and_last_error() {
        let h = StageHealth::default();
        h.record_success();
        h.record_failure("missing chunk number 0 for toast value (XX001)", 42);
        let snap = h.snapshot();
        assert_eq!(snap.consecutive_failures, 1);
        assert_eq!(snap.total_failures, 1);
        assert_eq!(snap.total_successes, 1);
        assert_eq!(
            snap.last_error.as_ref().map(|r| r.message.as_str()),
            Some("missing chunk number 0 for toast value (XX001)")
        );
    }

    #[test]
    fn snapshot_error_rate_and_systemic_verdict() {
        let empty = StageHealthSnapshot::default();
        assert_eq!(empty.error_rate(), 0.0, "no batches → no error rate");
        assert!(!empty.is_systemically_failing(B3_SYSTEMIC_FAILURE_THRESHOLD));

        let snap = StageHealthSnapshot {
            consecutive_failures: B3_SYSTEMIC_FAILURE_THRESHOLD,
            total_failures: 3,
            total_successes: 1,
            last_error: None,
        };
        assert!(snap.is_systemically_failing(B3_SYSTEMIC_FAILURE_THRESHOLD));
        assert_eq!(snap.error_rate(), 0.75);
    }

    #[test]
    fn systemic_failure_latches_only_at_threshold() {
        let h = StageHealth::default();
        for i in 1..B3_SYSTEMIC_FAILURE_THRESHOLD {
            h.record_failure("e", i as i64);
            assert!(
                !h.is_systemically_failing(B3_SYSTEMIC_FAILURE_THRESHOLD),
                "must not latch before threshold"
            );
        }
        h.record_failure("e", 99);
        assert!(h.is_systemically_failing(B3_SYSTEMIC_FAILURE_THRESHOLD));
    }
}
