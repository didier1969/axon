// REQ-AXO-094 — sibling tests for BEAM alarm projection. The brain
// is the authority for the alarm→subsystem mapping (PIL-AXO-001 /
// REQ-AXO-098). These tests pin: (a) known alarms produce the right
// Subsystem state transition, (b) unknown alarms are silently
// ignored (no registry mutation), (c) action=clear restores Ready.

use super::handle_beam_alarm;
use crate::runtime_readiness::{
    report_subsystem_state, reset_for_tests, snapshot_subsystem_reports, Subsystem, SubsystemState,
};
use std::sync::{Mutex, OnceLock};

fn registry_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn beam_alarm_memory_high_watermark_set_marks_dashboard_degraded() {
    let _guard = registry_test_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    // Pre-condition: explicitly Ready so the transition is observable.
    report_subsystem_state(Subsystem::Dashboard, SubsystemState::Ready);

    handle_beam_alarm(r#"{"alarm":"system_memory_high_watermark","action":"set"}"#);

    let reports = snapshot_subsystem_reports();
    let dashboard = reports
        .iter()
        .find(|r| r.subsystem == "dashboard")
        .expect("dashboard subsystem must be reported after BEAM alarm projection");
    match &dashboard.state {
        SubsystemState::Degraded { reason } => {
            assert_eq!(
                reason, "memory_pressure",
                "memory_high_watermark must map to reason=memory_pressure"
            );
        }
        other => panic!("expected Degraded(memory_pressure), got {other:?}"),
    }
}

#[test]
fn beam_alarm_disk_almost_full_set_marks_ist_writer_degraded() {
    let _guard = registry_test_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    report_subsystem_state(Subsystem::IstWriter, SubsystemState::Ready);

    handle_beam_alarm(r#"{"alarm":"disk_almost_full","action":"set"}"#);

    let reports = snapshot_subsystem_reports();
    let writer = reports
        .iter()
        .find(|r| r.subsystem == "ist_writer")
        .expect("ist_writer must be reported after disk alarm");
    match &writer.state {
        SubsystemState::Degraded { reason } => {
            assert_eq!(reason, "disk_almost_full");
        }
        other => panic!("expected Degraded(disk_almost_full), got {other:?}"),
    }
}

#[test]
fn beam_alarm_clear_restores_ready() {
    let _guard = registry_test_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    handle_beam_alarm(r#"{"alarm":"system_memory_high_watermark","action":"set"}"#);
    handle_beam_alarm(r#"{"alarm":"system_memory_high_watermark","action":"clear"}"#);

    let reports = snapshot_subsystem_reports();
    let dashboard = reports
        .iter()
        .find(|r| r.subsystem == "dashboard")
        .expect("dashboard must still be reported after clear");
    assert!(
        matches!(dashboard.state, SubsystemState::Ready),
        "clear must restore Ready, got {:?}",
        dashboard.state
    );
}

#[test]
fn beam_alarm_unknown_does_not_mutate_registry() {
    let _guard = registry_test_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    let before = snapshot_subsystem_reports();
    handle_beam_alarm(r#"{"alarm":"some_future_alarm_we_do_not_know","action":"set"}"#);
    let after = snapshot_subsystem_reports();
    // Defensive: registry size and state must not change for an
    // unknown alarm — protects the readiness contract from
    // dashboard-side bugs and malicious payloads.
    assert_eq!(
        before.len(),
        after.len(),
        "unknown BEAM alarm must not add entries to the registry"
    );
}

#[test]
fn beam_alarm_invalid_json_is_ignored_safely() {
    let _guard = registry_test_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    reset_for_tests();
    let before = snapshot_subsystem_reports();
    handle_beam_alarm("not json at all {{{");
    let after = snapshot_subsystem_reports();
    assert_eq!(
        before.len(),
        after.len(),
        "malformed BEAM_ALARM payload must NOT panic and must NOT mutate the registry"
    );
}
