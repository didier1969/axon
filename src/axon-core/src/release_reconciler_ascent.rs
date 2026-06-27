//! REQ-AXO-902111 / DEC-AXO-901662 (T2) — Ascent/Datalog re-expression of the
//! reconciler's RELEASE gates. The typed Rust predicates in
//! [`crate::release_reconciler`] stay the canonical oracle; this module proves the
//! Datalog rules derive the SAME verdicts (exhaustive differential test below), so
//! the migration is behaviour-preserving (T1 → T2 without semantic drift).
//!
//! Scope of this slice: the 3 release gates (`manifest_runtime_match`,
//! `no_stale_pending`, `qualification_passed`). Liveness + stop gates + phase
//! precedence follow once this equivalence is proven. Ascent = LOGIC (pass/fail);
//! the human `detail`/`next_action` strings stay in Rust (presentation).

use crate::release_reconciler::{LivenessFacts, ReleaseFacts};

use datalog::AscentProgram;

// The `ascent!` macro generates engine code with unused-variable bindings; scope the
// lint allowance to the generated module so our own code stays zero-warning.
#[allow(unused_variables, unused_assignments, clippy::all)]
mod datalog {
    use ascent::ascent;

    ascent! {
    /// Grounding fact so negation rules are range-restricted (Datalog safety).
    relation seed();
    /// Input relations (facts), populated from `ReleaseFacts`.
    relation live_build(String);
    relation manifest_build(String);
    relation pending_present();
    relation qualification_false();

    /// Liveness input facts (from `LivenessFacts`).
    relation brain_serving_fact();
    relation indexer_expected_fact();
    relation indexer_ready_fact();

    /// Derived gate-pass relations (a fact present == the gate passes).
    relation gate_manifest_match();
    relation gate_no_stale_pending();
    relation gate_qualification_passed();
    relation gate_brain_serving();
    relation gate_indexer_alive();

    // manifest_runtime_match: the running build id equals the promoted manifest id.
    gate_manifest_match() <-- live_build(b), manifest_build(b);
    // no_stale_pending: passes UNLESS a pending.json is present (stratified negation).
    gate_no_stale_pending() <-- seed(), !pending_present();
    // qualification_passed: passes unless an explicit non-ok verdict is recorded.
    gate_qualification_passed() <-- seed(), !qualification_false();
    // brain_serving: the brain answered the DB probe.
    gate_brain_serving() <-- brain_serving_fact();
    // indexer_alive: passes if no separate indexer is expected OR it is ready (union).
    gate_indexer_alive() <-- seed(), !indexer_expected_fact();
    gate_indexer_alive() <-- indexer_ready_fact();

    // --- PHASE precedence: the ordered Rust if/else re-expressed as mutually
    // exclusive derived relations via negative guards (stratified). ---
    relation has_manifest();
    relation manifest_ne_live();
    // release phase (mirrors release_reconciler::phase)
    relation rphase_staged();
    relation rphase_uninitialized();
    relation rphase_drift();
    relation rphase_clean();
    // liveness phase (mirrors release_reconciler::liveness_phase)
    relation lphase_brain_down();
    relation lphase_indexer_down();
    // overall phase = liveness_phase.unwrap_or(release phase) — a single string.
    relation overall_phase(String);

    has_manifest() <-- manifest_build(_x);
    manifest_ne_live() <-- live_build(l), manifest_build(m), if l != m;

    rphase_staged() <-- pending_present();
    rphase_uninitialized() <-- seed(), !pending_present(), !has_manifest();
    rphase_drift() <-- seed(), !pending_present(), has_manifest(), manifest_ne_live();
    rphase_clean() <-- seed(), !pending_present(), has_manifest(), !manifest_ne_live();

    lphase_brain_down() <-- seed(), !brain_serving_fact();
    lphase_indexer_down() <-- brain_serving_fact(), indexer_expected_fact(), !indexer_ready_fact();

    // liveness phases take precedence over the release-state phase.
    overall_phase("brain_down".to_string()) <-- lphase_brain_down();
    overall_phase("indexer_down".to_string()) <-- lphase_indexer_down();
    overall_phase("staged".to_string()) <-- rphase_staged(), !lphase_brain_down(), !lphase_indexer_down();
    overall_phase("uninitialized".to_string()) <-- rphase_uninitialized(), !lphase_brain_down(), !lphase_indexer_down();
    overall_phase("drift".to_string()) <-- rphase_drift(), !lphase_brain_down(), !lphase_indexer_down();
    overall_phase("clean".to_string()) <-- rphase_clean(), !lphase_brain_down(), !lphase_indexer_down();
    }
}

/// Run the Ascent program over the facts and return the 3 release-gate verdicts in
/// the same order as [`crate::release_reconciler::evaluate_gates`]:
/// `(manifest_runtime_match, no_stale_pending, qualification_passed)`.
pub fn ascent_release_gates(f: &ReleaseFacts) -> (bool, bool, bool) {
    let mut prog = AscentProgram::default();
    prog.seed = vec![()];
    prog.live_build = vec![(f.live_build_id.clone(),)];
    if let Some(m) = &f.manifest_build_id {
        prog.manifest_build = vec![(m.clone(),)];
    }
    if f.pending_present {
        prog.pending_present = vec![()];
    }
    if f.qualification_ok == Some(false) {
        prog.qualification_false = vec![()];
    }
    prog.run();
    (
        !prog.gate_manifest_match.is_empty(),
        !prog.gate_no_stale_pending.is_empty(),
        !prog.gate_qualification_passed.is_empty(),
    )
}

/// Run the Ascent program over the liveness facts and return the 2 liveness-gate
/// verdicts in [`crate::release_reconciler::evaluate_liveness_gates`] order:
/// `(brain_serving, indexer_alive)`.
pub fn ascent_liveness_gates(l: &LivenessFacts) -> (bool, bool) {
    let mut prog = AscentProgram::default();
    prog.seed = vec![()];
    if l.brain_serving {
        prog.brain_serving_fact = vec![()];
    }
    if l.indexer_expected {
        prog.indexer_expected_fact = vec![()];
    }
    if l.indexer_ready {
        prog.indexer_ready_fact = vec![()];
    }
    prog.run();
    (
        !prog.gate_brain_serving.is_empty(),
        !prog.gate_indexer_alive.is_empty(),
    )
}

/// Derive the OVERALL phase (liveness precedence over release state) via Datalog,
/// equivalent to `liveness_phase(l).unwrap_or_else(|| phase(f))` in Rust.
pub fn ascent_overall_phase(f: &ReleaseFacts, l: &LivenessFacts) -> String {
    let mut prog = AscentProgram::default();
    prog.seed = vec![()];
    prog.live_build = vec![(f.live_build_id.clone(),)];
    if let Some(m) = &f.manifest_build_id {
        prog.manifest_build = vec![(m.clone(),)];
    }
    if f.pending_present {
        prog.pending_present = vec![()];
    }
    if f.qualification_ok == Some(false) {
        prog.qualification_false = vec![()];
    }
    if l.brain_serving {
        prog.brain_serving_fact = vec![()];
    }
    if l.indexer_expected {
        prog.indexer_expected_fact = vec![()];
    }
    if l.indexer_ready {
        prog.indexer_ready_fact = vec![()];
    }
    prog.run();
    prog.overall_phase
        .first()
        .map(|(s,)| s.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_reconciler::{
        evaluate_gates, evaluate_liveness_gates, liveness_phase, phase,
    };

    /// Exhaustive differential test for the OVERALL phase precedence (liveness over
    /// release) over the full release × liveness fact grid.
    #[test]
    fn ascent_matches_rust_overall_phase_exhaustively() {
        let builds = [("v1", Some("v1")), ("v1", Some("v2")), ("v1", None)];
        for (live, manifest) in builds {
            for pending in [false, true] {
                for brain in [false, true] {
                    for expected in [false, true] {
                        for ready in [false, true] {
                            let f = ReleaseFacts {
                                live_build_id: live.to_string(),
                                manifest_build_id: manifest.map(str::to_string),
                                manifest_state: Some("promoted".to_string()),
                                qualification_ok: Some(true),
                                pending_present: pending,
                                pending_build_id: None,
                                runtime_contract: Some("brain_mcp_indexer_ist".to_string()),
                            };
                            let l = LivenessFacts {
                                brain_serving: brain,
                                indexer_expected: expected,
                                indexer_ready: ready,
                                indexer_lifecycle: "healthy".to_string(),
                                indexer_source: "pg_heartbeat".to_string(),
                            };
                            let rust = liveness_phase(&l)
                                .map(str::to_string)
                                .unwrap_or_else(|| phase(&f).to_string());
                            assert_eq!(
                                rust,
                                ascent_overall_phase(&f, &l),
                                "phase mismatch: live={live} manifest={manifest:?} pending={pending} brain={brain} expected={expected} ready={ready}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Exhaustive differential test for the 2 liveness gates over the full grid
    /// (brain_serving × indexer_expected × indexer_ready).
    #[test]
    fn ascent_matches_rust_liveness_gates_exhaustively() {
        for brain in [false, true] {
            for expected in [false, true] {
                for ready in [false, true] {
                    let l = LivenessFacts {
                        brain_serving: brain,
                        indexer_expected: expected,
                        indexer_ready: ready,
                        indexer_lifecycle: "healthy".to_string(),
                        indexer_source: "pg_heartbeat".to_string(),
                    };
                    let gates = evaluate_liveness_gates(&l);
                    let rust = (
                        gates.iter().find(|g| g.name == "brain_serving").unwrap().pass,
                        gates.iter().find(|g| g.name == "indexer_alive").unwrap().pass,
                    );
                    assert_eq!(
                        rust,
                        ascent_liveness_gates(&l),
                        "Ascent≠Rust liveness for brain={brain} expected={expected} ready={ready}"
                    );
                }
            }
        }
    }

    /// Exhaustive differential test: over the FULL finite fact grid, the Ascent
    /// rules must derive exactly the same pass/fail as the Rust oracle gates.
    #[test]
    fn ascent_matches_rust_release_gates_exhaustively() {
        let builds = [("v1", Some("v1")), ("v1", Some("v2")), ("v1", None)];
        let pendings = [false, true];
        let quals = [None, Some(true), Some(false)];
        for (live, manifest) in builds {
            for &pending in &pendings {
                for &qual in &quals {
                    let f = ReleaseFacts {
                        live_build_id: live.to_string(),
                        manifest_build_id: manifest.map(str::to_string),
                        manifest_state: Some("promoted".to_string()),
                        qualification_ok: qual,
                        pending_present: pending,
                        pending_build_id: if pending {
                            Some("v0-staged".to_string())
                        } else {
                            None
                        },
                        runtime_contract: Some("brain_mcp_indexer_ist".to_string()),
                    };
                    let gates = evaluate_gates(&f);
                    let rust = (
                        gates.iter().find(|g| g.name == "manifest_runtime_match").unwrap().pass,
                        gates.iter().find(|g| g.name == "no_stale_pending").unwrap().pass,
                        gates.iter().find(|g| g.name == "qualification_passed").unwrap().pass,
                    );
                    let asc = ascent_release_gates(&f);
                    assert_eq!(
                        rust, asc,
                        "Ascent≠Rust for live={live} manifest={manifest:?} pending={pending} qual={qual:?}"
                    );
                }
            }
        }
    }
}
