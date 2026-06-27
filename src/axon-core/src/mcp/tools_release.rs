//! REQ-AXO-902111 / DEC-AXO-901662 — `promote_status` MCP tool (T1 read-only).
//!
//! Thin surface over [`crate::release_reconciler`]: collects release facts and
//! returns `{phase, observed, gates, failed_gates, next_action, recovery}` so an
//! agent reads the release truth in one call instead of grepping the promote
//! scripts. Read-only — it never mutates runtime or release state.

use serde_json::{json, Value};

use super::McpServer;
use crate::release_reconciler::{evaluate_gates, next_action, phase, ReleaseFacts};

impl McpServer {
    pub(crate) fn axon_promote_status(&self, _args: &Value) -> Option<Value> {
        let live_build_id = std::env::var("AXON_BUILD_ID").unwrap_or_default();
        let release_dir = std::env::current_dir()
            .unwrap_or_default()
            .join(".axon")
            .join("live-release");
        let facts = ReleaseFacts::collect(&release_dir, live_build_id);
        let gates = evaluate_gates(&facts);
        let failed: Vec<&str> = gates.iter().filter(|g| !g.pass).map(|g| g.name).collect();
        let ph = phase(&facts);
        let action = next_action(&facts);

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
