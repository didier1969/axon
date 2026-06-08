//! REQ-AXO-901919 — in-flight file observability for pipeline A.
//!
//! When pipeline A wedges, throughput counters simply stop — they never name
//! WHICH file is stuck, in WHICH stage, for HOW long. Localising the chunker
//! encode-storm (REQ-AXO-901917) required a bespoke watchdog-instrumented
//! diagnostic because no runtime gauge could answer "what is A stuck on right
//! now?". This module is that gauge, made permanent: a process-global registry
//! of the files currently being processed, with the OLDEST in-flight item
//! (path + stage + elapsed) exposed for telemetry and watched by an in-process
//! watchdog that WARNs the moment one exceeds a budget.
//!
//! It also makes the uncancellable-`spawn_blocking` orphan class (REQ-AXO-901918)
//! observable: a file that times out at the stage level but whose worker thread
//! keeps spinning stays registered until the work actually returns, so the
//! watchdog keeps naming it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One in-flight work item.
#[derive(Clone, Debug)]
struct Entry {
    stage: &'static str,
    path: String,
    started: Instant,
}

/// A point-in-time view of the OLDEST in-flight item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlightSnapshot {
    pub stage: &'static str,
    pub path: String,
    pub age_ms: u128,
}

/// Process-global in-flight registry. Keyed by a monotonic sequence so the
/// lowest live key is always the earliest-started (oldest) item — O(log n)
/// `snapshot` via `BTreeMap::first_key_value`.
pub struct InFlightRegistry {
    seq: AtomicU64,
    entries: Mutex<BTreeMap<u64, Entry>>,
}

static GLOBAL: OnceLock<InFlightRegistry> = OnceLock::new();

impl InFlightRegistry {
    pub fn global() -> &'static InFlightRegistry {
        GLOBAL.get_or_init(|| InFlightRegistry {
            seq: AtomicU64::new(0),
            entries: Mutex::new(BTreeMap::new()),
        })
    }

    /// Register `path` as in-flight at `stage`. The returned guard removes the
    /// entry on drop — including when the future is cancelled (drop runs) and,
    /// for `spawn_blocking`, when the blocking closure finally returns. Hold the
    /// guard across the heavy work; the lock is taken only at enter/exit.
    #[must_use]
    pub fn enter(&'static self, stage: &'static str, path: impl Into<String>) -> InFlightGuard {
        let id = self.seq.fetch_add(1, Ordering::Relaxed);
        let entry = Entry {
            stage,
            path: path.into(),
            started: Instant::now(),
        };
        self.entries.lock().unwrap().insert(id, entry);
        InFlightGuard { registry: self, id }
    }

    /// The oldest live item, or `None` when nothing is in flight.
    pub fn snapshot(&self) -> Option<InFlightSnapshot> {
        let guard = self.entries.lock().unwrap();
        guard.first_key_value().map(|(_, e)| InFlightSnapshot {
            stage: e.stage,
            path: e.path.clone(),
            age_ms: e.started.elapsed().as_millis(),
        })
    }

    /// Count of items currently in flight.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn remove(&self, id: u64) {
        self.entries.lock().unwrap().remove(&id);
    }
}

/// RAII guard: drops the in-flight entry it created. `#[must_use]` so a caller
/// cannot accidentally drop it immediately and lose the registration.
#[must_use = "hold the guard across the work; dropping it immediately unregisters the file"]
pub struct InFlightGuard {
    registry: &'static InFlightRegistry,
    id: u64,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.registry.remove(self.id);
    }
}

/// Default age (ms) past which the watchdog WARNs about a stuck file.
const WATCHDOG_WARN_MS_DEFAULT: u128 = 20_000;

fn watchdog_warn_ms() -> u128 {
    std::env::var("AXON_INFLIGHT_WARN_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u128>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(WATCHDOG_WARN_MS_DEFAULT)
}

/// Spawn the in-process watchdog: every `interval`, if the oldest in-flight
/// item exceeds the warn budget, emit a WARN naming the file + stage + age.
/// Idempotent-friendly — call once per pipeline-A activation. A single spinning
/// `spawn_blocking` thread does not starve this tokio task (separate threads).
pub fn spawn_watchdog() {
    // Idempotent: only one watchdog per process even if pipeline A is
    // re-activated, so WARNs are not multiplied.
    static SPAWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::Relaxed) {
        return;
    }
    let interval = Duration::from_millis(
        std::env::var("AXON_INFLIGHT_WATCHDOG_INTERVAL_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v >= 250)
            .unwrap_or(3_000),
    );
    tokio::spawn(async move {
        let warn_ms = watchdog_warn_ms();
        loop {
            tokio::time::sleep(interval).await;
            if let Some(snap) = InFlightRegistry::global().snapshot() {
                if snap.age_ms >= warn_ms {
                    tracing::warn!(
                        target: "pipeline_v2::in_flight",
                        stage = snap.stage,
                        path = %snap.path,
                        age_ms = snap.age_ms as u64,
                        in_flight = InFlightRegistry::global().len(),
                        "REQ-AXO-901919 pipeline-A file in-flight past budget — likely a stage spin / spawn_blocking orphan; INVESTIGATE this file"
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // A dedicated registry instance for deterministic, isolated unit tests
    // (the process-global is shared across the suite).
    fn fresh() -> &'static InFlightRegistry {
        // Leak a fresh registry so it satisfies the `&'static` enter signature
        // without touching the process-global GLOBAL used in production.
        Box::leak(Box::new(InFlightRegistry {
            seq: AtomicU64::new(0),
            entries: Mutex::new(BTreeMap::new()),
        }))
    }

    #[test]
    fn snapshot_reports_the_oldest_live_entry() {
        let reg = fresh();
        assert!(reg.snapshot().is_none(), "empty registry has no in-flight");

        let g1 = reg.enter("A2", "/tmp/a.rs");
        std::thread::sleep(Duration::from_millis(5));
        let g2 = reg.enter("A1", "/tmp/b.rs");

        let snap = reg.snapshot().expect("one item in flight");
        assert_eq!(snap.path, "/tmp/a.rs", "oldest = first entered");
        assert_eq!(snap.stage, "A2");
        assert_eq!(reg.len(), 2);

        // Dropping the oldest promotes the next.
        drop(g1);
        let snap = reg.snapshot().expect("still one in flight");
        assert_eq!(snap.path, "/tmp/b.rs", "oldest now = the remaining entry");
        assert_eq!(reg.len(), 1);

        drop(g2);
        assert!(reg.snapshot().is_none(), "all guards dropped → nothing in flight");
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn guard_drop_unregisters_even_without_snapshot() {
        let reg = fresh();
        {
            let _g = reg.enter("A1", "/tmp/x.rs");
            assert_eq!(reg.len(), 1);
        }
        assert_eq!(reg.len(), 0, "scope-exit drop removed the entry");
    }
}
