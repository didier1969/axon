use serde_json::{json, Value};

use super::format::format_standard_contract;
use super::McpServer;

/// REQ-AXO-901961 S2 — retention window for the `axon.mcp_call_stat` rollup.
/// The per-(tool,project,status,bucket_hour) UPSERT is bounded per hour, but
/// `bucket_hour` advances forever; `mcp_telemetry_report` prunes buckets older
/// than this so the table can never grow unbounded over time.
const MCP_CALL_STAT_RETENTION_DAYS: i64 = 90;

/// REQ-AXO-902297 — is this `problem_class` an actual FAILURE?
///
/// Both the friction log and the per-call telemetry read the same field, and
/// both used to treat "non-empty and not `ok`" as an error. That swept in two
/// classes that are not failures at all:
///
///   * `degraded` (1003 occurrences, all from `query`) — `guidance.rs` emits it
///     with `next_best_actions: ["treat_result_as_partial"]`: the answer IS
///     served, only its quality is flagged. It fires while the index rebuilds
///     after a promote, so a week with six promotes looked like a broken tool.
///   * `none` (157 occurrences) — `cycle_audit.rs` literally emits
///     `if cycle_count == 0 { "none" }`: the PERFECT result. A SOLL audit
///     finding no cycle was being logged as friction and counted as an error.
///
/// Together ~41% of the friction log — the surface used to PRIORITISE work was
/// two fifths noise, and its top entry by volume was not a defect. It also
/// explains the "regressed since resolution" count: a legitimate degraded mode
/// can never stay "resolved", it just keeps being observed.
///
/// Shared by `record_mcp_call` and `record_mcp_friction` on purpose: the two
/// diverging readings of one field are what let this live. The allow-list is
/// deliberately narrow — an UNKNOWN class still counts as a failure, because a
/// new problem class must surface loudly rather than be silently swallowed.
pub(crate) fn problem_class_is_failure(problem_class: &str) -> bool {
    !NON_FAILURE_PROBLEM_CLASSES.contains(&problem_class)
}

/// The allow-list behind `problem_class_is_failure`, hoisted to a constant so the
/// SQL half of the same rule (`failure_class_sql`) cannot drift from the Rust half.
/// REQ-AXO-902310: the regression derivation runs in SQL, and a second hand-written
/// copy of this list is exactly how one reading of a field diverges from another —
/// which is the defect this module already documents above.
pub(crate) const NON_FAILURE_PROBLEM_CLASSES: &[&str] = &["", "ok", "none", "degraded"];

/// REQ-AXO-902319 — the third signature state, next to `open` and `resolved`.
///
/// A signature in this state describes a refusal that is CORRECT and PERMANENT:
/// the tool is right to reject, the caller is right to be told, and no code change
/// will ever make it go away. `open` would keep it at the top of the rollout
/// priorities forever; `resolved` would claim a fix that does not exist AND make
/// the regression rule (REQ-AXO-902310) raise a false alarm on every recurrence.
/// The column has no CHECK constraint, so this needs no DDL migration.
pub(crate) const BY_DESIGN_STATUS: &str = "by_design";

/// SQL predicate equivalent to `problem_class_is_failure`, derived from the same
/// constant. Values are static and alphanumeric, so the quoting is total.
pub(crate) fn failure_class_sql() -> String {
    let list = NON_FAILURE_PROBLEM_CLASSES
        .iter()
        .map(|c| format!("'{c}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("COALESCE(problem_class, '') NOT IN ({list})")
}

/// REQ-AXO-902292 — render the friction signatures INTO the text channel.
///
/// Same class REQ-AXO-902279 fixed for `wiring`/`orphan_clusters`: the rows were
/// already assembled in `data`, and `content[0].text` printed only a count. The
/// tool's own `next_action` says "fix a top-open signature" — which was
/// impossible, because it never named one. An LLM reading the summary had to
/// fall back to raw SQL on `axon.mcp_friction`, and `sql input_invalid` is the
/// #1 open friction: the triage surface was feeding the very signature it exists
/// to retire.
///
/// A table rather than `sample_identities` (the 902279 helper): a friction is
/// four correlated fields, not a name, and priority only reads off the count.
/// Truncation is ALWAYS disclosed against the true total, never the page size.
fn render_friction_rows(rows: &[Value], total: i64, header: &str) -> String {
    if rows.is_empty() {
        return format!("\n**{header}:** none\n");
    }
    let mut out = format!("\n**{header}** (showing {} of {total}):\n\n", rows.len());
    // REQ-AXO-902310 — the regression column exists only where regression is a
    // meaningful state (the resolved section). Naming the row is the point: a bare
    // "N regressed" count tells the reader something broke without telling them
    // WHAT, so the next move is raw SQL — the very fallback this table retired.
    let has_regressed = rows.iter().any(|r| r.get("regressed").is_some());
    if has_regressed {
        out.push_str("| id | tool | problem | field | count | état |\n|---|---|---|---|---|---|\n");
    } else {
        out.push_str("| id | tool | problem | field | count |\n|---|---|---|---|---|\n");
    }
    for r in rows {
        let cell = |k: &str| -> String {
            match r.get(k) {
                Some(Value::String(s)) if !s.is_empty() => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => "—".to_string(),
            }
        };
        let core = format!(
            "| {} | `{}` | {} | `{}` | {} |",
            cell("id"),
            cell("tool"),
            cell("problem_class"),
            cell("field_in_error"),
            cell("occurrence_count"),
        );
        if has_regressed {
            let state = if r["regressed"].as_bool() == Some(true) {
                let by = cell("resolved_by_req");
                format!("⚠️ RÉGRESSÉE — revue depuis {by}")
            } else {
                "fermée".to_string()
            };
            out.push_str(&format!("{core} {state} |\n"));
        } else {
            out.push_str(&format!("{core}\n"));
        }
    }
    if (rows.len() as i64) < total {
        out.push_str(&format!(
            "\n_{} more not shown — raise `limit` to see them._\n",
            total - rows.len() as i64
        ));
    }
    out
}

impl McpServer {
    /// REQ-AXO-901957 — best-effort friction capture, called for EVERY tool
    /// response on the dispatch chokepoint. Records ONLY the problem SHAPE
    /// (`project_code`, `tool`, `problem_class`, field NAME) — never any
    /// argument content (PIL-AXO-9003 commercial privacy: "Axon improves from
    /// your friction without ever seeing your data"). Failure-tolerant: a
    /// friction-log write must never affect the tool response. Terse successes
    /// (no `problem_class`) are not friction and are skipped.
    pub(crate) fn record_mcp_friction(&self, tool: &str, arguments: &Value, response: &Value) {
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
        // REQ-AXO-902297 — a served-but-degraded answer and a clean audit are not
        // frictions; logging them buried the real ones under 41% noise.
        if !problem_class_is_failure(problem_class) {
            return;
        }
        // field NAME only — never its value.
        let field_in_error = data
            .pointer("/parameter_repair/invalid_field")
            .or_else(|| data.pointer("/parameter_repair/field"))
            .and_then(Value::as_str)
            .unwrap_or("");
        // project_code is signature metadata (which tenant hit it), not client data.
        //
        // REQ-AXO-902309 — resolved by CASCADE, not from the response alone. Reading
        // only `data.project_code` left 47 of 68 signatures (2683 of ~2846
        // occurrences) unattributed, because an ERROR response is precisely the one
        // that does not get around to echoing the project scope. The consequence was
        // not a cosmetic gap: `mcp_friction_report project_code=AXO` answered "Open
        // signatures: 0" with 24 open — a filtered report that reads as a clean
        // surface and CLOSES the investigation.
        //
        // The fallbacks are the same scope the tool itself ran under, in decreasing
        // order of explicitness: what the response says → what the caller asked for →
        // what the cwd resolves to. Still signature metadata only; no argument value
        // is ever read here (PIL-AXO-9003).
        let project_code = data
            .get("project_code")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                arguments
                    .get("project_code")
                    .or_else(|| arguments.get("project"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string)
            })
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
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
        if tool == "mcp_friction_report"
            || tool == "mcp_telemetry_report"
            || tool == "mcp_feedback_report"
        {
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
        // REQ-AXO-902297 — same predicate as the friction log, so the two can no
        // longer disagree about what "error" means. `degraded` / `none` are not
        // failures: counting them made `query` read as 22.3% broken while it was
        // serving answers normally.
        let is_error = response
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || problem_class_is_failure(problem_class);
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

    /// REQ-AXO-901966 — voluntary LLM feedback / doléance. The friction log
    /// (`record_mcp_friction`) is SILENT + signature-only; this is the VOLUNTARY,
    /// content-rich complement an LLM calls to self-report a problem it hit
    /// (bug / unclear doc / undocumented / too slow / incomplete / too verbose),
    /// its proposed fix, and its satisfaction. One row per call (append-only,
    /// PIL-AXO-9004); the server stamps `created_at`. NOT a write-to-SOLL path.
    pub(crate) fn axon_mcp_feedback(&self, args: &Value) -> Option<Value> {
        let problem = args
            .get("problem")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if problem.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "Status: input_invalid\n`problem` is required — describe what was wrong / unclear / slow / missing / too verbose." }],
                "data": { "recorded": false, "reason": "problem_required" }
            }));
        }
        const CATEGORIES: &[&str] = &[
            "bug",
            "unclear_doc",
            "undocumented",
            "too_slow",
            "incomplete",
            "too_verbose",
            "other",
        ];
        let category = args.get("category").and_then(Value::as_str).unwrap_or("other");
        let category = if CATEGORIES.contains(&category) {
            category
        } else {
            "other"
        };
        // Severity for triage / prioritisation (operator request): a hard blocker
        // is a graver problem than something that merely wastes tokens.
        //   blocking   = the LLM could NOT complete its task
        //   token_cost = it worked, but cost significant extra tokens / turns
        //   minor      = cosmetic / small annoyance (default)
        const SEVERITIES: &[&str] = &["blocking", "token_cost", "minor"];
        let severity = args.get("severity").and_then(Value::as_str).unwrap_or("minor");
        let severity = if SEVERITIES.contains(&severity) {
            severity
        } else {
            "minor"
        };
        let llm_identity = args.get("llm_identity").and_then(Value::as_str).unwrap_or("");
        let tool = args.get("tool").and_then(Value::as_str).unwrap_or("");
        let project_code = args.get("project_code").and_then(Value::as_str).unwrap_or("");
        let proposed_solution = args
            .get("proposed_solution")
            .and_then(Value::as_str)
            .unwrap_or("");
        // optional 1..5; anything else → NULL. Inlined as a validated integer
        // (never client text), every text field stays parameterised.
        let satisfaction_sql = args
            .get("satisfaction")
            .and_then(Value::as_i64)
            .filter(|n| (1..=5).contains(n))
            .map(|n| n.to_string())
            .unwrap_or_else(|| "NULL".to_string());
        let build_id =
            std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());

        let result = self.graph_store.execute_param(
            &format!(
                "INSERT INTO axon.llm_feedback \
                    (llm_identity, category, severity, tool, project_code, problem, proposed_solution, satisfaction, contract_version) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, {satisfaction_sql}, ?)"
            ),
            &json!([llm_identity, category, severity, tool, project_code, problem, proposed_solution, build_id]),
        );
        match result {
            Ok(()) => Some(json!({
                "content": [{ "type": "text", "text": format!("Status: recorded\nThank you — your feedback (category={category}, severity={severity}) is logged for product optimization. Keep them coming: bugs, unclear docs, slow / verbose / incomplete tools.") }],
                "data": { "recorded": true, "category": category, "severity": severity }
            })),
            Err(e) => Some(json!({
                "content": [{ "type": "text", "text": format!("Status: writer_failed\nFeedback not stored: {e}") }],
                "data": { "recorded": false, "reason": "writer_failed" }
            })),
        }
    }

    /// REQ-AXO-901957 — `mcp_friction_report`: top OPEN friction signatures by
    /// frequency (rollout priorities) + RESOLVED ones with their REQ/VAL links
    /// (traceability), regressions surfaced (resolved but observed since).
    /// Optional `mark_resolved = {id, resolved_by_req, resolved_by_val, note}`
    /// closes a signature against the SOLL fix that resolved it.
    ///
    /// REQ-AXO-902319 — optional `mark_by_design = {id, note}` records the THIRD
    /// state: a refusal that is correct and permanent. See `BY_DESIGN_STATUS`.
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
        // REQ-AXO-902319 — the third state. `open` and `resolved` could not both be
        // wrong, yet for a permanent refusal they were: `open` keeps a signature at
        // the top of the fix-me list forever with nothing to fix, and `resolved` is
        // a false claim that ALSO makes the regression rule of REQ-AXO-902310 cry
        // wolf on every re-observation — inside the very indicator that REQ just
        // made trustworthy. A required field that cannot be derived (`attach_to` on
        // soll_manager create: guessing a parent would write a SOLL edge nobody
        // asked for) is not friction to eliminate. It is a contract, and the log
        // now has a word for it.
        //
        // A `note` is REQUIRED here, unlike on mark_resolved: "by design" without a
        // stated reason is indistinguishable from giving up, and this state is the
        // one that removes a signature from view permanently.
        if let Some(md) = args.get("mark_by_design") {
            let id = md.get("id").and_then(Value::as_i64);
            let note = md
                .get("note")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            match (id, note) {
                (Some(id), Some(note)) => {
                    let req = md.get("resolved_by_req").and_then(Value::as_str).unwrap_or("");
                    let _ = self.graph_store.execute_param(
                        &format!(
                            "UPDATE axon.mcp_friction SET status='{BY_DESIGN_STATUS}', resolved_at=now(),
                               resolved_by_req=NULLIF(?,''), resolution_note=NULLIF(?,'')
                             WHERE id=?"
                        ),
                        &json!([req, note, id]),
                    );
                }
                (Some(_), None) => {
                    return Some(json!({
                        "content": [{ "type": "text", "text":
                            "mark_by_design requires a `note`: state WHY the refusal is correct and \
                             permanent. This state hides the signature from the fix-me list for good — \
                             without a reason it is indistinguishable from giving up." }],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "operator_guidance": { "problem_class": "input_invalid" },
                            "parameter_repair": {
                                "tool": "mcp_friction_report",
                                "invalid_field": "mark_by_design.note",
                                "required_fields": ["mark_by_design.id", "mark_by_design.note"],
                            },
                        }
                    }));
                }
                _ => {}
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

        // REQ-AXO-902310 — a regression is a signature declared fixed and observed
        // AGAIN since, on a class that is genuinely a failure. Both halves matter:
        // without the failure filter, a `degraded`/`none` signature can never stay
        // "resolved" (it keeps being legitimately observed) and would raise a false
        // alarm on every report — the symmetric defect of the one REQ-AXO-902297 fixed.
        let regressed_sql = format!(
            "(status = 'resolved' AND resolved_at IS NOT NULL \
              AND last_observed_at > resolved_at AND {})",
            failure_class_sql()
        );
        let resolved_rows = self
            .graph_store
            .query_json_param(
                &format!(
                    "SELECT id, tool, problem_class, occurrence_count, COALESCE(resolved_by_req,''),
                            COALESCE(resolved_by_val,''), {regressed_sql}, field_in_error
                     FROM axon.mcp_friction
                     WHERE status = 'resolved' AND (? = '' OR project_code = ?)
                     ORDER BY {regressed_sql} DESC, occurrence_count DESC
                     LIMIT ?"
                ),
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
                    "field_in_error": r.get(7).cloned().unwrap_or(Value::Null),
                })
            })
            .collect();

        let open_count = open_frictions.len();
        // REQ-AXO-902292 — the true totals, not the page size. `open_count` is
        // `open_frictions.len()`, i.e. capped by LIMIT: reporting it as "Open
        // signatures: 15" told the reader there were exactly 15 when 15 was
        // merely the page. A count that silently equals the limit is worse than
        // no count — it reads as complete.
        let total_of = |status: &str| -> i64 {
            self.graph_store
                .query_json_param(
                    "SELECT count(*) FROM axon.mcp_friction
                     WHERE status = ? AND (? = '' OR project_code = ?)",
                    &json!([status, project_code, project_code]),
                )
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
                .and_then(|rows| {
                    rows.first()
                        .and_then(|r| r.first())
                        .and_then(Self::i64_cell)
                })
                .unwrap_or(-1)
        };
        let total_open = total_of("open");
        let total_resolved = total_of("resolved");
        let total_by_design = total_of(BY_DESIGN_STATUS);

        // REQ-AXO-902319 — listed, not hidden. The point of the third state is to
        // take these OUT of the fix-me ranking, not out of sight: a reader must be
        // able to check that "by design" was a judgement someone wrote down, and
        // challenge it. Hence the note travels with the row.
        let by_design_rows: Vec<Vec<Value>> = self
            .graph_store
            .query_json_param(
                &format!(
                    "SELECT id, tool, problem_class, field_in_error, occurrence_count, \
                            COALESCE(resolution_note,''), COALESCE(resolved_by_req,'') \
                     FROM axon.mcp_friction \
                     WHERE status = '{BY_DESIGN_STATUS}' AND (? = '' OR project_code = ?) \
                     ORDER BY occurrence_count DESC LIMIT ?"
                ),
                &json!([project_code, project_code, limit]),
            )
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default();
        let by_design_frictions: Vec<Value> = by_design_rows
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.first().cloned().unwrap_or(Value::Null),
                    "tool": r.get(1).cloned().unwrap_or(Value::Null),
                    "problem_class": r.get(2).cloned().unwrap_or(Value::Null),
                    "field_in_error": r.get(3).cloned().unwrap_or(Value::Null),
                    "occurrence_count": r.get(4).cloned().unwrap_or(Value::Null),
                    "reason": r.get(5).cloned().unwrap_or(Value::Null),
                    "resolved_by_req": r.get(6).cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        let by_design_section = if by_design_frictions.is_empty() {
            String::new()
        } else {
            let mut out = format!(
                "\n**Par conception** (refus corrects et permanents — hors priorités de correction ; \
                 showing {} of {total_by_design}):\n\n",
                by_design_frictions.len()
            );
            out.push_str("| id | tool | problem | field | count | pourquoi |\n|---|---|---|---|---|---|\n");
            for f in &by_design_frictions {
                let cell = |k: &str| -> String {
                    match f.get(k) {
                        Some(Value::String(s)) if !s.is_empty() => s.clone(),
                        Some(Value::Number(n)) => n.to_string(),
                        _ => "—".to_string(),
                    }
                };
                out.push_str(&format!(
                    "| {} | `{}` | {} | `{}` | {} | {} |\n",
                    cell("id"),
                    cell("tool"),
                    cell("problem_class"),
                    cell("field_in_error"),
                    cell("occurrence_count"),
                    cell("reason"),
                ));
            }
            out
        };

        // REQ-AXO-902310 — counted over the WHOLE table, not the page. The paged
        // count was the same defect REQ-AXO-902292 fixed for the open/resolved
        // totals, left behind on this one field: a regression sitting past `limit`
        // was reported as "0 regressed", and 0 is the number that stops the reader.
        let scalar = |sql: &str, params: Value| -> i64 {
            self.graph_store
                .query_json_param(sql, &params)
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
                .and_then(|rows| rows.first().and_then(|r| r.first()).and_then(Self::i64_cell))
                .unwrap_or(-1)
        };
        let regressed_count = scalar(
            &format!(
                "SELECT count(*) FROM axon.mcp_friction \
                 WHERE {regressed_sql} AND (? = '' OR project_code = ?)"
            ),
            json!([project_code, project_code]),
        );

        // REQ-AXO-902309 — a filtered report must never present itself as complete
        // while signatures sit in the unattributed bucket. Historic rows (written
        // before the project stamp was cascaded) carry an empty `project_code`; a
        // per-tenant filter drops them silently, which is how "0 open" was read as
        // a clean surface. They are counted and NAMED here instead.
        let unattributed = if project_code.is_empty() {
            0
        } else {
            scalar(
                "SELECT count(*) FROM axon.mcp_friction \
                 WHERE status = 'open' AND COALESCE(project_code, '') = ''",
                json!([]),
            )
        };
        let unattributed_note = if unattributed > 0 {
            format!(
                "\n⚠️ **{unattributed} signature(s) ouverte(s) NON attribuée(s)** — antérieures au \
                 tampon projet (REQ-AXO-902309), donc invisibles pour ce filtre. Le total \
                 ci-dessus n'est PAS la surface complète : rappeler sans `project_code` pour \
                 les voir.\n"
            )
        } else {
            String::new()
        };

        let by_design_line = if total_by_design > 0 {
            format!("\n**Par conception:** {total_by_design} (refus corrects, rien à corriger)")
        } else {
            String::new()
        };
        let report = format!(
            "## 🔁 MCP Friction Report\n\n**Open signatures (rollout priorities):** {}\n\
             **Resolved:** {} ({} regressed since resolution){}\n\
             **Privacy:** signature-only — no argument content is ever stored.\n\
             {}{}{}{}\n_Table: `axon.mcp_friction`._\n",
            total_open,
            total_resolved,
            regressed_count,
            by_design_line,
            unattributed_note,
            render_friction_rows(&open_frictions, total_open, "Open signatures"),
            render_friction_rows(&resolved_frictions, total_resolved, "Resolved"),
            by_design_section,
        );
        Some(json!({
            "content": [{ "type": "text", "text": format_standard_contract(
                "ok",
                "friction signatures assembled (no argument content stored)",
                "scope:mcp_surface",
                &report,
                &["fix a top-open signature, then call mcp_friction_report mark_resolved={id, resolved_by_req, resolved_by_val} to close the loop — or mark_by_design={id, note} when the refusal is correct and permanent"],
                "high",
            )}],
            "data": {
                "open_frictions": open_frictions,
                "resolved_frictions": resolved_frictions,
                "by_design_frictions": by_design_frictions,
                "open_count": open_count,
                "total_open": total_open,
                "total_resolved": total_resolved,
                "total_by_design": total_by_design,
                "regressed_count": regressed_count,
                "unattributed_open": unattributed,
                "privacy": "signature-only — no argument content is ever stored",
            }
        }))
    }

    /// REQ-AXO-902020 — `mcp_feedback_report`: the content-rich READ/triage
    /// counterpart to `mcp_feedback` (write-only until now). Lists voluntary LLM
    /// doléances from `axon.llm_feedback` (problem / proposed_solution /
    /// severity / satisfaction), newest first, OPEN by default. Filters:
    /// `project_code`, `category`, `severity`, `tool`, `window_hours` (default
    /// 168 = 7d), `limit` (default 30), `include_resolved`. Optional
    /// `mark_resolved = {id, resolved_by_req, note}` closes an item against the
    /// SOLL fix — symmetric to `mcp_friction_report`. Closes the write-only gap
    /// (PIL-AXO-002 / PIL-AXO-9003 closed-loop) so triage no longer needs raw SQL.
    pub(crate) fn axon_mcp_feedback_report(&self, args: &Value) -> Option<Value> {
        if let Some(mr) = args.get("mark_resolved") {
            if let Some(id) = mr.get("id").and_then(Value::as_i64) {
                let req = mr.get("resolved_by_req").and_then(Value::as_str).unwrap_or("");
                let note = mr.get("note").and_then(Value::as_str).unwrap_or("");
                let _ = self.graph_store.execute_param(
                    "UPDATE axon.llm_feedback SET triage_status='resolved', resolved_at=now(),
                       resolved_by_req=NULLIF(?,''), resolution_note=NULLIF(?,'')
                     WHERE id=?",
                    &json!([req, note, id]),
                );
            }
        }
        // REQ-AXO-902439 — read ONE item (or a few) in full.
        //
        // Reported by AXO (llm_feedback #213) and paid twice on 2026-08-21 by
        // the author of this very fix: to triage 18 doléances you must READ
        // them, and the list surface clips `problem` at 160 chars and renders
        // `proposed_solution` only for blocking items. The only working path
        // was `sql SELECT problem, proposed_solution FROM axon.llm_feedback` —
        // exactly the raw-SQL fallback GUI-PRO-114 forbids. A triage tool whose
        // documented workflow ends in the forbidden tool is not a triage tool.
        let requested_ids: Vec<i64> = args
            .get("ids")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .collect()
            })
            .or_else(|| args.get("id").and_then(Value::as_i64).map(|id| vec![id]))
            .unwrap_or_default();
        if !requested_ids.is_empty() {
            return self.mcp_feedback_items_in_full(&requested_ids);
        }

        let project_code = args.get("project_code").and_then(Value::as_str).unwrap_or("");
        let category = args.get("category").and_then(Value::as_str).unwrap_or("");
        let severity = args.get("severity").and_then(Value::as_str).unwrap_or("");
        let tool = args.get("tool").and_then(Value::as_str).unwrap_or("");
        let window_hours = args.get("window_hours").and_then(Value::as_i64).unwrap_or(168).max(1);
        let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(30).max(1);
        let include_resolved = args
            .get("include_resolved")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let rows = self
            .graph_store
            .query_json_param(
                "SELECT id, created_at::text, llm_identity, category, severity, tool,
                        project_code, problem, proposed_solution, satisfaction,
                        triage_status, COALESCE(resolved_by_req,'')
                 FROM axon.llm_feedback
                 WHERE created_at > now() - make_interval(hours => ?)
                   AND (? = '' OR project_code = ?)
                   AND (? = '' OR category = ?)
                   AND (? = '' OR severity = ?)
                   AND (? = '' OR tool = ?)
                   AND (? = 1 OR triage_status = 'open')
                 ORDER BY (triage_status = 'open') DESC, created_at DESC
                 LIMIT ?",
                &json!([
                    window_hours,
                    project_code, project_code,
                    category, category,
                    severity, severity,
                    tool, tool,
                    if include_resolved { 1 } else { 0 },
                    limit
                ]),
            )
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default();

        let feedback: Vec<Value> = rows
            .into_iter()
            .map(|r| {
                // PG renders every column as text through query_json_param; coerce
                // id + satisfaction back to numbers so the LLM consumer gets the
                // proper types (id is what mark_resolved expects as an integer).
                let as_i64 = |cell: Option<&Value>| -> Value {
                    cell.and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                        .map(Value::from)
                        .unwrap_or(Value::Null)
                };
                json!({
                    "id": as_i64(r.first()),
                    "created_at": r.get(1).cloned().unwrap_or(Value::Null),
                    "llm_identity": r.get(2).cloned().unwrap_or(Value::Null),
                    "category": r.get(3).cloned().unwrap_or(Value::Null),
                    "severity": r.get(4).cloned().unwrap_or(Value::Null),
                    "tool": r.get(5).cloned().unwrap_or(Value::Null),
                    "project_code": r.get(6).cloned().unwrap_or(Value::Null),
                    "problem": r.get(7).cloned().unwrap_or(Value::Null),
                    "proposed_solution": r.get(8).cloned().unwrap_or(Value::Null),
                    "satisfaction": as_i64(r.get(9)),
                    "triage_status": r.get(10).cloned().unwrap_or(Value::Null),
                    "resolved_by_req": r.get(11).cloned().unwrap_or(Value::Null),
                })
            })
            .collect();

        let open_count = feedback
            .iter()
            .filter(|f| f["triage_status"].as_str() == Some("open"))
            .count();
        let blocking_count = feedback
            .iter()
            .filter(|f| f["severity"].as_str() == Some("blocking"))
            .count();
        // REQ-AXO-902398 — render the ITEMS, not just how many there are.
        //
        // Reported by KKI (llm_feedback #180, blocking): ten items came back as
        // three lines of counters. The rows only ever reached `data.feedback`,
        // which the Claude Code client does not expose to the LLM — the same
        // cause REQ-AXO-902355 closed for the kickoff_bundle. Worse, the
        // documented triage path (`mark_resolved={id,…}`) needs an `id` the
        // report never printed, so the tool told you to do something it made
        // impossible. On 2026-08-21 this produced a FALSE negative fact under a
        // `Status: ok`: asked what KKI had sent, AXO answered "nothing".
        let clip = |v: &Value, max: usize| -> String {
            let s = v.as_str().unwrap_or("").replace(['\n', '|'], " ");
            if s.chars().count() <= max {
                s
            } else {
                format!("{}…", s.chars().take(max).collect::<String>())
            }
        };
        let mut report = format!(
            "## 📨 MCP Feedback Report\n\n**Items (last {}h):** {} ({} open, {} blocking)\n",
            window_hours,
            feedback.len(),
            open_count,
            blocking_count,
        );
        if feedback.is_empty() {
            report.push_str(
                "\n_No item matches these filters — widen `window_hours`, or drop \
                 `project_code`/`severity`/`tool`. An empty list is a filter result, \
                 not proof that nothing was reported._\n",
            );
        } else {
            report.push_str("\n| id | sev | cat | tool | proj | problem |\n|---|---|---|---|---|---|\n");
            for f in &feedback {
                report.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n",
                    f["id"].as_i64().map(|v| v.to_string()).unwrap_or_default(),
                    clip(&f["severity"], 10),
                    clip(&f["category"], 12),
                    clip(&f["tool"], 24),
                    clip(&f["project_code"], 6),
                    clip(&f["problem"], 160),
                ));
            }
            // The blocking ones carry the proposed fix in full-ish: they are the
            // items the triage line asks you to act on first.
            for f in feedback.iter().filter(|f| {
                f["severity"].as_str() == Some("blocking")
                    && f["triage_status"].as_str() == Some("open")
            }) {
                report.push_str(&format!(
                    "\n**#{} · {} — proposed:** {}\n",
                    f["id"].as_i64().map(|v| v.to_string()).unwrap_or_default(),
                    clip(&f["tool"], 32),
                    clip(&f["proposed_solution"], 600),
                ));
            }
        }
        report.push_str(
            "\n**Triage:** fix an item, then `mcp_feedback_report mark_resolved={id, resolved_by_req, note}` to close it.\n",
        );
        Some(json!({
            "content": [{ "type": "text", "text": format_standard_contract(
                "ok",
                "voluntary LLM feedback assembled (content-rich)",
                "scope:mcp_surface",
                &report,
                &["triage the top blocking/open item, then mcp_feedback_report mark_resolved={id, resolved_by_req} to close the loop"],
                "high",
            )}],
            "data": {
                "feedback": feedback,
                "open_count": open_count,
                "blocking_count": blocking_count,
                "window_hours": window_hours,
            }
        }))
    }

    /// REQ-AXO-902439 — the `ids` lane of `mcp_feedback_report`: the named
    /// items, complete, with no clipping of `problem` / `proposed_solution`.
    ///
    /// Bounded by VOLUME rather than by count, because the weight of a doléance
    /// is unknowable before the call (a TE2 report runs 1 500-3 000 chars, a
    /// one-liner runs 80) and `limit` only ever bounded the number. Whatever
    /// does not fit is NAMED by id rather than silently dropped — the same
    /// remedy REQ-AXO-902419 applied to `mcp_inbox_read`.
    fn mcp_feedback_items_in_full(&self, ids: &[i64]) -> Option<Value> {
        // Ids are parsed i64, so this interpolation carries no injectable text.
        let id_list = ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let rows = self
            .graph_store
            .query_json(&format!(
                "SELECT id, created_at::text, llm_identity, category, severity, tool, \
                        project_code, problem, proposed_solution, satisfaction, \
                        triage_status, COALESCE(resolved_by_req,'') \
                 FROM axon.llm_feedback WHERE id IN ({id_list}) ORDER BY id"
            ))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default();

        let cell = |r: &Vec<Value>, i: usize| -> String {
            r.get(i)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let feedback: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "id": cell(r, 0).parse::<i64>().ok(),
                    "created_at": cell(r, 1),
                    "llm_identity": cell(r, 2),
                    "category": cell(r, 3),
                    "severity": cell(r, 4),
                    "tool": cell(r, 5),
                    "project_code": cell(r, 6),
                    "problem": cell(r, 7),
                    "proposed_solution": cell(r, 8),
                    "satisfaction": cell(r, 9).parse::<i64>().ok(),
                    "triage_status": cell(r, 10),
                    "resolved_by_req": cell(r, 11),
                })
            })
            .collect();

        // ~24 KB of text: several full doléances per call, still far from any
        // client-side truncation point.
        const TEXT_BUDGET: usize = 24_000;
        let mut report = String::from("## 📨 MCP Feedback — items in full
");
        let mut rendered = 0usize;
        let mut deferred: Vec<String> = Vec::new();
        for r in &rows {
            let id = cell(r, 0);
            let block = format!(
                "\n---\n\n### #{id} · {sev} · {cat} · `{tool}` · {proj} · {status}\n\
                 _{who}, {when}_\n\n**Problem**\n\n{problem}\n\n**Proposed**\n\n{proposed}\n",
                sev = cell(r, 4),
                cat = cell(r, 3),
                tool = cell(r, 5),
                proj = cell(r, 6),
                status = cell(r, 10),
                who = cell(r, 2),
                when = cell(r, 1),
                problem = cell(r, 7),
                proposed = {
                    let p = cell(r, 8);
                    if p.trim().is_empty() {
                        "_(none supplied)_".to_string()
                    } else {
                        p
                    }
                },
            );
            if rendered > 0 && report.len() + block.len() > TEXT_BUDGET {
                deferred.push(id);
                continue;
            }
            report.push_str(&block);
            rendered += 1;
        }

        let missing: Vec<String> = ids
            .iter()
            .filter(|id| !rows.iter().any(|r| cell(r, 0) == id.to_string()))
            .map(|id| id.to_string())
            .collect();
        if !missing.is_empty() {
            report.push_str(&format!(
                "\n_No such feedback id: {}._\n",
                missing.join(", ")
            ));
        }
        if !deferred.is_empty() {
            report.push_str(&format!(
                "\n_{} item(s) not rendered here to stay under the text budget — \
                 call again with `ids=[{}]`. They are complete in \
                 `data.feedback`._\n",
                deferred.len(),
                deferred.join(", ")
            ));
        }

        Some(json!({
            "content": [{ "type": "text", "text": format_standard_contract(
                "ok",
                "named feedback items rendered in full",
                "scope:mcp_surface",
                &report,
                &["fix an item, then mcp_feedback_report mark_resolved={id, resolved_by_req, note}"],
                "high",
            )}],
            "data": {
                "feedback": feedback,
                "requested_ids": ids,
                "rendered_in_text": rendered,
                "deferred_ids": deferred,
                "unknown_ids": missing,
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

    /// REQ-AXO-902292 — same story for integers: a PG `count(*)` comes back as a
    /// JSON number on one path and as text on another (bigint is rendered as a
    /// string by several drivers). Reading only `as_i64` silently yielded `None`,
    /// which a `unwrap_or(-1)` would then print as a total of -1.
    fn i64_cell(cell: &Value) -> Option<i64> {
        match cell {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.trim().parse::<i64>().ok(),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn i64_cell_for_tests(cell: &Value) -> Option<i64> {
        Self::i64_cell(cell)
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

#[cfg(test)]
mod failure_classification_tests {
    use super::problem_class_is_failure;

    // REQ-AXO-902297 — a served answer and a clean audit are not failures.
    #[test]
    fn served_but_degraded_is_not_a_failure() {
        assert!(
            !problem_class_is_failure("degraded"),
            "`degraded` ships the answer and only flags its quality \
             (guidance.rs: treat_result_as_partial) — counting it as an error made \
             `query` read as 22.3% broken while it was working"
        );
    }

    #[test]
    fn the_literal_none_class_is_not_a_failure() {
        assert!(
            !problem_class_is_failure("none"),
            "`cycle_audit.rs` emits \"none\" when it finds ZERO cycles — the perfect \
             result. It was being logged as friction and counted as an error"
        );
    }

    #[test]
    fn absent_and_ok_are_not_failures() {
        assert!(!problem_class_is_failure(""));
        assert!(!problem_class_is_failure("ok"));
    }

    #[test]
    fn real_problem_classes_still_count_as_failures() {
        for pc in [
            "input_invalid",
            "invalid_arguments",
            "unknown_tool",
            "wrong_project_scope",
            "git_add_rejected_paths",
            "cycle_present_in_soll",
        ] {
            assert!(problem_class_is_failure(pc), "`{pc}` is a genuine failure");
        }
    }

    #[test]
    fn an_unknown_class_is_treated_as_a_failure() {
        // The allow-list is narrow ON PURPOSE: a problem class nobody has
        // classified yet must surface loudly, never be swallowed as benign.
        assert!(problem_class_is_failure("some_future_class"));
    }
}

#[cfg(test)]
mod friction_rendering_tests {
    use super::{render_friction_rows, McpServer};
    use serde_json::json;

    fn row(id: i64, tool: &str, problem: &str, field: &str, count: i64) -> serde_json::Value {
        json!({
            "id": id, "tool": tool, "problem_class": problem,
            "field_in_error": field, "occurrence_count": count,
        })
    }

    // REQ-AXO-902292 — the summary must NAME the signatures, not just count them.
    #[test]
    fn signatures_are_enumerated_in_the_text_channel() {
        let rows = vec![
            row(1, "sql", "input_invalid", "", 459),
            row(2, "soll_manager", "input_invalid", "data.status", 57),
        ];
        let out = render_friction_rows(&rows, 2, "Open signatures");

        for needle in ["sql", "soll_manager", "input_invalid", "data.status", "459", "57"] {
            assert!(
                out.contains(needle),
                "the text channel must carry `{needle}` — an LLM that reads only \
                 content[0].text cannot triage a count, and falls back to raw SQL"
            );
        }
        assert!(!out.contains("more not shown"), "nothing was truncated here");
    }

    // Truncation is disclosed against the TRUE total, never the page size — the
    // defect that made "Open signatures: 15" read as complete when 15 was the limit.
    #[test]
    fn truncation_is_disclosed_against_the_true_total() {
        let rows = vec![row(1, "sql", "input_invalid", "", 459)];
        let out = render_friction_rows(&rows, 40, "Open signatures");

        assert!(out.contains("showing 1 of 40"), "the page must state its own bounds: {out}");
        assert!(out.contains("39 more not shown"), "the remainder must be named: {out}");
    }

    #[test]
    fn an_empty_section_says_none_rather_than_printing_an_empty_table() {
        let out = render_friction_rows(&[], 0, "Open signatures");
        assert!(out.contains("none"), "{out}");
        assert!(!out.contains("| id |"), "no header for an empty set: {out}");
    }

    // A PG `count(*)` arrives as a JSON number on one path and as text on another;
    // reading only `as_i64` yielded None and printed a total of -1.
    #[test]
    fn counts_parse_from_both_json_number_and_pg_text() {
        assert_eq!(McpServer::i64_cell_for_tests(&json!(42)), Some(42));
        assert_eq!(McpServer::i64_cell_for_tests(&json!("42")), Some(42));
        assert_eq!(McpServer::i64_cell_for_tests(&json!("  42 ")), Some(42));
        assert_eq!(McpServer::i64_cell_for_tests(&json!("oops")), None);
        assert_eq!(McpServer::i64_cell_for_tests(&json!(null)), None);
    }
}
