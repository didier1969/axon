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
    evaluate_gates, evaluate_liveness_gates, liveness_next_action, liveness_phase, next_action,
    phase, LivenessFacts, ReleaseFacts,
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
        let now_ms = Self::now_unix_ms();
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
        let failed: Vec<&str> = gates.iter().filter(|g| !g.pass).map(|g| g.name).collect();
        // Liveness failures take precedence over the release-state phase/action.
        let ph = liveness_phase(&lf).unwrap_or_else(|| phase(&facts));
        let action = liveness_next_action(&lf).or_else(|| next_action(&facts));

        let gates_json: Vec<Value> = gates
            .iter()
            .map(|g| json!({ "name": g.name, "pass": g.pass, "detail": g.detail }))
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
                "none".to_string()
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
                    "qualification_ok": facts.qualification_ok,
                    "pending_present": facts.pending_present,
                    "pending_build_id": facts.pending_build_id,
                    "runtime_contract": facts.runtime_contract,
                    "liveness": {
                        "brain_serving": lf.brain_serving,
                        "indexer_expected": lf.indexer_expected,
                        "indexer_ready": lf.indexer_ready,
                        "indexer_lifecycle": lf.indexer_lifecycle,
                        "indexer_source": lf.indexer_source,
                    },
                },
                "gates": gates_json,
                "failed_gates": failed,
                "next_action": action,
                "recovery": {
                    "resume": "bash scripts/release/promote_live.sh --manifest <candidate> --restart-live --resume",
                    "re_promote": "bash scripts/release/promote_live_safe.sh --project AXO",
                    "rollback": "bash scripts/release/rollback_live.sh"
                }
            }
        }))
    }
}
