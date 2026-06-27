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
        ReleaseFacts {
            live_build_id,
            manifest_build_id,
            manifest_state,
            qualification_ok,
            pending_present: pending.is_some(),
            pending_build_id: pending.as_ref().and_then(extract_build_id),
        }
    }
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
        }
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
}
