use serde_json::{json, Value};

use super::format::format_standard_contract;
use super::McpServer;

impl McpServer {
    /// REQ-AXO-901957 — best-effort friction capture, called for EVERY tool
    /// response on the dispatch chokepoint. Records ONLY the problem SHAPE
    /// (`project_code`, `tool`, `problem_class`, field NAME) — never any
    /// argument content (PIL-AXO-9003 commercial privacy: "Axon improves from
    /// your friction without ever seeing your data"). Failure-tolerant: a
    /// friction-log write must never affect the tool response. Terse successes
    /// (no `problem_class`) are not friction and are skipped.
    pub(crate) fn record_mcp_friction(&self, tool: &str, response: &Value) {
        if tool == "mcp_friction_report" {
            return; // never self-loop on the friction surface itself
        }
        let Some(data) = response.get("data") else {
            return;
        };
        let problem_class = data
            .pointer("/operator_guidance/problem_class")
            .or_else(|| data.get("problem_class"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if problem_class.is_empty() || problem_class == "ok" {
            return;
        }
        // field NAME only — never its value.
        let field_in_error = data
            .pointer("/parameter_repair/invalid_field")
            .or_else(|| data.pointer("/parameter_repair/field"))
            .and_then(Value::as_str)
            .unwrap_or("");
        // project_code is signature metadata (which tenant hit it), not client data.
        let project_code = data
            .get("project_code")
            .and_then(Value::as_str)
            .unwrap_or("");
        let build_id =
            std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
        // Event-sourced upsert (PIL-AXO-9004): one row per distinct signature,
        // occurrence_count + last_observed_at bumped on recurrence. A resolved
        // signature stays `resolved` but its bumped last_observed_at lets the
        // report DERIVE regression (last_observed_at > resolved_at).
        let _ = self.graph_store.execute_param(
            "INSERT INTO axon.mcp_friction (project_code, tool, problem_class, field_in_error, contract_version)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (project_code, tool, problem_class, field_in_error)
             DO UPDATE SET occurrence_count = axon.mcp_friction.occurrence_count + 1,
                           last_observed_at = now(),
                           contract_version = EXCLUDED.contract_version",
            &json!([project_code, tool, problem_class, field_in_error, build_id]),
        );
    }

    /// REQ-AXO-901957 — `mcp_friction_report`: top OPEN friction signatures by
    /// frequency (rollout priorities) + RESOLVED ones with their REQ/VAL links
    /// (traceability), regressions surfaced (resolved but observed since).
    /// Optional `mark_resolved = {id, resolved_by_req, resolved_by_val, note}`
    /// closes a signature against the SOLL fix that resolved it.
    pub(crate) fn axon_mcp_friction_report(&self, args: &Value) -> Option<Value> {
        if let Some(mr) = args.get("mark_resolved") {
            if let Some(id) = mr.get("id").and_then(Value::as_i64) {
                let req = mr.get("resolved_by_req").and_then(Value::as_str).unwrap_or("");
                let val = mr.get("resolved_by_val").and_then(Value::as_str).unwrap_or("");
                let note = mr.get("note").and_then(Value::as_str).unwrap_or("");
                let _ = self.graph_store.execute_param(
                    "UPDATE axon.mcp_friction SET status='resolved', resolved_at=now(),
                       resolved_by_req=NULLIF(?,''), resolved_by_val=NULLIF(?,''), resolution_note=NULLIF(?,'')
                     WHERE id=?",
                    &json!([req, val, note, id]),
                );
            }
        }
        let project_code = args
            .get("project_code")
            .and_then(Value::as_str)
            .unwrap_or("");
        let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(15).max(1);

        let open_rows = self
            .graph_store
            .query_json_param(
                "SELECT id, project_code, tool, problem_class, field_in_error, occurrence_count,
                        contract_version, last_observed_at::text
                 FROM axon.mcp_friction
                 WHERE status = 'open' AND (? = '' OR project_code = ?)
                 ORDER BY occurrence_count DESC, last_observed_at DESC
                 LIMIT ?",
                &json!([project_code, project_code, limit]),
            )
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default();
        let open_frictions: Vec<Value> = open_rows
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.first().cloned().unwrap_or(Value::Null),
                    "project_code": r.get(1).cloned().unwrap_or(Value::Null),
                    "tool": r.get(2).cloned().unwrap_or(Value::Null),
                    "problem_class": r.get(3).cloned().unwrap_or(Value::Null),
                    "field_in_error": r.get(4).cloned().unwrap_or(Value::Null),
                    "occurrence_count": r.get(5).cloned().unwrap_or(Value::Null),
                    "contract_version": r.get(6).cloned().unwrap_or(Value::Null),
                    "last_observed_at": r.get(7).cloned().unwrap_or(Value::Null),
                })
            })
            .collect();

        let resolved_rows = self
            .graph_store
            .query_json_param(
                "SELECT id, tool, problem_class, occurrence_count, COALESCE(resolved_by_req,''),
                        COALESCE(resolved_by_val,''), (last_observed_at > resolved_at)
                 FROM axon.mcp_friction
                 WHERE status = 'resolved' AND (? = '' OR project_code = ?)
                 ORDER BY occurrence_count DESC
                 LIMIT ?",
                &json!([project_code, project_code, limit]),
            )
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default();
        let resolved_frictions: Vec<Value> = resolved_rows
            .into_iter()
            .map(|r| {
                let regressed = r.get(6).map(Self::truthy_cell).unwrap_or(false);
                json!({
                    "id": r.first().cloned().unwrap_or(Value::Null),
                    "tool": r.get(1).cloned().unwrap_or(Value::Null),
                    "problem_class": r.get(2).cloned().unwrap_or(Value::Null),
                    "occurrence_count": r.get(3).cloned().unwrap_or(Value::Null),
                    "resolved_by_req": r.get(4).cloned().unwrap_or(Value::Null),
                    "resolved_by_val": r.get(5).cloned().unwrap_or(Value::Null),
                    "regressed": regressed,
                })
            })
            .collect();

        let open_count = open_frictions.len();
        let regressed_count = resolved_frictions
            .iter()
            .filter(|f| f["regressed"].as_bool() == Some(true))
            .count();
        let report = format!(
            "## 🔁 MCP Friction Report\n\n**Open signatures (rollout priorities):** {}\n**Resolved:** {} ({} regressed since resolution)\n**Privacy:** signature-only — no argument content is ever stored.\n",
            open_count,
            resolved_frictions.len(),
            regressed_count,
        );
        Some(json!({
            "content": [{ "type": "text", "text": format_standard_contract(
                "ok",
                "friction signatures assembled (no argument content stored)",
                "scope:mcp_surface",
                &report,
                &["fix a top-open signature, then call mcp_friction_report mark_resolved={id, resolved_by_req, resolved_by_val} to close the loop"],
                "high",
            )}],
            "data": {
                "open_frictions": open_frictions,
                "resolved_frictions": resolved_frictions,
                "open_count": open_count,
                "regressed_count": regressed_count,
                "privacy": "signature-only — no argument content is ever stored",
            }
        }))
    }

    /// PG may render a boolean as a JSON bool or as the text "t"/"true".
    fn truthy_cell(cell: &Value) -> bool {
        match cell {
            Value::Bool(b) => *b,
            Value::String(s) => matches!(s.as_str(), "t" | "true" | "TRUE" | "1"),
            _ => false,
        }
    }
}
