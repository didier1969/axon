//! REQ-AXO-097 — runtime watchdog loop.
//!
//! Builds on the readiness primitive in `runtime_readiness`. A
//! tokio task spawned at boot calls `tick_watchdog(now_ms)` every
//! `tick_interval_ms`; any subsystem opted into heartbeating via
//! `require_heartbeat` whose `last_observed_at_ms` exceeds
//! `period_ms * HEARTBEAT_STALENESS_MULTIPLIER` is flipped to
//! `Failed { reason: "no_telemetry_window_exceeded ..." }` and
//! emitted as a structured log event.
//!
//! Heartbeats are how a subsystem tells the watchdog "I am still
//! alive". For tokio-native subsystems, a periodic
//! `tokio::time::interval` calling `report_subsystem_state(self,
//! Ready)` is the canonical pattern. For sync threads with a recv
//! loop, calling `report_subsystem_state` on each iteration of the
//! loop is sufficient.
//!
//! ## What this module does NOT do
//!
//! - It does NOT restart role processes. The watchdog detects death
//!   in-process (a tokio task panic, a stuck thread). Cross-process
//!   restart belongs to the supervisor (axonctl), which polls
//!   `mcp__axon__status` (or the equivalent CLI) and acts on
//!   `data.subsystems[]` Failed entries.
//! - It does NOT issue heartbeats itself. Heartbeats are the
//!   responsibility of each subsystem; the watchdog only checks
//!   freshness.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::runtime_readiness::{tick_watchdog, SubsystemState};

/// Default watchdog tick cadence. Conservative enough that the
/// watchdog itself never becomes a hot path, fast enough that a
/// dead subsystem is observable within ~5s after the staleness
/// window closes.
pub const DEFAULT_TICK_INTERVAL_MS: u64 = 5_000;

/// Default heartbeat cadence for top-level role subsystems. With
/// `HEARTBEAT_STALENESS_MULTIPLIER = 3` this gives a 15s death
/// detection window, balancing responsiveness against false
/// positives during GC pauses or short stalls.
pub const DEFAULT_HEARTBEAT_PERIOD_MS: u64 = 5_000;

/// Idempotency guard: only spawn one watchdog task per process.
static WATCHDOG_SPAWNED: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Spawn the watchdog tokio task. Idempotent: subsequent calls in
/// the same process are a no-op so test harnesses and re-init code
/// paths cannot accidentally start two watchdogs racing on the
/// registry.
///
/// The returned `JoinHandle` is intentionally detached by the
/// caller; the watchdog runs for the lifetime of the runtime.
pub fn spawn_watchdog_task(tick_interval_ms: u64) -> Option<tokio::task::JoinHandle<()>> {
    if WATCHDOG_SPAWNED.swap(true, Ordering::SeqCst) {
        tracing::debug!("runtime watchdog already spawned; skipping");
        return None;
    }
    let interval = Duration::from_millis(tick_interval_ms.max(100));
    tracing::info!(
        target = "axon::runtime_watchdog",
        tick_interval_ms = tick_interval_ms,
        "REQ-AXO-097 runtime watchdog spawned"
    );
    let handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let transitions = tick_watchdog(now_ms());
            for (subsystem, state) in transitions {
                emit_transition_event(&subsystem, &state);
            }
        }
    });
    Some(handle)
}

/// Spawn a heartbeater for a single subsystem. Reports
/// `SubsystemState::Ready` for `subsystem` every `period_ms` ms
/// while the tokio runtime is alive. The heartbeater dying (e.g.
/// because the runtime panics) is exactly the death signal the
/// watchdog is meant to observe.
///
/// The `require_heartbeat` opt-in must be called separately (see
/// `wire_brain_role_heartbeats`) so the watchdog knows to flip the
/// subsystem on staleness.
pub fn spawn_heartbeat_task(
    subsystem: crate::runtime_readiness::Subsystem,
    period_ms: u64,
) -> tokio::task::JoinHandle<()> {
    let interval = Duration::from_millis(period_ms.max(100));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            crate::runtime_readiness::report_subsystem_state(
                subsystem,
                crate::runtime_readiness::SubsystemState::Ready,
            );
        }
    })
}

/// Wire watchdog supervision for the brain role: opt brain_mcp and
/// ist_reader into heartbeating, spawn their heartbeaters, then
/// spawn the watchdog tick. Called from `runtime_boot::boot` after
/// the initial subsystem reports.
pub fn wire_brain_role_heartbeats() {
    crate::runtime_readiness::require_heartbeat(
        crate::runtime_readiness::Subsystem::BrainMcp,
        DEFAULT_HEARTBEAT_PERIOD_MS,
    );
    crate::runtime_readiness::require_heartbeat(
        crate::runtime_readiness::Subsystem::IstReader,
        DEFAULT_HEARTBEAT_PERIOD_MS,
    );
    spawn_heartbeat_task(
        crate::runtime_readiness::Subsystem::BrainMcp,
        DEFAULT_HEARTBEAT_PERIOD_MS,
    );
    spawn_heartbeat_task(
        crate::runtime_readiness::Subsystem::IstReader,
        DEFAULT_HEARTBEAT_PERIOD_MS,
    );
}

/// Wire watchdog supervision for the indexer role: opt ist_writer
/// and watcher into heartbeating, spawn their heartbeaters.
pub fn wire_indexer_role_heartbeats() {
    crate::runtime_readiness::require_heartbeat(
        crate::runtime_readiness::Subsystem::IstWriter,
        DEFAULT_HEARTBEAT_PERIOD_MS,
    );
    crate::runtime_readiness::require_heartbeat(
        crate::runtime_readiness::Subsystem::Watcher,
        DEFAULT_HEARTBEAT_PERIOD_MS,
    );
    spawn_heartbeat_task(
        crate::runtime_readiness::Subsystem::IstWriter,
        DEFAULT_HEARTBEAT_PERIOD_MS,
    );
    spawn_heartbeat_task(
        crate::runtime_readiness::Subsystem::Watcher,
        DEFAULT_HEARTBEAT_PERIOD_MS,
    );
}

fn emit_transition_event(subsystem: &str, state: &SubsystemState) {
    match state {
        SubsystemState::Failed { reason } => {
            tracing::warn!(
                target = "axon::runtime_watchdog",
                event = "subsystem_failed_via_watchdog",
                subsystem = subsystem,
                reason = reason.as_str(),
                "REQ-AXO-097: watchdog flipped {subsystem} to Failed (reason: {reason}). \
                 Cross-process restart is the supervisor's (axonctl) responsibility."
            );
        }
        SubsystemState::Degraded { reason } => {
            tracing::warn!(
                target = "axon::runtime_watchdog",
                event = "subsystem_degraded_via_watchdog",
                subsystem = subsystem,
                reason = reason.as_str()
            );
        }
        SubsystemState::Ready => {
            tracing::info!(
                target = "axon::runtime_watchdog",
                event = "subsystem_recovered_via_watchdog",
                subsystem = subsystem
            );
        }
    }
}

#[cfg(test)]
#[path = "runtime_watchdog_tests.rs"]
mod runtime_watchdog_tests;
