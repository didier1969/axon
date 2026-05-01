//! REQ-AXO-098 / DEC-AXO-062 / CPT-AXO-023 — Subsystem-tagged tristate
//! runtime readiness contract.
//!
//! Each subsystem (brain_mcp, ist_writer, ist_reader, dashboard,
//! embedder, watcher) reports its own state independently into a
//! thread-safe registry; the overall runtime readiness is computed as
//! a roll-up. Failed dominates Degraded; Degraded dominates Ready.
//!
//! This is the prerequisite contract for REQ-AXO-097 (watchdog) and
//! REQ-AXO-094 (BEAM alarm classification): a single global "healthy"
//! flag does not let a watchdog know which role to restart, and does
//! not let an alarm classifier project an alarm onto a specific
//! subsystem state.
//!
//! Reporter pattern: a subsystem registers itself implicitly by
//! calling `report_subsystem_state(name, state)`; the registry
//! materializes the reporter on first call and updates its
//! `last_observed_at_ms`. The status path reads a snapshot atomically
//! — no polling needed.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Canonical subsystem identifiers. Future subsystems are added here
/// explicitly so the contract is stable across releases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Subsystem {
    BrainMcp,
    IstWriter,
    IstReader,
    Dashboard,
    Embedder,
    Watcher,
}

impl Subsystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Subsystem::BrainMcp => "brain_mcp",
            Subsystem::IstWriter => "ist_writer",
            Subsystem::IstReader => "ist_reader",
            Subsystem::Dashboard => "dashboard",
            Subsystem::Embedder => "embedder",
            Subsystem::Watcher => "watcher",
        }
    }
}

/// Tristate per-subsystem state. `Degraded` means the subsystem is
/// responding but at reduced capacity (e.g. embedder on CPU instead
/// of GPU, IST reader lagging). `Failed` means non-functional.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubsystemState {
    Ready,
    Degraded { reason: String },
    Failed { reason: String },
}

impl SubsystemState {
    pub fn label(&self) -> &'static str {
        match self {
            SubsystemState::Ready => "ready",
            SubsystemState::Degraded { .. } => "degraded",
            SubsystemState::Failed { .. } => "failed",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            SubsystemState::Ready => None,
            SubsystemState::Degraded { reason } | SubsystemState::Failed { reason } => {
                Some(reason)
            }
        }
    }
}

/// Per-subsystem report exposed to status callers.
#[derive(Clone, Debug, Serialize)]
pub struct SubsystemReport {
    pub subsystem: String,
    #[serde(flatten)]
    pub state: SubsystemState,
    pub last_observed_at_ms: u64,
}

/// Rolled-up runtime readiness. `reasons` aggregates per-subsystem
/// reasons in subsystem-prefixed form (e.g. "embedder: model_load_failed")
/// so an LLM client can act on the right component.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeReadiness {
    Ready,
    Degraded { reasons: Vec<String> },
    Failed { reasons: Vec<String> },
}

impl RuntimeReadiness {
    pub fn label(&self) -> &'static str {
        match self {
            RuntimeReadiness::Ready => "ready",
            RuntimeReadiness::Degraded { .. } => "degraded",
            RuntimeReadiness::Failed { .. } => "failed",
        }
    }

    /// Failed dominates Degraded; Degraded dominates Ready. If any
    /// subsystem is Failed, overall is Failed with all Failed reasons.
    /// Otherwise if any is Degraded, overall is Degraded with all
    /// Degraded reasons. Otherwise Ready. Empty input also collapses
    /// to Ready (no subsystem reported is interpreted as "no signal of
    /// trouble" — conservative for a fresh boot).
    pub fn roll_up(reports: &[SubsystemReport]) -> Self {
        let mut failed_reasons = Vec::new();
        let mut degraded_reasons = Vec::new();
        for report in reports {
            match &report.state {
                SubsystemState::Failed { reason } => {
                    failed_reasons.push(format!("{}: {}", report.subsystem, reason));
                }
                SubsystemState::Degraded { reason } => {
                    degraded_reasons.push(format!("{}: {}", report.subsystem, reason));
                }
                SubsystemState::Ready => {}
            }
        }
        if !failed_reasons.is_empty() {
            RuntimeReadiness::Failed {
                reasons: failed_reasons,
            }
        } else if !degraded_reasons.is_empty() {
            RuntimeReadiness::Degraded {
                reasons: degraded_reasons,
            }
        } else {
            RuntimeReadiness::Ready
        }
    }
}

/// Internal registry slot for a single subsystem.
#[derive(Clone, Debug)]
struct ReporterSlot {
    state: SubsystemState,
    last_observed_at_ms: u64,
}

fn registry() -> &'static Mutex<HashMap<&'static str, ReporterSlot>> {
    static REGISTRY: OnceLock<Mutex<HashMap<&'static str, ReporterSlot>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Report a subsystem's current state. Replaces any prior state for
/// that subsystem and bumps `last_observed_at_ms`. Calling repeatedly
/// with the same state is allowed and acts as a heartbeat (the
/// timestamp updates).
pub fn report_subsystem_state(subsystem: Subsystem, state: SubsystemState) {
    let mut guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    guard.insert(
        subsystem.as_str(),
        ReporterSlot {
            state,
            last_observed_at_ms: now_ms(),
        },
    );
}

/// Snapshot the registry as a sorted Vec<SubsystemReport>. Sort order
/// is the canonical Subsystem enum order so the output is stable
/// across calls.
pub fn snapshot_subsystem_reports() -> Vec<SubsystemReport> {
    let guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    const CANONICAL: &[Subsystem] = &[
        Subsystem::BrainMcp,
        Subsystem::IstWriter,
        Subsystem::IstReader,
        Subsystem::Dashboard,
        Subsystem::Embedder,
        Subsystem::Watcher,
    ];
    CANONICAL
        .iter()
        .filter_map(|subsystem| {
            guard.get(subsystem.as_str()).map(|slot| SubsystemReport {
                subsystem: subsystem.as_str().to_string(),
                state: slot.state.clone(),
                last_observed_at_ms: slot.last_observed_at_ms,
            })
        })
        .collect()
}

/// Convenience that snapshots the registry and rolls up overall
/// readiness in one atomic-ish operation.
pub fn snapshot_runtime_readiness() -> (RuntimeReadiness, Vec<SubsystemReport>) {
    let reports = snapshot_subsystem_reports();
    let readiness = RuntimeReadiness::roll_up(&reports);
    (readiness, reports)
}

/// Test-only reset hook so each test starts from a clean registry.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let mut guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    guard.clear();
}

#[cfg(test)]
#[path = "runtime_readiness_tests.rs"]
mod runtime_readiness_tests;
