// REQ-AXO-098 — sibling tests for the subsystem-tagged tristate
// runtime readiness contract.
//
// The registry is a process-global singleton. Tests in the same binary
// run in parallel by default, so registry-touching tests acquire a
// shared mutex to serialize against each other. Roll-up tests do not
// touch the global registry and stay fully parallel.

use std::sync::{Mutex, OnceLock};

use super::{
    report_subsystem_state, reset_for_tests, snapshot_runtime_readiness,
    snapshot_subsystem_reports, RuntimeReadiness, Subsystem, SubsystemReport, SubsystemState,
};

fn registry_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn report(subsystem: &str, state: SubsystemState) -> SubsystemReport {
    SubsystemReport {
        subsystem: subsystem.to_string(),
        state,
        last_observed_at_ms: 0,
    }
}

#[test]
fn empty_registry_rolls_up_to_ready() {
    // No subsystem reported yet → conservative Ready (no signal of
    // trouble). This is the fresh-boot state before any reporter has
    // fired. The roll-up must not invent failures the registry has
    // not observed.
    assert!(matches!(
        RuntimeReadiness::roll_up(&[]),
        RuntimeReadiness::Ready
    ));
}

#[test]
fn all_ready_subsystems_roll_up_to_ready() {
    let reports = vec![
        report("brain_mcp", SubsystemState::Ready),
        report("ist_reader", SubsystemState::Ready),
        report("embedder", SubsystemState::Ready),
    ];
    assert!(matches!(
        RuntimeReadiness::roll_up(&reports),
        RuntimeReadiness::Ready
    ));
}

#[test]
fn any_degraded_with_no_failed_rolls_up_to_degraded() {
    let reports = vec![
        report("brain_mcp", SubsystemState::Ready),
        report(
            "embedder",
            SubsystemState::Degraded {
                reason: "model_load_warn".to_string(),
            },
        ),
        report("ist_reader", SubsystemState::Ready),
    ];
    match RuntimeReadiness::roll_up(&reports) {
        RuntimeReadiness::Degraded { reasons } => {
            assert_eq!(reasons.len(), 1);
            assert!(
                reasons[0].starts_with("embedder:"),
                "reason must be subsystem-prefixed: {}",
                reasons[0]
            );
            assert!(reasons[0].contains("model_load_warn"));
        }
        other => panic!("expected Degraded, got {other:?}"),
    }
}

#[test]
fn any_failed_dominates_degraded_in_roll_up() {
    let reports = vec![
        report(
            "embedder",
            SubsystemState::Degraded {
                reason: "cpu_only".to_string(),
            },
        ),
        report(
            "brain_mcp",
            SubsystemState::Failed {
                reason: "port_not_bound".to_string(),
            },
        ),
        report(
            "dashboard",
            SubsystemState::Degraded {
                reason: "sql_econnrefused".to_string(),
            },
        ),
    ];
    match RuntimeReadiness::roll_up(&reports) {
        RuntimeReadiness::Failed { reasons } => {
            assert_eq!(
                reasons.len(),
                1,
                "Failed dominates: only Failed reasons appear in the rollup"
            );
            assert!(reasons[0].contains("brain_mcp"));
            assert!(reasons[0].contains("port_not_bound"));
        }
        other => panic!("expected Failed (Failed must dominate Degraded), got {other:?}"),
    }
}

#[test]
fn registry_report_and_snapshot_round_trip() {
    let _guard = registry_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    report_subsystem_state(Subsystem::BrainMcp, SubsystemState::Ready);
    report_subsystem_state(
        Subsystem::Embedder,
        SubsystemState::Degraded {
            reason: "cpu_fallback".to_string(),
        },
    );
    let reports = snapshot_subsystem_reports();
    assert_eq!(reports.len(), 2);
    let brain = reports.iter().find(|r| r.subsystem == "brain_mcp").unwrap();
    assert!(matches!(brain.state, SubsystemState::Ready));
    let embedder = reports.iter().find(|r| r.subsystem == "embedder").unwrap();
    assert!(matches!(
        embedder.state,
        SubsystemState::Degraded { ref reason } if reason == "cpu_fallback"
    ));
}

#[test]
fn registry_replaces_state_on_repeated_report() {
    let _guard = registry_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    report_subsystem_state(
        Subsystem::IstReader,
        SubsystemState::Failed {
            reason: "db_unavailable".to_string(),
        },
    );
    report_subsystem_state(Subsystem::IstReader, SubsystemState::Ready);
    let reports = snapshot_subsystem_reports();
    let entry = reports.iter().find(|r| r.subsystem == "ist_reader").unwrap();
    assert!(
        matches!(entry.state, SubsystemState::Ready),
        "later report must replace earlier state, not append"
    );
}

#[test]
fn snapshot_order_is_canonical_across_reporting_order() {
    let _guard = registry_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    report_subsystem_state(Subsystem::Watcher, SubsystemState::Ready);
    report_subsystem_state(Subsystem::BrainMcp, SubsystemState::Ready);
    report_subsystem_state(Subsystem::Embedder, SubsystemState::Ready);
    let reports = snapshot_subsystem_reports();
    let names: Vec<&str> = reports.iter().map(|r| r.subsystem.as_str()).collect();
    // Canonical order regardless of reporting order:
    // brain_mcp before ist_writer before ist_reader before dashboard
    // before embedder before watcher.
    let brain_idx = names.iter().position(|s| *s == "brain_mcp").unwrap();
    let embed_idx = names.iter().position(|s| *s == "embedder").unwrap();
    let watch_idx = names.iter().position(|s| *s == "watcher").unwrap();
    assert!(
        brain_idx < embed_idx && embed_idx < watch_idx,
        "snapshot must be in canonical order, got {names:?}"
    );
}

#[test]
fn snapshot_runtime_readiness_combines_snapshot_and_roll_up_atomically() {
    let _guard = registry_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    report_subsystem_state(Subsystem::BrainMcp, SubsystemState::Ready);
    report_subsystem_state(
        Subsystem::Dashboard,
        SubsystemState::Degraded {
            reason: "sql_econnrefused".to_string(),
        },
    );
    let (readiness, reports) = snapshot_runtime_readiness();
    assert_eq!(reports.len(), 2);
    assert!(matches!(readiness, RuntimeReadiness::Degraded { .. }));
}
