use serde_json::{json, Value};

use super::format::format_standard_contract;
use super::McpServer;

/// REQ-AXO-901961 S2 — retention window for the `axon.mcp_call_stat` rollup.
/// The per-(tool,project,status,bucket_hour) UPSERT is bounded per hour, but
/// `bucket_hour` advances forever; `mcp_telemetry_report` prunes buckets older
/// than this so the table can never grow unbounded over time.
const MCP_CALL_STAT_RETENTION_DAYS: i64 = 90;

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

    /// REQ-AXO-901961 — best-effort per-call telemetry, called for EVERY tool
    /// response at the dispatch chokepoint (S1). Upserts ONE time-bucketed
    /// aggregate row per (tool, project, ok/error, hour) — signature-only, never
    /// argument content (PIL-AXO-9003). Bounded by construction (the rollup IS
    /// the table). Failure-tolerant: a telemetry write must never affect the
    /// tool response (`let _`). Average latency derives from latency_sum_ms /
    /// call_count; latency_max_ms keeps the tail outlier. The observability
    /// surfaces themselves are skipped so they never self-inflate the stats.
    pub(crate) fn record_mcp_call(&self, tool: &str, response: &Value, latency_ms: i64) {
        if tool == "mcp_friction_report" || tool == "mcp_telemetry_report" {
            return;
        }
        let data = response.get("data");
        let problem_class = data
            .and_then(|d| {
                d.pointer("/operator_guidance/problem_class")
                    .or_else(|| d.get("problem_class"))
            })
            .and_then(Value::as_str)
            .unwrap_or("");
        let is_error = response
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || (!problem_class.is_empty() && problem_class != "ok");
        let status = if is_error { "error" } else { "ok" };
        let project_code = data
            .and_then(|d| d.get("project_code"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let build_id =
            std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
        let lm = latency_ms.max(0);
        let _ = self.graph_store.execute_param(
            "INSERT INTO axon.mcp_call_stat (tool, project_code, status, bucket_hour, call_count, latency_sum_ms, latency_max_ms, contract_version)
             VALUES (?, ?, ?, date_trunc('hour', now()), 1, ?, ?, ?)
             ON CONFLICT (tool, project_code, status, bucket_hour)
             DO UPDATE SET call_count = axon.mcp_call_stat.call_count + 1,
                           latency_sum_ms = axon.mcp_call_stat.latency_sum_ms + EXCLUDED.latency_sum_ms,
                           latency_max_ms = greatest(axon.mcp_call_stat.latency_max_ms, EXCLUDED.latency_max_ms),
                           contract_version = EXCLUDED.contract_version",
            &json!([tool, project_code, status, lm, lm, build_id]),
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

    /// REQ-AXO-901961 S3/S4 — `mcp_telemetry_report`: usage + latency analytics
    /// projected from the `axon.mcp_call_stat` rollup. Answers "how is the
    /// system used / average latency / where are the errors" without an external
    /// analytics tool — PG IS the engine. Signature-only by construction (the
    /// rollup never held argument content). avg latency cast to float8 so the
    /// sql-gateway renders it (numeric would hit REQ-AXO-901905's sentinel).
    pub(crate) fn axon_mcp_telemetry_report(&self, args: &Value) -> Option<Value> {
        // REQ-AXO-901961 S2 — bound the rollup over TIME. Prune buckets older
        // than the retention window here, on the operator-invoked report (OFF
        // the per-call hot path). Best-effort: a failed prune never blocks the
        // report.
        let _ = self.graph_store.execute_param(
            "DELETE FROM axon.mcp_call_stat WHERE bucket_hour < now() - make_interval(days => ?)",
            &json!([MCP_CALL_STAT_RETENTION_DAYS]),
        );
        let project_code = args.get("project_code").and_then(Value::as_str).unwrap_or("");
        let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(20).max(1);
        let window_hours = args
            .get("window_hours")
            .and_then(Value::as_i64)
            .unwrap_or(168) // 7 days
            .max(1);

        let rows = self
            .graph_store
            .query_json_param(
                // sum(bigint) → numeric, which the sql-gateway renderer can't
                // decode yet (REQ-AXO-901905) — cast counts to ::bigint and the
                // average to ::float8 so every cell renders as a readable scalar.
                "SELECT tool,
                        sum(call_count)::bigint AS calls,
                        COALESCE(sum(call_count) FILTER (WHERE status='error'), 0)::bigint AS errors,
                        round((sum(latency_sum_ms)::numeric / nullif(sum(call_count),0)), 1)::float8 AS avg_ms,
                        max(latency_max_ms) AS max_ms
                 FROM axon.mcp_call_stat
                 WHERE bucket_hour > now() - make_interval(hours => ?)
                   AND (? = '' OR project_code = ?)
                 GROUP BY tool
                 ORDER BY calls DESC
                 LIMIT ?",
                &json!([window_hours, project_code, project_code, limit]),
            )
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default();

        let cell = |r: &[Value], i: usize| -> String {
            r.get(i)
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };
        let to_i = |s: &str| s.parse::<i64>().unwrap_or(0);

        let mut total_calls = 0i64;
        let mut total_errors = 0i64;
        let mut lines = String::new();
        let tools: Vec<Value> = rows
            .iter()
            .map(|r| {
                let tool = cell(r, 0);
                let calls = to_i(&cell(r, 1));
                let errors = to_i(&cell(r, 2));
                let avg_ms = cell(r, 3);
                let max_ms = cell(r, 4);
                total_calls += calls;
                total_errors += errors;
                let err_pct = if calls > 0 {
                    (errors as f64) * 100.0 / (calls as f64)
                } else {
                    0.0
                };
                lines.push_str(&format!(
                    "| {tool} | {calls} | {errors} ({err_pct:.0}%) | {avg_ms} | {max_ms} |\n"
                ));
                json!({
                    "tool": tool, "calls": calls, "errors": errors,
                    "avg_latency_ms": avg_ms, "max_latency_ms": max_ms,
                })
            })
            .collect();

        let overall_err_pct = if total_calls > 0 {
            (total_errors as f64) * 100.0 / (total_calls as f64)
        } else {
            0.0
        };
        let report = format!(
            "## 📊 MCP Telemetry (last {window_hours}h{})\n\n**Total calls:** {total_calls} · **errors:** {total_errors} ({overall_err_pct:.1}%)\n\n| tool | calls | errors | avg ms | max ms |\n|---|---|---|---|---|\n{lines}\n_Signature-only (tool + ok/error + project) — no argument content. PG-native rollup._",
            if project_code.is_empty() { String::new() } else { format!(", project {project_code}") },
        );

        Some(json!({
            "content": [{ "type": "text", "text": format_standard_contract(
                "ok",
                "mcp usage + latency analytics assembled",
                "scope:mcp_surface",
                &report,
                &["filter by project_code, or widen window_hours, to drill down"],
                "high",
            )}],
            "data": {
                "tools": tools,
                "total_calls": total_calls,
                "total_errors": total_errors,
                "window_hours": window_hours,
                "privacy": "signature-only — no argument content is ever stored",
            }
        }))
    }
}
