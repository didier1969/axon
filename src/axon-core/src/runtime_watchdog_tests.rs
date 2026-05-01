// REQ-AXO-097 — sibling tests for the runtime watchdog tokio task
// and the heartbeater. The registry is a process-global singleton;
// these tests acquire a shared mutex with the runtime_readiness
// tests to prevent interleaving on the same registry.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::{spawn_heartbeat_task, DEFAULT_HEARTBEAT_PERIOD_MS, DEFAULT_TICK_INTERVAL_MS};
use crate::runtime_readiness::{
    report_subsystem_state, require_heartbeat, reset_for_tests, set_last_observed_for_tests,
    snapshot_subsystem_reports, tick_watchdog, Subsystem, SubsystemState,
    HEARTBEAT_STALENESS_MULTIPLIER,
};

fn registry_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn watchdog_defaults_are_internally_consistent() {
    // The default tick must be ≤ the default heartbeat period so the
    // watchdog cannot miss a staleness window between two ticks.
    assert!(
        DEFAULT_TICK_INTERVAL_MS <= DEFAULT_HEARTBEAT_PERIOD_MS,
        "tick interval ({DEFAULT_TICK_INTERVAL_MS}) must be ≤ heartbeat period ({DEFAULT_HEARTBEAT_PERIOD_MS})"
    );
    // The staleness multiplier must be > 1 — otherwise a single
    // missed tick due to GC pause flips the subsystem.
    assert!(
        HEARTBEAT_STALENESS_MULTIPLIER >= 2,
        "staleness multiplier must be ≥ 2 to absorb a single missed heartbeat"
    );
}

#[tokio::test]
async fn heartbeat_task_resets_staleness_clock_within_two_periods() {
    let _guard = registry_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    let period_ms: u64 = 200;
    require_heartbeat(Subsystem::BrainMcp, period_ms);
    // Pre-stale the slot so the next heartbeat is the test signal.
    report_subsystem_state(Subsystem::BrainMcp, SubsystemState::Ready);
    set_last_observed_for_tests(Subsystem::BrainMcp, 0);

    let handle = spawn_heartbeat_task(Subsystem::BrainMcp, period_ms);
    // Wait long enough for at least 2 heartbeats to fire.
    tokio::time::sleep(Duration::from_millis(period_ms * 3)).await;
    handle.abort();

    let reports = snapshot_subsystem_reports();
    let brain = reports.iter().find(|r| r.subsystem == "brain_mcp").unwrap();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let elapsed_since_last_observed = now_ms.saturating_sub(brain.last_observed_at_ms);
    assert!(
        elapsed_since_last_observed < period_ms * 2,
        "heartbeat task must have reset the staleness clock; elapsed={elapsed_since_last_observed}ms, period={period_ms}ms"
    );
    assert!(matches!(brain.state, SubsystemState::Ready));
}

#[tokio::test]
async fn watchdog_observes_dead_heartbeat_after_threshold() {
    let _guard = registry_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    let period_ms: u64 = 100;
    require_heartbeat(Subsystem::Embedder, period_ms);
    report_subsystem_state(Subsystem::Embedder, SubsystemState::Ready);

    // Spawn a heartbeat task and immediately abort it to simulate a
    // dead/panicked subsystem. The registry retains the last
    // last_observed_at_ms; from that moment the staleness clock
    // ticks against the watchdog.
    let handle = spawn_heartbeat_task(Subsystem::Embedder, period_ms);
    handle.abort();

    // Wait for one full staleness window plus margin, then tick.
    let threshold_ms = period_ms * HEARTBEAT_STALENESS_MULTIPLIER;
    tokio::time::sleep(Duration::from_millis(threshold_ms + period_ms)).await;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let transitions = tick_watchdog(now_ms);
    assert_eq!(
        transitions.len(),
        1,
        "watchdog must observe the dead heartbeat as one transition, got {transitions:?}"
    );
    assert_eq!(transitions[0].0, "embedder");
    assert!(matches!(
        transitions[0].1,
        SubsystemState::Failed { .. }
    ));
}
