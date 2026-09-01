//! REQ-AXO-902111 / DEC-AXO-901662 — `promote_status` MCP tool (T1 read-only).
//!
//! Thin surface over [`crate::release_reconciler`]: collects release facts and
//! returns `{phase, observed, gates, failed_gates, next_action, recovery}` so an
//! agent reads the release truth in one call instead of grepping the promote
//! scripts. Read-only — it never mutates runtime or release state.

use serde_json::{json, Value};

use super::runtime_topology_support::{
    resolve_indexer_liveness, EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
};
use super::McpServer;
use crate::release_reconciler::{
    attempt_next_action, evaluate_attempt_gate, evaluate_gates, evaluate_liveness_gates,
    liveness_next_action, liveness_phase, next_action, phase, LivenessFacts, ReleaseFacts,
};

impl McpServer {
    pub(crate) fn axon_promote_status(&self, _args: &Value) -> Option<Value> {
        let live_build_id = std::env::var("AXON_BUILD_ID").unwrap_or_default();
        let release_dir = std::env::current_dir()
            .unwrap_or_default()
            .join(".axon")
            .join("live-release");
        let facts = ReleaseFacts::collect(&release_dir, live_build_id);

        // REQ-AXO-902111 liveness slice — populate runtime liveness from the SAME
        // in-process sources `status` trusts (never the declared mode of this
        // brain_only process). Indexer = PG heartbeat → resolve_indexer_liveness;
        // brain = a real `SELECT 1` DB probe (catches up-but-DB-disconnected).
        let now_ms = crate::clock::now_unix_ms();
        let hb = self
            .graph_store
            .latest_lifecycle_heartbeat("indexer")
            .ok()
            .flatten();
        let live = resolve_indexer_liveness(
            now_ms,
            hb.as_ref().map(|r| r.heartbeat_ms),
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        );
        let lf = LivenessFacts {
            brain_serving: self.execute_raw_sql("SELECT 1").is_ok(),
            indexer_expected: facts.indexer_expected(),
            indexer_ready: live.ready,
            indexer_lifecycle: live.lifecycle.to_string(),
            indexer_source: live.source.to_string(),
        };

        let mut gates = evaluate_gates(&facts);
        gates.extend(evaluate_liveness_gates(&lf));
        // REQ-AXO-902585 (défaut 2) — la dernière tentative enregistrée a son mot à dire.
        gates.push(evaluate_attempt_gate(&facts));
        // REQ-AXO-902585 — `failed_gates` ne porte que les gates VRAIMENT rouges.
        // Sûreté, pas style : `promote_live_safe.sh` teste `recon_failed ==
        // "indexer_alive"` par ÉGALITÉ EXACTE, et tout autre contenu le fait basculer
        // en redémarrage complet — « THIS INTERRUPTS THE BRAIN ». Un `Unknown` qui
        // fuiterait ici couperait le service à chaque promote sur un manifeste
        // ancien, c'est-à-dire presque toujours.
        let failed: Vec<&str> = gates.iter().filter(|g| g.is_red()).map(|g| g.name).collect();
        let unknown: Vec<&str> = gates
            .iter()
            .filter(|g| g.status == crate::release_reconciler::GateStatus::Unknown)
            .map(|g| g.name)
            .collect();
        // Liveness failures take precedence over the release-state phase/action.
        let ph = liveness_phase(&lf).unwrap_or_else(|| phase(&facts));
        // REQ-AXO-902585 — en DERNIER : un service à terre prime sur un déploiement
        // raté, mais un déploiement raté prime sur le silence.
        let action = liveness_next_action(&lf)
            .or_else(|| next_action(&facts))
            .or_else(|| attempt_next_action(&facts));
        let mut trace = facts.attempt.clone().unwrap_or_else(|| {
            json!({
                "release_attempt_id": facts.release_attempt_id.clone(),
                "phase": ph,
                "status": "unknown",
                "sha": Value::Null,
                "deadline_unix_ms": Value::Null,
                "last_event": Value::Null,
            })
        });
        if let Some(object) = trace.as_object_mut() {
            // REQ-AXO-902585 — dire si la tentative tracée est celle qui a produit le
            // manifeste servi. Sans ça, il fallait comparer deux ids à l'œil.
            object.insert(
                "manifest_release_attempt_id".to_string(),
                facts
                    .release_attempt_id
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            object.insert(
                "diverges_from_manifest".to_string(),
                Value::Bool(
                    facts.attempt_id.is_some() && facts.attempt_id != facts.release_attempt_id,
                ),
            );
            object.insert(
                "artifact_sha256".to_string(),
                facts
                    .artifact_sha256
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }

        let gates_json: Vec<Value> = gates
            .iter()
            // REQ-AXO-902585 — `pass` reste, en lecture conservatrice (Unknown != succès),
            // et `status` porte le tri-état pour qui sait le lire.
            .map(|g| json!({ "name": g.name, "pass": g.passes(), "status": g.status_str(), "detail": g.detail }))
            .collect();

        let text = format!(
            "### 🚦 promote_status\n\nphase=**{}** — running={} manifest={}{}\nfailed_gates: {}\nnext_action: {}",
            ph,
            facts.live_build_id,
            facts.manifest_build_id.as_deref().unwrap_or("<none>"),
            if facts.pending_present {
                " · pending staging present"
            } else {
                ""
            },
            if failed.is_empty() {
                if unknown.is_empty() {
                    "none".to_string()
                } else {
                    format!("none (unknown: {})", unknown.join(", "))
                }
            } else {
                failed.join(", ")
            },
            action
                .as_deref()
                .unwrap_or("none — live matches the promoted manifest"),
        );

        Some(json!({
            "content": [{ "type": "text", "text": text }],
            "data": {
                "status": "ok",
                "phase": ph,
                "observed": {
                    "live_build_id": facts.live_build_id,
                    "manifest_build_id": facts.manifest_build_id,
                    "manifest_state": facts.manifest_state,
                    "core_qualification_status": facts.core_qualification_status,
                    "core_qualification_evidence": facts.core_qualification_evidence,
                    "qualification_source": facts.qualification_source,
                    "pending_present": facts.pending_present,
                    "pending_build_id": facts.pending_build_id,
                    "runtime_contract": facts.runtime_contract,
                    "release_attempt_id": facts.release_attempt_id,
                    "pending_release_attempt_id": facts.pending_release_attempt_id,
                    "artifact_sha256": facts.artifact_sha256,
                    "liveness": {
                        "brain_serving": lf.brain_serving,
                        "indexer_expected": lf.indexer_expected,
                        "indexer_ready": lf.indexer_ready,
                        "indexer_lifecycle": lf.indexer_lifecycle,
                        "indexer_source": lf.indexer_source,
                    },
                },
                "gates": gates_json,
                "trace": trace,
                "failed_gates": failed,
                // REQ-AXO-902585 — les gates qui n'ont PAS PU mesurer, tenus à part
                // des rouges : ce sont deux verdicts différents, et un seul des deux
                // doit déclencher une reprise.
                "unknown_gates": unknown,
                "next_action": action,
                // REQ-AXO-902256 — `promote_live.sh` is deleted; the resume path is a
                // re-run of promote_live_safe.sh, which detects the stranded pending.json
                // and replays the cutover on that build's candidate manifest (and the
                // REQ-AXO-902258 byte check runs on the way through, so a resume cannot
                // re-commit a wrong binary). Handing an operator a command that names a
                // deleted script is the kind of stale instruction this session spent hours
                // paying for.
                "recovery": {
                    "resume": "bash scripts/release/promote_live_safe.sh --project AXO   # auto-resumes a stranded pending via the cutover",
                    "re_promote": "bash scripts/release/promote_live_safe.sh --project AXO",
                    "rollback": "bash scripts/release/rollback_live.sh",
                    "direct_executor": "bin/axonctl cutover --project-root . --instance-kind live --manifest <candidate.json> --max-polls 120 --poll-interval-ms 2000"
                }
            }
        }))
    }
}
