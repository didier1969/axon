//! REQ-AXO-901677 — periodic_sweep_worker tests.
//!
//! The worker is the reconciliation safety net for inotify drops :
//!   * Queue overflows on large refactors (>16k events)
//!   * Mount changes (network FS, container vol remount)
//!   * Silent `inotify_init` failures
//!   * Rename storms on high-fanout directories
//!
//! When the watcher misses an event, `IndexedFile` drifts from the
//! filesystem and IST queries return stale truth until next service
//! restart. The periodic sweep re-walks the watch root, recomputes a
//! stable content hash, and pushes deltas (paths missing from
//! `IndexedFile` OR with a mismatched hash) back into the ingress
//! buffer as low-priority subtree hints so the standard A1 pipeline
//! picks them up.
//!
//! Tests below isolate one tick of the worker body via
//! [`crate::pipeline_v2_runtime::periodic_sweep_tick_for_tests`] so we
//! don't need a real `tokio::time::interval` to run.
//!
//! Acceptance criteria covered :
//!   1. `AXON_PERIODIC_SWEEP_HOURS=0` disables the worker entirely.
//!   2. Delta detected for an `IndexedFile` row missing from PG.
//!   3. High CPU load skips the tick instead of running.
//!   4. Telemetry counters surface via
//!      `crate::ingress_buffer::periodic_sweep_metrics_snapshot()` and
//!      drive the JSON block returned by
//!      `crate::mcp::tools_system::axon_embedding_status`.

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::{Mutex, OnceLock};

    use crate::ingress_buffer::{
        periodic_sweep_metrics_snapshot, reset_periodic_sweep_metrics_for_tests, IngressBuffer,
    };
    use crate::pipeline_v2_runtime::{
        periodic_sweep_tick_for_tests, PeriodicSweepConfig, PeriodicSweepTickOutcome,
    };
    use crate::tests::test_helpers::unique_test_scope;

    /// Serialize tests : `record_periodic_sweep_tick` writes process-global
    /// atomics ; running parallel tests would smear the snapshot.
    fn sweep_metrics_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Create a temp watch root containing N source files.
    fn make_temp_watch_root(label: &str, file_count: usize) -> (std::path::PathBuf, Vec<String>) {
        let scope = unique_test_scope(label);
        let root = std::env::temp_dir().join(format!("periodic-sweep-{scope}"));
        std::fs::create_dir_all(&root).expect("create watch root");
        let mut files = Vec::with_capacity(file_count);
        for idx in 0..file_count {
            let path = root.join(format!("file_{idx}.rs"));
            std::fs::write(
                &path,
                format!("// REQ-AXO-901677 fixture {idx}\nfn main() {{}}\n"),
            )
            .expect("write fixture file");
            files.push(path.to_string_lossy().to_string());
        }
        (root, files)
    }

    /// REQ-AXO-901677 acceptance #1 — `AXON_PERIODIC_SWEEP_HOURS=0`
    /// short-circuits the config so the worker is never spawned.
    #[test]
    fn periodic_sweep_config_zero_hours_disables_worker() {
        let _g = sweep_metrics_lock();
        let prev = std::env::var("AXON_PERIODIC_SWEEP_HOURS").ok();
        std::env::set_var("AXON_PERIODIC_SWEEP_HOURS", "0");
        let cfg = PeriodicSweepConfig::from_env();
        assert!(
            !cfg.is_enabled(),
            "hours=0 must disable the worker (cfg={cfg:?})"
        );
        match prev {
            Some(v) => std::env::set_var("AXON_PERIODIC_SWEEP_HOURS", v),
            None => std::env::remove_var("AXON_PERIODIC_SWEEP_HOURS"),
        }
    }

    /// REQ-AXO-901677 acceptance #1 — default cadence = 4h with default
    /// CPU threshold = 50%. Locks the defaults so a silent regression on
    /// the constants is caught here instead of by an operator hours later.
    #[test]
    fn periodic_sweep_config_defaults_are_4h_and_50pct() {
        let _g = sweep_metrics_lock();
        let prev_h = std::env::var("AXON_PERIODIC_SWEEP_HOURS").ok();
        let prev_c = std::env::var("AXON_PERIODIC_SWEEP_CPU_THRESHOLD_PCT").ok();
        std::env::remove_var("AXON_PERIODIC_SWEEP_HOURS");
        std::env::remove_var("AXON_PERIODIC_SWEEP_CPU_THRESHOLD_PCT");
        let cfg = PeriodicSweepConfig::from_env();
        assert_eq!(cfg.hours, 4);
        assert_eq!(cfg.cpu_threshold_pct, 50);
        assert!(cfg.is_enabled());
        match prev_h {
            Some(v) => std::env::set_var("AXON_PERIODIC_SWEEP_HOURS", v),
            None => std::env::remove_var("AXON_PERIODIC_SWEEP_HOURS"),
        }
        match prev_c {
            Some(v) => std::env::set_var("AXON_PERIODIC_SWEEP_CPU_THRESHOLD_PCT", v),
            None => std::env::remove_var("AXON_PERIODIC_SWEEP_CPU_THRESHOLD_PCT"),
        }
    }

    /// REQ-AXO-901677 acceptance #4 — when filesystem holds files that
    /// the (empty) `IndexedFile` cache doesn't know about, the sweep
    /// enqueues one subtree hint per missing file and the snapshot
    /// reflects the work done.
    #[tokio::test(flavor = "current_thread")]
    async fn periodic_sweep_tick_detects_filesystem_delta_and_records_metrics() {
        let _g = sweep_metrics_lock();
        reset_periodic_sweep_metrics_for_tests();

        let (root, files) = make_temp_watch_root("delta", 3);
        let buffer = Arc::new(std::sync::Mutex::new(IngressBuffer::default()));
        let cfg = PeriodicSweepConfig {
            hours: 1,
            cpu_threshold_pct: 100,
        };

        // Empty known-set : every file on disk is a delta.
        let outcome = periodic_sweep_tick_for_tests(
            &buffer,
            root.to_string_lossy().as_ref(),
            &cfg,
            std::collections::HashSet::new(),
            /* cpu_override = */ Some(true),
        );

        assert!(
            matches!(outcome, PeriodicSweepTickOutcome::Ran { .. }),
            "tick must run when CPU check is forced ok (outcome={outcome:?})"
        );
        if let PeriodicSweepTickOutcome::Ran {
            files_compared,
            deltas_found,
            duration_ms: _,
        } = outcome
        {
            assert_eq!(files_compared, files.len() as u64);
            assert_eq!(
                deltas_found,
                files.len() as u64,
                "every file on disk is a delta against the empty known-set"
            );
        }

        // Each delta turned into a subtree hint.
        let guard = buffer.lock().unwrap_or_else(|p| p.into_inner());
        let snap = guard.metrics_snapshot();
        // Subtree hints are tracked via `subtree_hints` (current set size).
        assert!(
            snap.subtree_hints >= 1,
            "deltas must produce at least one subtree hint (snap.subtree_hints={})",
            snap.subtree_hints
        );

        let metrics = periodic_sweep_metrics_snapshot();
        assert!(
            metrics.last_run_at_ms > 0,
            "last_run_at_ms must be set after a sweep (got={})",
            metrics.last_run_at_ms
        );
        assert_eq!(metrics.last_files_compared, files.len() as u64);
        assert_eq!(metrics.last_deltas_found, files.len() as u64);
        assert!(metrics.last_duration_ms < 10_000, "fast for 3 files");
        assert_eq!(metrics.skipped_high_cpu_total, 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// REQ-AXO-901677 acceptance #3 — when CPU is above threshold the
    /// tick MUST be a no-op (no enumeration, no hints, no duration
    /// recorded). The counter `skipped_high_cpu_total` increments so the
    /// operator can spot a starved sweep.
    #[tokio::test(flavor = "current_thread")]
    async fn periodic_sweep_tick_skips_when_cpu_above_threshold() {
        let _g = sweep_metrics_lock();
        reset_periodic_sweep_metrics_for_tests();

        let (root, _files) = make_temp_watch_root("cpuskip", 2);
        let buffer = Arc::new(std::sync::Mutex::new(IngressBuffer::default()));
        let cfg = PeriodicSweepConfig {
            hours: 1,
            cpu_threshold_pct: 50,
        };

        let outcome = periodic_sweep_tick_for_tests(
            &buffer,
            root.to_string_lossy().as_ref(),
            &cfg,
            std::collections::HashSet::new(),
            /* cpu_override = */ Some(false),
        );
        assert!(
            matches!(outcome, PeriodicSweepTickOutcome::SkippedHighCpu),
            "tick must skip under CPU pressure (outcome={outcome:?})"
        );

        let metrics = periodic_sweep_metrics_snapshot();
        assert_eq!(metrics.skipped_high_cpu_total, 1);
        assert_eq!(
            metrics.last_files_compared, 0,
            "skip path must not enumerate files"
        );
        assert_eq!(metrics.last_deltas_found, 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// REQ-AXO-901677 acceptance #3 — when the known-set already covers
    /// every file on disk, the sweep runs but reports zero deltas. This
    /// is the steady-state quiet path : worker confirms IST is current
    /// without producing any A1 work.
    #[tokio::test(flavor = "current_thread")]
    async fn periodic_sweep_tick_reports_zero_deltas_when_known_set_matches() {
        let _g = sweep_metrics_lock();
        reset_periodic_sweep_metrics_for_tests();

        let (root, files) = make_temp_watch_root("steady", 2);
        let buffer = Arc::new(std::sync::Mutex::new(IngressBuffer::default()));
        let cfg = PeriodicSweepConfig {
            hours: 1,
            cpu_threshold_pct: 100,
        };
        let known: std::collections::HashSet<String> = files.iter().cloned().collect();

        let outcome = periodic_sweep_tick_for_tests(
            &buffer,
            root.to_string_lossy().as_ref(),
            &cfg,
            known,
            /* cpu_override = */ Some(true),
        );

        if let PeriodicSweepTickOutcome::Ran {
            files_compared,
            deltas_found,
            duration_ms: _,
        } = outcome
        {
            assert_eq!(files_compared, files.len() as u64);
            assert_eq!(deltas_found, 0, "no deltas when known-set matches disk");
        } else {
            panic!("expected Ran outcome (got {outcome:?})");
        }

        let guard = buffer.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(
            guard.metrics_snapshot().subtree_hints,
            0,
            "quiet path must not enqueue subtree hints"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
