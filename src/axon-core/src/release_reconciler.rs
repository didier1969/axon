//! REQ-AXO-902111 / DEC-AXO-901662 — declarative control-plane reconciler.
//!
//! T1 read-only slice: collect release-lifecycle facts → evaluate gates (typed
//! Rust predicates; Ascent/Datalog migration is T2) → derive `phase` +
//! `next_action`. The bash promote scripts still ACT; this surfaces the truth they
//! act on so an LLM (or operator) reads `{phase, failed_gates, next_action}`
//! instead of grepping 700 lines of shell. The two failures of session 91 — a
//! manifest/runtime drift after a killed promote, and a stranded `pending.json` —
//! both become a one-line derived verdict here.
//!
//! Scope of T1: the *release* state machine (manifest ↔ running build_id ↔ pending
//! staging). Runtime liveness gates (brain/indexer health) join in a later slice
//! once the in-process health source is wired (the `status` tool already owns it).

use std::path::Path;

use serde_json::Value;

/// Facts about the live release, collected from the on-disk manifests + the
/// running process's own build identity. All reads are cheap and side-effect-free.
#[derive(Debug, Clone, Default)]
pub struct ReleaseFacts {
    /// `AXON_BUILD_ID` of the process serving this call (the running brain).
    pub live_build_id: String,
    /// `runtime_version.build_id` recorded in `current.json` (the promoted truth).
    pub manifest_build_id: Option<String>,
    /// `state` field of `current.json` (e.g. "promoted").
    pub manifest_state: Option<String>,
    /// `qualification.verdict == "ok"` when present.
    pub qualification_ok: Option<bool>,
    /// A `pending.json` exists — a promote is mid-flight OR was stranded by a crash.
    pub pending_present: bool,
    /// `runtime_version.build_id` of `pending.json` when present.
    pub pending_build_id: Option<String>,
    /// `runtime_contract` recorded in `current.json` (e.g. "brain_mcp_indexer_ist").
    /// The presence of "indexer" in it = the live topology runs a SEPARATE indexer
    /// process that must be alive (REQ-AXO-902111 liveness slice). This is the only
    /// declarative source for "is an indexer expected" — the answering brain's own
    /// runtime mode is `brain_only` and would lie.
    pub runtime_contract: Option<String>,
}

fn read_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn extract_build_id(v: &Value) -> Option<String> {
    v.get("runtime_version")
        .and_then(|rv| rv.get("build_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

impl ReleaseFacts {
    /// Collect facts from `release_dir` (`.axon/live-release`) + the process build
    /// identity. `live_build_id` is read from `AXON_BUILD_ID` by the caller so this
    /// stays pure/testable.
    pub fn collect(release_dir: &Path, live_build_id: String) -> Self {
        let current = read_json(&release_dir.join("current.json"));
        let pending = read_json(&release_dir.join("pending.json"));
        let manifest_build_id = current.as_ref().and_then(extract_build_id);
        let manifest_state = current
            .as_ref()
            .and_then(|c| c.get("state"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let qualification_ok = current.as_ref().and_then(|c| {
            c.get("qualification")
                .and_then(|q| q.get("verdict"))
                .and_then(Value::as_str)
                .map(|verdict| verdict.eq_ignore_ascii_case("ok"))
        });
        let runtime_contract = current
            .as_ref()
            .and_then(|c| c.get("runtime_contract"))
            .and_then(Value::as_str)
            .map(str::to_string);
        ReleaseFacts {
            live_build_id,
            manifest_build_id,
            manifest_state,
            qualification_ok,
            pending_present: pending.is_some(),
            pending_build_id: pending.as_ref().and_then(extract_build_id),
            runtime_contract,
        }
    }

    /// The live topology runs a separate indexer process that must be alive.
    /// Derived from `runtime_contract` (never from the answering process's own mode,
    /// which is `brain_only` in the split deployment and would lie).
    pub fn indexer_expected(&self) -> bool {
        self.runtime_contract
            .as_deref()
            .is_some_and(|c| c.contains("indexer"))
    }
}

/// Runtime liveness facts — populated by the tool wrapper (`tools_release.rs`, which
/// holds `&self`/IO) from the SAME in-process sources the `status` tool trusts:
/// `resolve_indexer_liveness(latest_lifecycle_heartbeat("indexer"))` for the indexer
/// and a `SELECT 1` DB probe for the brain. Kept separate from `ReleaseFacts` so the
/// gates stay pure, IO-free predicates (testable without a runtime).
#[derive(Debug, Clone, Default)]
pub struct LivenessFacts {
    /// Brain answered a `SELECT 1` DB probe (process up AND DB reachable).
    pub brain_serving: bool,
    /// The live `runtime_contract` names a separate indexer (must be alive).
    pub indexer_expected: bool,
    /// Indexer heartbeat is fresh (`resolve_indexer_liveness(..).ready`).
    pub indexer_ready: bool,
    /// Lifecycle verdict: "healthy" | "crashed_or_abandoned" | "never_launched".
    pub indexer_lifecycle: String,
    /// Liveness source: "pg_heartbeat" | "pg_heartbeat_stale" | "no_heartbeat".
    pub indexer_source: String,
}

/// Evaluate the runtime liveness gates (pure predicates over `LivenessFacts`).
/// `brain_serving` is universal; `indexer_alive` is conditional on the profile
/// (N/A when the `runtime_contract` has no separate indexer).
pub fn evaluate_liveness_gates(l: &LivenessFacts) -> Vec<Gate> {
    vec![
        Gate {
            name: "brain_serving",
            pass: l.brain_serving,
            detail: if l.brain_serving {
                "brain DB probe SELECT 1 ok".to_string()
            } else {
                "brain not serving (db_probe_failed)".to_string()
            },
        },
        Gate {
            name: "indexer_alive",
            pass: !l.indexer_expected || l.indexer_ready,
            detail: if !l.indexer_expected {
                "no separate indexer in runtime_contract — gate N/A".to_string()
            } else if l.indexer_ready {
                format!("indexer healthy ({})", l.indexer_source)
            } else {
                format!("indexer {} ({})", l.indexer_lifecycle, l.indexer_source)
            },
        },
    ]
}

/// Liveness phase, taking precedence over the release-state phase when red.
pub fn liveness_phase(l: &LivenessFacts) -> Option<&'static str> {
    if !l.brain_serving {
        Some("brain_down")
    } else if l.indexer_expected && !l.indexer_ready {
        Some("indexer_down")
    } else {
        None
    }
}

/// The corrective action for a liveness failure, keyed on the lifecycle verdict so a
/// stale heartbeat (restart) is distinguished from a never-launched indexer (start).
pub fn liveness_next_action(l: &LivenessFacts) -> Option<String> {
    if !l.brain_serving {
        return Some(
            "brain process up but DB probe (SELECT 1) failed — check Postgres reachability, then restart the brain."
                .to_string(),
        );
    }
    if l.indexer_expected && !l.indexer_ready {
        return Some(match l.indexer_lifecycle.as_str() {
            "crashed_or_abandoned" => "indexer heartbeat went stale — restart the indexer (`axonctl` / `promote-live --restart-live`) then re-check.".to_string(),
            "never_launched" => "no indexer heartbeat — the split indexer was never started; start the full runtime (`./scripts/axon-live start full`).".to_string(),
            _ => "indexer not ready — inspect the indexer process and its heartbeat.".to_string(),
        });
    }
    None
}

/// A single declarative gate: a named predicate over the facts with a human detail.
#[derive(Debug, Clone)]
pub struct Gate {
    pub name: &'static str,
    pub pass: bool,
    pub detail: String,
}

/// Evaluate the release gates. These are the T1 predicates; T2 re-expresses them in
/// Ascent without changing their meaning.
pub fn evaluate_gates(f: &ReleaseFacts) -> Vec<Gate> {
    let manifest_match = f.manifest_build_id.as_deref() == Some(f.live_build_id.as_str());
    vec![
        Gate {
            name: "manifest_runtime_match",
            pass: manifest_match,
            detail: format!(
                "running={} manifest={}",
                f.live_build_id,
                f.manifest_build_id.as_deref().unwrap_or("<none>")
            ),
        },
        Gate {
            name: "no_stale_pending",
            pass: !f.pending_present,
            detail: if f.pending_present {
                format!(
                    "pending.json present (build_id={})",
                    f.pending_build_id.as_deref().unwrap_or("<unknown>")
                )
            } else {
                "no pending staging".to_string()
            },
        },
        Gate {
            name: "qualification_passed",
            // Absent qualification is not a failure (older manifests); only an
            // explicit non-ok verdict fails the gate.
            pass: f.qualification_ok != Some(false),
            detail: match f.qualification_ok {
                Some(true) => "qualify verdict=ok".to_string(),
                Some(false) => "qualify verdict=NOT ok".to_string(),
                None => "no qualification recorded".to_string(),
            },
        },
    ]
}

/// Derive the release phase from the facts (the projection of the FSM state).
pub fn phase(f: &ReleaseFacts) -> &'static str {
    if f.pending_present {
        // A staging exists: either a promote is mid-flight or it was stranded.
        "staged"
    } else if f.manifest_build_id.is_none() {
        "uninitialized"
    } else if f.manifest_build_id.as_deref() != Some(f.live_build_id.as_str()) {
        "drift"
    } else {
        "clean"
    }
}

/// The single corrective action that closes the gap, or `None` when clean.
pub fn next_action(f: &ReleaseFacts) -> Option<String> {
    match phase(f) {
        "staged" => Some(format!(
            "a promote is mid-flight or stranded (pending build_id={}). If no promote is running: resume it (`promote-live --resume --restart-live`) or clear `.axon/live-release/pending.json`.",
            f.pending_build_id.as_deref().unwrap_or("<unknown>")
        )),
        "drift" => Some(format!(
            "running build_id ({}) != promoted manifest ({}). Re-promote HEAD (`promote_live_safe.sh --project AXO`) or roll back (`rollback_live.sh`).",
            f.live_build_id,
            f.manifest_build_id.as_deref().unwrap_or("<none>")
        )),
        "uninitialized" => {
            Some("no current.json manifest — run an initial promote to record the live release.".to_string())
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Cutover FSM (REQ-AXO-902165 — health-gated cutover + auto-rollback).
//
// True blue-green is INFEASIBLE here: the SOLL/IST writer guards are EXCLUSIVE and
// acquired at boot (runtime_boot.rs — a second writer instance is refused startup),
// so the new and old runtimes cannot coexist. The cutover is therefore in-place
// (stop old → start new) with a health-gate + AUTO-ROLLBACK: the new runtime must
// prove the FULL runtime_contract healthy within a deadline, otherwise the previous
// release is restored — turning a failed promote from a stranded outage (the s94
// incident) into a brief blip + auto-recovery. Pure predicates, same shape as the
// release/stop FSMs: facts in, `Vec<Gate>` + derived `phase`/`next_action` out.
// ---------------------------------------------------------------------------

/// Facts about an in-flight in-place cutover, sampled after the new runtime is started.
#[derive(Debug, Clone, Default)]
pub struct CutoverFacts {
    /// Liveness of the NEW (candidate) runtime.
    pub new_liveness: LivenessFacts,
    /// Qualify verdict on the new runtime (`None` = not run / not yet).
    pub new_qualify_ok: Option<bool>,
    /// The health-gate deadline elapsed without the new runtime going healthy.
    pub deadline_exceeded: bool,
    /// Auto-rollback finished: the previous binary + manifest are restored & serving.
    pub old_restored: bool,
}

impl CutoverFacts {
    /// The new runtime is fully healthy: brain serving + indexer alive (per the
    /// runtime_contract) AND qualify not-failed. Reuses the liveness gates as the
    /// single source of truth (an absent qualify verdict is not a failure).
    pub fn new_healthy(&self) -> bool {
        evaluate_liveness_gates(&self.new_liveness)
            .iter()
            .all(|g| g.pass)
            && self.new_qualify_ok != Some(false)
    }
}

/// Evaluate the cutover gate (pure predicate over `CutoverFacts`).
pub fn evaluate_cutover_gates(f: &CutoverFacts) -> Vec<Gate> {
    vec![Gate {
        name: "new_runtime_healthy",
        pass: f.new_healthy(),
        detail: if f.new_healthy() {
            "new runtime healthy (full runtime_contract + qualify)".to_string()
        } else if f.deadline_exceeded {
            "new runtime NOT healthy within the deadline → auto-rollback".to_string()
        } else {
            "new runtime not yet healthy → awaiting".to_string()
        },
    }]
}

/// Derive the cutover phase (projection of the cutover FSM state). A `healthy` new
/// runtime wins even at the deadline; otherwise a passed deadline triggers rollback.
pub fn cutover_phase(f: &CutoverFacts) -> &'static str {
    if f.new_healthy() {
        "healthy"
    } else if f.old_restored {
        "rolled_back"
    } else if f.deadline_exceeded {
        "rolling_back"
    } else {
        "awaiting_health"
    }
}

/// The single corrective action that advances the cutover, or `None` when the new
/// runtime is healthy (the promote finalizes).
pub fn cutover_next_action(f: &CutoverFacts) -> Option<String> {
    match cutover_phase(f) {
        "healthy" => None,
        "awaiting_health" => Some(
            "new runtime started — poll its liveness (brain_serving + indexer_alive) until healthy or the deadline elapses.".to_string(),
        ),
        "rolling_back" => Some(
            "new runtime failed the health-gate within the deadline — AUTO-ROLLBACK: restore the previous binary + manifest and restart the old release.".to_string(),
        ),
        "rolled_back" => Some(
            "auto-rollback complete: the previous release is serving again; the promote did NOT apply. Investigate the candidate before retrying.".to_string(),
        ),
        _ => None,
    }
}

/// REQ-AXO-902165 — the cutover DRIVER: poll the new runtime's health up to `max_polls`
/// times, returning `Promoted` the instant it is healthy, or `RolledBack` once the polls
/// are exhausted (the deadline). Both effects — the health probe and the inter-poll wait
/// — are INJECTED, so the finalize-vs-rollback decision flow is unit-testable without a
/// runtime or a real clock. The caller (`axonctl cutover`) supplies the real probe
/// (`axonctl liveness`) + the wait (sleep) and performs finalize/rollback on the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutoverOutcome {
    /// New runtime went healthy within the deadline → finalize the promote.
    Promoted,
    /// New runtime never healthy within the deadline → restore the previous release.
    RolledBack,
}

pub fn run_cutover_loop(
    mut probe_healthy: impl FnMut() -> bool,
    max_polls: usize,
    mut wait_between_polls: impl FnMut(),
) -> CutoverOutcome {
    for _ in 0..max_polls.max(1) {
        if probe_healthy() {
            return CutoverOutcome::Promoted;
        }
        wait_between_polls();
    }
    CutoverOutcome::RolledBack
}

// ---------------------------------------------------------------------------
// Cutover CHOREOGRAPHY (REQ-AXO-902165 — the I/O executor's decision layer).
//
// `drive_cutover` sequences the side-effecting steps of an in-place cutover
// (snapshot → stage → restart → poll-health → finalize|rollback) around the
// already-tested `run_cutover_loop` driver. Every effect is behind the injected
// `CutoverIo` trait + a separate health probe/wait, so the WHOLE finalize-vs-
// rollback decision flow — including the s94 incident guard (an unhealthy
// candidate must ALWAYS restore the old release, never strand a half-finalized
// manifest) — is unit-testable without a runtime, a clock, or disk (practice 128:
// the decision + driver are pure/injected; only the real `CutoverIo` impl in
// `axonctl` touches bin/*, manifests, and processes, and that is gated on an E2E
// DEV fault-injection run before it may drive a live promote).
// ---------------------------------------------------------------------------

/// The side-effecting steps of an in-place cutover, injected so the choreography is
/// testable without a runtime. The real impl (`axonctl`'s `RealCutoverIo`) replicates
/// promote_live.sh's manifest/bin I/O; a fake records the call order + scripted errors.
///
/// Invariant every impl must uphold: after `rollback()` returns `Ok`, the PREVIOUS
/// release (captured by `snapshot_current`) is restored on disk and restarting.
pub trait CutoverIo {
    /// Capture the currently-serving release (bin/* + current.json) as the rollback
    /// target. Runs BEFORE anything is mutated; `Err` aborts with nothing touched.
    fn snapshot_current(&mut self) -> Result<(), String>;
    /// Stage the candidate: write pending.json (state=staged) + swap the candidate
    /// bin/* into place. `Err` → the old release is restored (bin/* may be partial).
    fn stage_candidate(&mut self) -> Result<(), String>;
    /// Restart the runtime onto the swapped binaries (stop old → start new).
    fn restart_runtime(&mut self) -> Result<(), String>;
    /// Finalize the promote: archive current→history, pending→current (state=promoted).
    fn finalize(&mut self) -> Result<(), String>;
    /// AUTO-ROLLBACK: restore bin/* from the snapshot (current.json), drop pending,
    /// restart the previous release. Must leave the OLD release serving.
    fn rollback(&mut self) -> Result<(), String>;
}

/// The terminal verdict of a cutover: either the candidate went healthy and was
/// finalized, or it failed (at a named step) and the old release was restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutoverVerdict {
    /// New runtime healthy within the deadline → promote finalized.
    Promoted,
    /// Candidate failed at `failed_step`; the old release was restored. `rollback_ok`
    /// is the result of the restore itself — `false` means the rollback ALSO failed
    /// (a genuine outage requiring operator action, surfaced distinctly).
    RolledBack {
        failed_step: &'static str,
        rollback_ok: bool,
        detail: Option<String>,
    },
}

impl CutoverVerdict {
    pub fn is_promoted(&self) -> bool {
        matches!(self, CutoverVerdict::Promoted)
    }
}

/// REQ-AXO-902165 — the in-place cutover choreography. Composes the tested cutover
/// driver (`run_cutover_loop`) with the injected I/O steps + a health probe/wait
/// (kept separate from `io` so the poll never double-borrows the effects object).
///
/// Failure handling (the incident guard): a failure of `stage_candidate`,
/// `restart_runtime`, OR the health-gate all funnel into `rollback()` and return
/// `RolledBack` — so a bad candidate is a blip + auto-recovery, never a stranded
/// outage. A `snapshot_current` failure aborts BEFORE any mutation (old release
/// untouched, so no rollback is attempted — nothing was changed).
pub fn drive_cutover<Io, Probe, Wait>(
    io: &mut Io,
    mut probe_healthy: Probe,
    max_polls: usize,
    wait_between_polls: Wait,
) -> CutoverVerdict
where
    Io: CutoverIo,
    Probe: FnMut() -> bool,
    Wait: FnMut(),
{
    // Snapshot first. If we cannot even capture a rollback target, do NOT touch the
    // running release — abort with everything intact (no rollback needed/possible).
    if let Err(e) = io.snapshot_current() {
        return CutoverVerdict::RolledBack {
            failed_step: "snapshot_current",
            rollback_ok: true, // nothing was mutated; the old release still serves.
            detail: Some(e),
        };
    }
    // From here on, any failure restores the snapshot.
    if let Err(e) = io.stage_candidate() {
        return rolled_back(io, "stage_candidate", e);
    }
    if let Err(e) = io.restart_runtime() {
        return rolled_back(io, "restart_runtime", e);
    }
    // Health-gate the new runtime. `run_cutover_loop` returns the instant it is
    // healthy, or `RolledBack` once the deadline (max_polls) is exhausted.
    match run_cutover_loop(&mut probe_healthy, max_polls, wait_between_polls) {
        CutoverOutcome::Promoted => match io.finalize() {
            Ok(()) => CutoverVerdict::Promoted,
            // Candidate was healthy but the manifest finalize failed: the new runtime
            // IS serving, but current.json wasn't advanced. Roll back to the coherent
            // old release rather than leave bin/* ↔ manifest drift (the s91 failure).
            Err(e) => rolled_back(io, "finalize", e),
        },
        CutoverOutcome::RolledBack => rolled_back(
            io,
            "health_gate",
            "new runtime never healthy within the deadline".to_string(),
        ),
    }
}

/// Restore the old release and build the `RolledBack` verdict, recording whether the
/// restore itself succeeded (a failed rollback = a real outage, surfaced distinctly).
fn rolled_back<Io: CutoverIo>(io: &mut Io, failed_step: &'static str, detail: String) -> CutoverVerdict {
    let rollback_ok = io.rollback().is_ok();
    CutoverVerdict::RolledBack {
        failed_step,
        rollback_ok,
        detail: Some(detail),
    }
}

// ---------------------------------------------------------------------------
// Stop FSM (REQ-AXO-902111 — stop-verdict slice).
//
// The stop verdict must live where it SURVIVES the thing being stopped: in
// `axonctl` (the supervisor, which outlives the brain it tears down), NOT in an
// MCP tool of the brain (which dies mid-answer the moment its own listener is
// reaped). So the gates live here as pure predicates and `axonctl::cmd_stop`
// populates `StopFacts` from `find_instance_all_pids` + a PC-daemon probe (the
// wiring step is orchestrator-side; see WIRING.md). Same shape as the release
// gates above: facts in, `Vec<Gate>` + derived `phase`/`next_action` out.
// ---------------------------------------------------------------------------

/// Facts about an in-flight stop, collected by `axonctl` AFTER it has emitted the
/// teardown signals. All scoped to the role being stopped (`stop_role`): "all" for
/// a full teardown, or a single role ("brain"/"indexer") for a role-scoped stop
/// that intentionally preserves the other role (PIL-AXO-004 split deployment).
#[derive(Debug, Clone, Default)]
pub struct StopFacts {
    /// Which role we asked to stop: "all" | "brain" | "indexer".
    pub stop_role: String,
    /// Live PIDs still bound to the canonical listeners for `stop_role` (post-SIGTERM).
    /// A non-empty set means a process survived the teardown = orphaned.
    pub canonical_listeners: Vec<i32>,
    /// The brain MCP port is still bound (may be kernel TIME_WAIT draining even when
    /// `canonical_listeners` is already empty).
    pub brain_port_bound: bool,
    /// The supervisor (PC-daemon / axonctl supervise loop) is still alive. For a full
    /// teardown this is an orphan (it will respawn the role we killed); for a
    /// role-scoped stop it is expected (it keeps the surviving role up).
    pub supervisor_healthy: bool,
    /// Writer locks still held on disk (e.g. IST writer lock files) for `stop_role`.
    pub writer_locks_held: Vec<String>,
    /// Control sockets (telemetry/mcp) still present on disk.
    pub sockets_present: bool,
    /// The indexer heartbeat is still fresh (draining indicator when the indexer is
    /// the role being stopped).
    pub indexer_heartbeat_fresh: bool,
}

impl StopFacts {
    fn is_full_teardown(&self) -> bool {
        self.stop_role.eq_ignore_ascii_case("all")
    }
}

/// Evaluate the stop gates (pure predicates over `StopFacts`).
/// `no_canonical_listeners` + `writer_locks_released` + `sockets_cleaned` are
/// universal; `supervisor_quiesced` is N/A for a role-scoped stop (the supervisor
/// stays up for the surviving role by design).
pub fn evaluate_stop_gates(f: &StopFacts) -> Vec<Gate> {
    let full = f.is_full_teardown();
    vec![
        Gate {
            name: "no_canonical_listeners",
            pass: f.canonical_listeners.is_empty(),
            detail: if f.canonical_listeners.is_empty() {
                format!("no canonical listeners left for role '{}'", f.stop_role)
            } else {
                format!(
                    "listeners survived for role '{}' (pids={:?})",
                    f.stop_role, f.canonical_listeners
                )
            },
        },
        Gate {
            name: "supervisor_quiesced",
            // N/A unless this is a full teardown: a role-scoped stop intentionally
            // leaves the supervisor running for the surviving role (PIL-AXO-004).
            pass: !full || !f.supervisor_healthy,
            detail: if !full {
                format!(
                    "role-scoped stop ('{}') — supervisor stays up for the other role; gate N/A",
                    f.stop_role
                )
            } else if f.supervisor_healthy {
                "supervisor still healthy — it will respawn the role just killed".to_string()
            } else {
                "supervisor quiesced".to_string()
            },
        },
        Gate {
            name: "writer_locks_released",
            pass: f.writer_locks_held.is_empty(),
            detail: if f.writer_locks_held.is_empty() {
                "no writer locks held".to_string()
            } else {
                format!("writer locks still held: {}", f.writer_locks_held.join(", "))
            },
        },
        Gate {
            name: "sockets_cleaned",
            pass: !f.sockets_present,
            detail: if f.sockets_present {
                "control sockets still present on disk".to_string()
            } else {
                "control sockets cleaned".to_string()
            },
        },
    ]
}

/// Derive the stop phase (the projection of the stop FSM state).
///
/// Precedence: orphaned (a live listener survived OR a full-teardown supervisor is
/// still alive) > stopping (listeners gone but ports/heartbeat draining or cleanup
/// pending) > partial (role-scoped success, the other role preserved by design) /
/// stopped (full teardown, everything clean).
pub fn stop_phase(f: &StopFacts) -> &'static str {
    let full = f.is_full_teardown();
    // Orphaned: a real listener PID survived the teardown, or — on a full teardown —
    // the supervisor is still alive and will respawn what we just killed.
    if !f.canonical_listeners.is_empty() || (full && f.supervisor_healthy) {
        return "orphaned";
    }
    // Live listeners are gone. Still draining (kernel port TIME_WAIT / heartbeat TTL)
    // or cleanup not yet done?
    let draining = f.brain_port_bound
        || f.indexer_heartbeat_fresh
        || f.sockets_present
        || !f.writer_locks_held.is_empty();
    if draining {
        return "stopping";
    }
    // Fully clean. A role-scoped stop that left the other role alive by design is a
    // first-class success (PIL-AXO-004), reported distinctly from a full teardown.
    if full {
        "stopped"
    } else {
        "partial"
    }
}

/// The corrective action that closes an orphaned stop, or `None` when the stop
/// reached a terminal good state (stopped/partial) or is merely still draining.
pub fn stop_next_action(f: &StopFacts) -> Option<String> {
    if stop_phase(f) != "orphaned" {
        return None;
    }
    // Supervisor first: killing the listeners is futile while a live supervisor will
    // respawn them.
    if f.is_full_teardown() && f.supervisor_healthy {
        return Some(
            "supervisor still alive — it will respawn the role you killed. Reap the supervisor and re-run the teardown with --hard (`axonctl stop --hard`).".to_string(),
        );
    }
    if !f.canonical_listeners.is_empty() {
        let pids = f
            .canonical_listeners
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        return Some(format!(
            "listeners survived SIGTERM for role '{}' — kill them by PID and re-verify: `kill -9 {}`.",
            f.stop_role, pids
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(live: &str, manifest: Option<&str>, pending: bool) -> ReleaseFacts {
        ReleaseFacts {
            live_build_id: live.to_string(),
            manifest_build_id: manifest.map(str::to_string),
            manifest_state: Some("promoted".to_string()),
            qualification_ok: Some(true),
            pending_present: pending,
            pending_build_id: if pending { Some("v0.0.0-staged".to_string()) } else { None },
            runtime_contract: Some("brain_mcp_indexer_ist".to_string()),
        }
    }

    fn live(brain: bool, expected: bool, ready: bool, lifecycle: &str, source: &str) -> LivenessFacts {
        LivenessFacts {
            brain_serving: brain,
            indexer_expected: expected,
            indexer_ready: ready,
            indexer_lifecycle: lifecycle.to_string(),
            indexer_source: source.to_string(),
        }
    }

    #[test]
    fn indexer_expected_from_runtime_contract() {
        let f = facts("v1", Some("v1"), false);
        assert!(f.indexer_expected()); // "brain_mcp_indexer_ist" contains "indexer"
        let mut g = f.clone();
        g.runtime_contract = Some("brain_only".to_string());
        assert!(!g.indexer_expected());
    }

    #[test]
    fn liveness_clean_when_brain_serves_and_indexer_fresh() {
        let l = live(true, true, true, "healthy", "pg_heartbeat");
        assert!(evaluate_liveness_gates(&l).iter().all(|g| g.pass));
        assert!(liveness_phase(&l).is_none());
        assert!(liveness_next_action(&l).is_none());
    }

    #[test]
    fn brain_down_takes_precedence() {
        let l = live(false, true, true, "healthy", "pg_heartbeat");
        assert_eq!(liveness_phase(&l), Some("brain_down"));
        assert!(liveness_next_action(&l).unwrap().contains("DB probe"));
        assert!(evaluate_liveness_gates(&l).iter().any(|g| g.name == "brain_serving" && !g.pass));
    }

    #[test]
    fn indexer_stale_vs_never_launched_actions_differ() {
        let stale = live(true, true, false, "crashed_or_abandoned", "pg_heartbeat_stale");
        assert_eq!(liveness_phase(&stale), Some("indexer_down"));
        assert!(liveness_next_action(&stale).unwrap().contains("restart"));
        let never = live(true, true, false, "never_launched", "no_heartbeat");
        assert!(liveness_next_action(&never).unwrap().contains("start the full runtime"));
    }

    #[test]
    fn indexer_gate_na_when_not_expected() {
        // brain-only contract: a missing indexer is not a failure.
        let l = live(true, false, false, "never_launched", "no_heartbeat");
        assert!(evaluate_liveness_gates(&l).iter().all(|g| g.pass));
        assert!(liveness_phase(&l).is_none());
    }

    #[test]
    fn clean_when_manifest_matches_and_no_pending() {
        let f = facts("v1-gabc", Some("v1-gabc"), false);
        assert_eq!(phase(&f), "clean");
        assert!(next_action(&f).is_none());
        assert!(evaluate_gates(&f).iter().all(|g| g.pass));
    }

    #[test]
    fn drift_when_running_differs_from_manifest() {
        let f = facts("v2-gnew", Some("v1-gold"), false);
        assert_eq!(phase(&f), "drift");
        assert!(next_action(&f).unwrap().contains("Re-promote"));
        let gates = evaluate_gates(&f);
        assert!(gates.iter().any(|g| g.name == "manifest_runtime_match" && !g.pass));
    }

    #[test]
    fn staged_when_pending_present() {
        // The session-91 stranded-pending failure.
        let f = facts("v1-gabc", Some("v1-gabc"), true);
        assert_eq!(phase(&f), "staged");
        let gates = evaluate_gates(&f);
        assert!(gates.iter().any(|g| g.name == "no_stale_pending" && !g.pass));
        assert!(next_action(&f).unwrap().contains("resume"));
    }

    #[test]
    fn failed_qualification_fails_only_that_gate() {
        let mut f = facts("v1-gabc", Some("v1-gabc"), false);
        f.qualification_ok = Some(false);
        let gates = evaluate_gates(&f);
        assert!(gates.iter().any(|g| g.name == "qualification_passed" && !g.pass));
        assert!(gates.iter().any(|g| g.name == "manifest_runtime_match" && g.pass));
    }

    // --- Cutover FSM ------------------------------------------------------

    #[test]
    fn cutover_healthy_new_finalizes() {
        let f = CutoverFacts {
            new_liveness: live(true, true, true, "healthy", "pg_heartbeat"),
            new_qualify_ok: Some(true),
            deadline_exceeded: false,
            old_restored: false,
        };
        assert!(f.new_healthy());
        assert_eq!(cutover_phase(&f), "healthy");
        assert!(evaluate_cutover_gates(&f).iter().all(|g| g.pass));
        assert!(cutover_next_action(&f).is_none());
    }

    #[test]
    fn cutover_awaits_while_new_converging() {
        let f = CutoverFacts {
            new_liveness: live(true, true, false, "never_launched", "no_heartbeat"),
            new_qualify_ok: None,
            deadline_exceeded: false,
            old_restored: false,
        };
        assert_eq!(cutover_phase(&f), "awaiting_health");
        assert!(cutover_next_action(&f).unwrap().contains("poll"));
    }

    #[test]
    fn cutover_rolls_back_when_deadline_exceeded_unhealthy() {
        // THE s94 failure mode: the new runtime never becomes healthy. Must AUTO-ROLLBACK,
        // never strand the live in an outage with a half-finalized manifest.
        let f = CutoverFacts {
            new_liveness: live(false, true, false, "crashed_or_abandoned", "no_heartbeat"),
            new_qualify_ok: None,
            deadline_exceeded: true,
            old_restored: false,
        };
        assert_eq!(cutover_phase(&f), "rolling_back");
        assert!(evaluate_cutover_gates(&f)
            .iter()
            .any(|g| g.name == "new_runtime_healthy" && !g.pass));
        assert!(cutover_next_action(&f).unwrap().contains("AUTO-ROLLBACK"));
    }

    #[test]
    fn cutover_rolled_back_after_restore() {
        let f = CutoverFacts {
            new_liveness: live(false, true, false, "crashed_or_abandoned", "no_heartbeat"),
            new_qualify_ok: None,
            deadline_exceeded: true,
            old_restored: true,
        };
        assert_eq!(cutover_phase(&f), "rolled_back");
        assert!(cutover_next_action(&f)
            .unwrap()
            .contains("previous release is serving"));
    }

    #[test]
    fn cutover_healthy_wins_even_at_deadline() {
        // Went healthy right as the deadline passed → finalize, do NOT roll back.
        let f = CutoverFacts {
            new_liveness: live(true, true, true, "healthy", "pg_heartbeat"),
            new_qualify_ok: Some(true),
            deadline_exceeded: true,
            old_restored: false,
        };
        assert_eq!(cutover_phase(&f), "healthy");
    }

    #[test]
    fn cutover_failed_qualify_blocks_health_even_when_live() {
        // brain+indexer live but qualify FAILED → not healthy → rollback on deadline.
        let f = CutoverFacts {
            new_liveness: live(true, true, true, "healthy", "pg_heartbeat"),
            new_qualify_ok: Some(false),
            deadline_exceeded: true,
            old_restored: false,
        };
        assert!(!f.new_healthy());
        assert_eq!(cutover_phase(&f), "rolling_back");
    }

    #[test]
    fn cutover_loop_promotes_on_first_healthy_poll() {
        let mut polls = 0;
        let out = run_cutover_loop(
            || {
                polls += 1;
                true
            },
            10,
            || {},
        );
        assert_eq!(out, CutoverOutcome::Promoted);
        assert_eq!(polls, 1, "should stop probing the instant it is healthy");
    }

    #[test]
    fn cutover_loop_rolls_back_when_never_healthy() {
        // THE incident guard: an unhealthy new runtime must roll back after the deadline,
        // never hang or strand.
        let mut waits = 0;
        let out = run_cutover_loop(|| false, 5, || waits += 1);
        assert_eq!(out, CutoverOutcome::RolledBack);
        assert_eq!(waits, 5);
    }

    #[test]
    fn cutover_loop_promotes_when_healthy_on_third_poll() {
        let mut n = 0;
        let out = run_cutover_loop(
            || {
                n += 1;
                n >= 3
            },
            10,
            || {},
        );
        assert_eq!(out, CutoverOutcome::Promoted);
        assert_eq!(n, 3);
    }

    // --- Cutover CHOREOGRAPHY (drive_cutover) -----------------------------

    /// Records the ordered I/O steps a cutover performed, with a scripted failure at a
    /// chosen step, so `drive_cutover`'s sequencing + rollback decisions are asserted
    /// without a runtime. `fail_at` names the step whose call returns `Err`.
    #[derive(Default)]
    struct FakeIo {
        calls: Vec<&'static str>,
        fail_at: Option<&'static str>,
        rollback_fails: bool,
    }

    impl FakeIo {
        fn failing(step: &'static str) -> Self {
            FakeIo {
                fail_at: Some(step),
                ..Default::default()
            }
        }
        fn step(&mut self, name: &'static str) -> Result<(), String> {
            self.calls.push(name);
            if self.fail_at == Some(name) {
                Err(format!("scripted failure at {name}"))
            } else {
                Ok(())
            }
        }
    }

    impl CutoverIo for FakeIo {
        fn snapshot_current(&mut self) -> Result<(), String> {
            self.step("snapshot_current")
        }
        fn stage_candidate(&mut self) -> Result<(), String> {
            self.step("stage_candidate")
        }
        fn restart_runtime(&mut self) -> Result<(), String> {
            self.step("restart_runtime")
        }
        fn finalize(&mut self) -> Result<(), String> {
            self.step("finalize")
        }
        fn rollback(&mut self) -> Result<(), String> {
            self.calls.push("rollback");
            if self.rollback_fails {
                Err("scripted rollback failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn cutover_healthy_candidate_finalizes_never_rolls_back() {
        let mut io = FakeIo::default();
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        assert_eq!(verdict, CutoverVerdict::Promoted);
        // The happy path: snapshot → stage → restart → finalize, and NO rollback.
        assert_eq!(
            io.calls,
            vec![
                "snapshot_current",
                "stage_candidate",
                "restart_runtime",
                "finalize"
            ]
        );
        assert!(!io.calls.contains(&"rollback"));
    }

    #[test]
    fn cutover_unhealthy_candidate_auto_rolls_back() {
        // THE s94 incident guard: a candidate that never goes healthy MUST restore the
        // old release (rollback) and NEVER finalize a half-promoted manifest.
        let mut io = FakeIo::default();
        let verdict = drive_cutover(&mut io, || false, 3, || {});
        assert_eq!(
            verdict,
            CutoverVerdict::RolledBack {
                failed_step: "health_gate",
                rollback_ok: true,
                detail: Some("new runtime never healthy within the deadline".to_string()),
            }
        );
        assert_eq!(
            io.calls,
            vec![
                "snapshot_current",
                "stage_candidate",
                "restart_runtime",
                "rollback"
            ]
        );
        assert!(!io.calls.contains(&"finalize"), "must NOT finalize a bad candidate");
    }

    #[test]
    fn cutover_snapshot_failure_aborts_before_touching_anything() {
        // Cannot capture a rollback target → do NOT stage/restart. Old release intact,
        // no rollback attempted (nothing was mutated).
        let mut io = FakeIo::failing("snapshot_current");
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        match verdict {
            CutoverVerdict::RolledBack { failed_step, rollback_ok, .. } => {
                assert_eq!(failed_step, "snapshot_current");
                assert!(rollback_ok, "nothing mutated → old release still serves");
            }
            other => panic!("expected RolledBack, got {other:?}"),
        }
        assert_eq!(io.calls, vec!["snapshot_current"]);
        assert!(!io.calls.contains(&"stage_candidate"));
        assert!(!io.calls.contains(&"rollback"));
    }

    #[test]
    fn cutover_stage_failure_rolls_back_without_restart() {
        let mut io = FakeIo::failing("stage_candidate");
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        assert!(matches!(
            verdict,
            CutoverVerdict::RolledBack { failed_step: "stage_candidate", rollback_ok: true, .. }
        ));
        assert_eq!(
            io.calls,
            vec!["snapshot_current", "stage_candidate", "rollback"]
        );
        assert!(!io.calls.contains(&"restart_runtime"));
    }

    #[test]
    fn cutover_restart_failure_rolls_back() {
        let mut io = FakeIo::failing("restart_runtime");
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        assert!(matches!(
            verdict,
            CutoverVerdict::RolledBack { failed_step: "restart_runtime", .. }
        ));
        assert_eq!(
            io.calls,
            vec!["snapshot_current", "stage_candidate", "restart_runtime", "rollback"]
        );
    }

    #[test]
    fn cutover_finalize_failure_rolls_back_to_coherent_old_release() {
        // Healthy candidate but the manifest finalize failed: roll back rather than
        // leave bin/* ↔ current.json drift (the s91 stranded-pending class).
        let mut io = FakeIo::failing("finalize");
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        assert!(matches!(
            verdict,
            CutoverVerdict::RolledBack { failed_step: "finalize", .. }
        ));
        assert_eq!(
            io.calls,
            vec![
                "snapshot_current",
                "stage_candidate",
                "restart_runtime",
                "finalize",
                "rollback"
            ]
        );
    }

    #[test]
    fn cutover_failed_rollback_is_surfaced_distinctly() {
        // A rollback that ALSO fails = a genuine outage; `rollback_ok:false` lets the
        // caller escalate (operator action) instead of reporting a clean auto-recovery.
        let mut io = FakeIo {
            rollback_fails: true,
            ..Default::default()
        };
        let verdict = drive_cutover(&mut io, || false, 2, || {});
        assert!(matches!(
            verdict,
            CutoverVerdict::RolledBack { failed_step: "health_gate", rollback_ok: false, .. }
        ));
    }

    #[test]
    fn cutover_healthy_on_second_poll_finalizes() {
        let mut n = 0;
        let mut waits = 0;
        let mut io = FakeIo::default();
        let verdict = drive_cutover(
            &mut io,
            || {
                n += 1;
                n >= 2
            },
            5,
            || waits += 1,
        );
        assert_eq!(verdict, CutoverVerdict::Promoted);
        assert_eq!(n, 2);
        assert_eq!(waits, 1, "one wait between the failed first poll and the healthy second");
    }

    // --- Stop FSM ---------------------------------------------------------

    /// A fully clean full teardown: nothing left to do.
    fn stop_clean_all() -> StopFacts {
        StopFacts {
            stop_role: "all".to_string(),
            canonical_listeners: vec![],
            brain_port_bound: false,
            supervisor_healthy: false,
            writer_locks_held: vec![],
            sockets_present: false,
            indexer_heartbeat_fresh: false,
        }
    }

    #[test]
    fn stop_clean_full_teardown_is_stopped() {
        let f = stop_clean_all();
        assert_eq!(stop_phase(&f), "stopped");
        assert!(evaluate_stop_gates(&f).iter().all(|g| g.pass));
        assert!(stop_next_action(&f).is_none());
    }

    #[test]
    fn stop_orphaned_when_supervisor_alive_on_full_teardown() {
        let mut f = stop_clean_all();
        f.supervisor_healthy = true;
        assert_eq!(stop_phase(&f), "orphaned");
        let gates = evaluate_stop_gates(&f);
        assert!(gates.iter().any(|g| g.name == "supervisor_quiesced" && !g.pass));
        // Supervisor takes priority: the action is reap + --hard, not kill-by-pid.
        let action = stop_next_action(&f).unwrap();
        assert!(action.contains("--hard"));
        assert!(action.contains("supervisor"));
    }

    #[test]
    fn stop_orphaned_when_listeners_survive() {
        let mut f = stop_clean_all();
        f.canonical_listeners = vec![4242, 4243];
        assert_eq!(stop_phase(&f), "orphaned");
        let gates = evaluate_stop_gates(&f);
        assert!(gates.iter().any(|g| g.name == "no_canonical_listeners" && !g.pass));
        let action = stop_next_action(&f).unwrap();
        assert!(action.contains("kill -9 4242 4243"));
    }

    #[test]
    fn stop_partial_when_role_scoped_supervisor_is_na() {
        // Role-scoped stop of the indexer: the supervisor stays up for the brain by
        // design (PIL-AXO-004), so supervisor_quiesced is N/A and the verdict is a
        // first-class success (partial), NOT orphaned.
        let mut f = stop_clean_all();
        f.stop_role = "indexer".to_string();
        f.supervisor_healthy = true;
        assert_eq!(stop_phase(&f), "partial");
        let gates = evaluate_stop_gates(&f);
        assert!(gates.iter().any(|g| g.name == "supervisor_quiesced" && g.pass));
        assert!(gates.iter().all(|g| g.pass));
        assert!(stop_next_action(&f).is_none());
    }

    #[test]
    fn stop_stopping_while_draining() {
        // Listeners gone, but the kernel port is still in TIME_WAIT and sockets not
        // yet unlinked — transient, no corrective action.
        let mut f = stop_clean_all();
        f.brain_port_bound = true;
        f.sockets_present = true;
        assert_eq!(stop_phase(&f), "stopping");
        assert!(stop_next_action(&f).is_none());
    }
}
