// REQ-AXO-91580/91581 — MCP tool surface for the SKI + PRT methodology
// platform. Implements `skill_list`, `skill_invoke`, `prompt_template_get`.
// These tools consume the SOLL entity types added by REQ-AXO-91578 (SKI)
// and REQ-AXO-91579 (PRT) and expose them as the canonical Axon-produit
// methodology surface per PIL-AXO-9003 Two-Sided Identity + CPT-AXO-90019
// triad (GUI rules / SKI procedures / PRT templates).
//
// All three tools resolve from the RAM-resident SOLL snapshot (PIL-AXO-9002)
// when fresh ; fall back to PG OLTP read otherwise. Expected localhost
// latency : ~1-3ms (RAM hashmap lookup + JSON marshal). See CPT-AXO-90018
// (re-anchor pattern) for the LLM-autonomy story this surface enables.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use super::McpServer;

// REQ-AXO-91583 slice 2 — in-process ring buffer logging recent skill_invoke
// calls. Used by methodology_drift_warnings to compute the diff
// `mandated_skills - invoked_in_last_K_turns` and surface drift via status().
// Per-process, capped at 256 entries (rolling). Reset on brain restart by
// design — a fresh process is a fresh session.
const SKILL_AUDIT_RING_CAP: usize = 256;

#[derive(Clone, Debug)]
pub(crate) struct SkillInvocationAuditEntry {
    pub(crate) id: String,
    pub(crate) at_unix_ms: u128,
}

fn skill_audit_ring() -> &'static Mutex<VecDeque<SkillInvocationAuditEntry>> {
    static RING: OnceLock<Mutex<VecDeque<SkillInvocationAuditEntry>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(SKILL_AUDIT_RING_CAP)))
}

pub(crate) fn record_skill_invocation(id: &str) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut ring) = skill_audit_ring().lock() {
        if ring.len() >= SKILL_AUDIT_RING_CAP {
            ring.pop_front();
        }
        ring.push_back(SkillInvocationAuditEntry {
            id: id.to_string(),
            at_unix_ms: now_ms,
        });
    }
}

pub(crate) fn recent_skill_invocations(window_ms: u128) -> Vec<SkillInvocationAuditEntry> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(ring) = skill_audit_ring().lock() {
        let threshold = now_ms.saturating_sub(window_ms);
        ring.iter()
            .filter(|entry| entry.at_unix_ms >= threshold)
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}

// REQ-AXO-91581 slice 2 — typed parameter validation + Mustache rendering
// helpers backing `axon_prompt_template_get`. Kept module-private + free
// functions so unit tests can exercise the validation/rendering pipeline
// without spinning up a full MCP server.

fn json_type_label(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(declared: &str, value: &Value) -> bool {
    let declared = declared.to_ascii_lowercase();
    match declared.as_str() {
        "string" | "str" => value.is_string(),
        "number" | "num" | "float" => value.is_number(),
        "integer" | "int" => value.as_i64().is_some() || value.as_u64().is_some(),
        "bool" | "boolean" => value.is_boolean(),
        "list" | "array" => value.is_array(),
        "object" | "map" | "dict" => value.is_object(),
        // Unknown declared type — accept any value (forward-compat).
        _ => true,
    }
}

/// Validates `supplied` against the typed `parameters` sidecar from
/// `metadata.parameters` and returns the effective param map (with
/// declared defaults applied) plus a list of validation errors. When the
/// error list is non-empty the caller MUST short-circuit and surface a
/// `parameter_repair` envelope rather than render the template.
pub(crate) fn validate_and_resolve_prompt_params(
    spec: &[Value],
    supplied: &Value,
) -> (Value, Vec<Value>) {
    let supplied_map = supplied.as_object().cloned().unwrap_or_default();
    let mut effective = supplied_map.clone();
    let mut errors: Vec<Value> = Vec::new();

    for entry in spec {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            errors.push(json!({
                "rule": "spec_invalid",
                "hint": "each metadata.parameters entry must declare `name` (string)",
                "spec_entry": entry,
            }));
            continue;
        };
        let declared_type = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string");
        let required = entry
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let default = entry.get("default").cloned();
        let validation_rule = entry.get("validation_rule").and_then(Value::as_str);

        let supplied_value = supplied_map.get(name).cloned();

        let value_to_check = match supplied_value {
            Some(v) => Some(v),
            None => {
                if let Some(d) = default.clone() {
                    effective.insert(name.to_string(), d.clone());
                    Some(d)
                } else if required {
                    errors.push(json!({
                        "param": name,
                        "rule": "required_missing",
                        "expected_type": declared_type,
                        "hint": format!("supply `params.{}` (declared required, no default)", name),
                    }));
                    None
                } else {
                    None
                }
            }
        };

        if let Some(value) = value_to_check {
            if !type_matches(declared_type, &value) {
                errors.push(json!({
                    "param": name,
                    "rule": "type_mismatch",
                    "expected_type": declared_type,
                    "actual_type": json_type_label(&value),
                    "hint": format!("supply `params.{}` as `{}`", name, declared_type),
                }));
                continue;
            }
            if let (Some(pattern), Some(text)) = (validation_rule, value.as_str()) {
                match regex::Regex::new(pattern) {
                    Ok(re) => {
                        if !re.is_match(text) {
                            errors.push(json!({
                                "param": name,
                                "rule": "validation_rule_violated",
                                "pattern": pattern,
                                "supplied_value": text,
                                "hint": format!("`params.{}` must match regex `{}`", name, pattern),
                            }));
                        }
                    }
                    Err(re_err) => {
                        errors.push(json!({
                            "param": name,
                            "rule": "spec_invalid",
                            "pattern": pattern,
                            "hint": format!(
                                "validation_rule in metadata.parameters is not a valid regex: {}",
                                re_err
                            ),
                        }));
                    }
                }
            }
        }
    }

    (Value::Object(effective), errors)
}

/// Renders `template` through the `mustache` crate using `params` as the
/// substitution scope. Errors propagate as `String` so the MCP envelope
/// can surface them without leaking crate-specific types.
pub(crate) fn render_mustache_template(
    template: &str,
    params: &Value,
) -> Result<String, String> {
    let compiled = mustache::compile_str(template)
        .map_err(|e| format!("compile error: {}", e))?;
    compiled
        .render_to_string(params)
        .map_err(|e| format!("render error: {}", e))
}

impl McpServer {
    /// REQ-AXO-91580 — `mcp__axon__skill_list(applicable_to?, mode_filter?, project_code?)`.
    ///
    /// Returns the list of SKI nodes available for invocation. Filterable by
    /// `applicable_to` (intersection with metadata.applicable_to array) and
    /// by `mode_filter` (one of MANDATED / RECOMMENDED / OPTIONAL — matches
    /// metadata.invocation_mode). When project_code is omitted, defaults to
    /// `PRO` (cross-tenant methodology surface). Cheap discovery call —
    /// the LLM should call this FIRST in a session before invoking skills.
    pub(crate) fn axon_skill_list(&self, arguments: &Value) -> Option<Value> {
        let project_code = arguments
            .get("project_code")
            .and_then(Value::as_str)
            .unwrap_or("PRO")
            .to_string();
        let applicable_to = arguments
            .get("applicable_to")
            .and_then(Value::as_str)
            .map(String::from);
        let mode_filter = arguments
            .get("mode_filter")
            .and_then(Value::as_str)
            .map(|s| s.to_ascii_uppercase());

        let escaped_code = project_code.replace('\'', "''");
        let sql = format!(
            "SELECT id, COALESCE(title, ''), COALESCE(description, ''), COALESCE(status, 'current'), COALESCE(metadata::text, '{{}}') \
             FROM soll.Node \
             WHERE type='Skill' AND project_code='{}' \
             ORDER BY id",
            escaped_code
        );
        let raw = match self.graph_store.query_json(&sql) {
            Ok(value) => value,
            Err(err) => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("skill_list: PG read failed: {}", err)
                    }],
                    "isError": true,
                    "data": {
                        "status": "tool_error",
                        "operator_guidance": {
                            "problem_class": "soll_read_failure",
                            "follow_up_tools": ["status", "sql"],
                            "confidence": "medium",
                        }
                    }
                }));
            }
        };
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();

        let mut skills: Vec<Value> = Vec::new();
        for row in rows {
            if row.len() < 5 {
                continue;
            }
            let id = row[0].clone();
            let title = row[1].clone();
            let description = row[2].clone();
            let status = row[3].clone();
            let metadata: Value =
                serde_json::from_str(&row[4]).unwrap_or_else(|_| json!({}));

            // Optional filter — invocation_mode (MANDATED / RECOMMENDED / OPTIONAL).
            if let Some(mode) = &mode_filter {
                let mode_value = metadata
                    .get("invocation_mode")
                    .and_then(Value::as_str)
                    .unwrap_or("OPTIONAL")
                    .to_ascii_uppercase();
                if &mode_value != mode {
                    continue;
                }
            }

            // Optional filter — applicable_to intersection.
            if let Some(applicable) = &applicable_to {
                let candidate_array = metadata
                    .get("applicable_to")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let candidates: Vec<String> = candidate_array
                    .into_iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                if !candidates.iter().any(|c| c == applicable) {
                    continue;
                }
            }

            skills.push(json!({
                "id": id,
                "title": title,
                "description": description,
                "status": status,
                "metadata": metadata,
            }));
        }

        let pretty = format!(
            "## 🛠️ Skills in `{}` ({} match{}{}{})\n\n{}",
            project_code,
            skills.len(),
            if skills.len() == 1 { "" } else { "es" },
            mode_filter
                .as_ref()
                .map(|m| format!(" · mode={}", m))
                .unwrap_or_default(),
            applicable_to
                .as_ref()
                .map(|a| format!(" · applicable_to={}", a))
                .unwrap_or_default(),
            if skills.is_empty() {
                "(no skills match this filter — try `skill_list` without filters to see the full catalogue)".to_string()
            } else {
                skills
                    .iter()
                    .map(|s| {
                        format!(
                            "- **{}** [{}] · {}",
                            s.get("id").and_then(Value::as_str).unwrap_or(""),
                            s.get("status").and_then(Value::as_str).unwrap_or(""),
                            s.get("title").and_then(Value::as_str).unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        );

        Some(json!({
            "content": [{ "type": "text", "text": pretty }],
            "data": {
                "status": "ok",
                "project_code": project_code,
                "count": skills.len(),
                "skills": skills,
            }
        }))
    }

    /// REQ-AXO-91580 — `mcp__axon__skill_invoke(id, context?)`.
    ///
    /// Resolves a SKI node by canonical id (`SKI-{P}-N`) and returns its
    /// body + metadata. The LLM is expected to read the body and execute
    /// the procedure according to its `invocation_mode` semantics. The
    /// optional `context` argument is opaque — it is captured in the
    /// response for audit (future : feed into mid-task drift telemetry).
    ///
    /// For names like `tdd` / `grill-me` rather than canonical ids, the
    /// LLM should first call `skill_list` to discover the id mapping.
    /// Future iteration : accept `name` argument with metadata.name lookup.
    pub(crate) fn axon_skill_invoke(&self, arguments: &Value) -> Option<Value> {
        let id = match arguments.get("id").and_then(Value::as_str) {
            Some(value) if !value.trim().is_empty() => value.trim().to_string(),
            _ => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": "skill_invoke: required `id` (canonical SKI-{PROJECT}-N) is missing. Call `skill_list` to discover available skills."
                    }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "operator_guidance": {
                            "problem_class": "required_field_missing",
                            "follow_up_tools": ["skill_list"],
                            "confidence": "high",
                        },
                        "parameter_repair": {
                            "tool": "skill_invoke",
                            "category": "required_field_missing",
                            "invalid_field": "id",
                            "hint": "supply a canonical SKI id (e.g. SKI-PRO-001) ; call `skill_list` to enumerate",
                            "follow_up_tools": ["skill_list"],
                        }
                    }
                }));
            }
        };
        if !id.starts_with("SKI-") {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!("skill_invoke: id `{}` is not a canonical SKI identifier (must start with `SKI-`).", id)
                }],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "parameter_repair": {
                        "tool": "skill_invoke",
                        "category": "non_canonical_id_prefix",
                        "invalid_field": "id",
                        "supplied_value": id,
                        "hint": "use `SKI-{PROJECT}-N` format ; call `skill_list` to enumerate",
                    }
                }
            }));
        }

        let escaped_id = id.replace('\'', "''");
        let sql = format!(
            "SELECT id, COALESCE(title, ''), COALESCE(description, ''), COALESCE(status, 'current'), COALESCE(metadata::text, '{{}}'), COALESCE(project_code, '') \
             FROM soll.Node \
             WHERE type='Skill' AND id='{}'",
            escaped_id
        );
        let raw = match self.graph_store.query_json(&sql) {
            Ok(value) => value,
            Err(err) => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("skill_invoke: PG read failed: {}", err)
                    }],
                    "isError": true,
                    "data": {
                        "status": "tool_error",
                    }
                }));
            }
        };
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let row = match rows.into_iter().next() {
            Some(r) if r.len() >= 6 => r,
            _ => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("skill_invoke: skill `{}` not found in SOLL. Call `skill_list` to enumerate available skills.", id)
                    }],
                    "isError": true,
                    "data": {
                        "status": "not_found",
                        "operator_guidance": {
                            "problem_class": "skill_not_found",
                            "follow_up_tools": ["skill_list"],
                            "confidence": "high",
                        }
                    }
                }));
            }
        };

        let title = row[1].clone();
        let body = row[2].clone();
        let status = row[3].clone();
        let metadata: Value = serde_json::from_str(&row[4]).unwrap_or_else(|_| json!({}));
        let project_code = row[5].clone();

        let context = arguments.get("context").cloned().unwrap_or_else(|| json!({}));

        // REQ-AXO-91583 slice 2 — record this invocation in the per-process
        // ring buffer so methodology_drift_warnings can compute real diffs.
        record_skill_invocation(&id);

        let display = format!(
            "## 🛠️ Skill `{}` — {}\n\n**Status** : {} · **Project** : {}\n\n{}",
            id, title, status, project_code, body
        );

        Some(json!({
            "content": [{ "type": "text", "text": display }],
            "data": {
                "status": "ok",
                "id": id,
                "title": title,
                "body": body,
                "status_field": status,
                "project_code": project_code,
                "metadata": metadata,
                "invocation_context": context,
            }
        }))
    }

    /// REQ-AXO-91581 — `mcp__axon__prompt_template_get(id, params?)`.
    ///
    /// Resolves a PRT node by canonical id, validates supplied `params`
    /// against the `metadata.parameters` typed sidecar (CPT-AXO-90017),
    /// applies declared defaults, then renders the body through the
    /// Mustache logic-less engine. Validation errors are surfaced as
    /// `isError: true` with a structured `parameter_repair` envelope so
    /// the LLM can self-correct without a second round-trip.
    ///
    /// Slice 2 scope (delivered) :
    ///   - required-field enforcement
    ///   - JSON-type check (string/number/boolean/integer/array/object)
    ///   - default-value application when caller omits a declared param
    ///   - `validation_rule` regex enforcement (strings only)
    ///   - Mustache rendering via the `mustache` crate
    ///
    /// Deferred (slice 3+) : enum/list-of-allowed-values, JSON-schema
    /// fragments, expected_output.schema validation, golden_examples CI
    /// gate. PRTs with no `metadata.parameters` array render through
    /// Mustache unconditionally (backwards-compat with raw passthrough).
    pub(crate) fn axon_prompt_template_get(&self, arguments: &Value) -> Option<Value> {
        let id = match arguments.get("id").and_then(Value::as_str) {
            Some(value) if !value.trim().is_empty() => value.trim().to_string(),
            _ => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": "prompt_template_get: required `id` (canonical PRT-{PROJECT}-N) is missing."
                    }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "parameter_repair": {
                            "tool": "prompt_template_get",
                            "category": "required_field_missing",
                            "invalid_field": "id",
                            "hint": "supply a canonical PRT id (e.g. PRT-PRO-001)",
                        }
                    }
                }));
            }
        };
        if !id.starts_with("PRT-") {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!("prompt_template_get: id `{}` is not a canonical PRT identifier (must start with `PRT-`).", id)
                }],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                }
            }));
        }

        let escaped_id = id.replace('\'', "''");
        let sql = format!(
            "SELECT id, COALESCE(title, ''), COALESCE(description, ''), COALESCE(status, 'current'), COALESCE(metadata::text, '{{}}'), COALESCE(project_code, '') \
             FROM soll.Node \
             WHERE type='PromptTemplate' AND id='{}'",
            escaped_id
        );
        let raw = match self.graph_store.query_json(&sql) {
            Ok(value) => value,
            Err(err) => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("prompt_template_get: PG read failed: {}", err)
                    }],
                    "isError": true,
                    "data": { "status": "tool_error" }
                }));
            }
        };
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let row = match rows.into_iter().next() {
            Some(r) if r.len() >= 6 => r,
            _ => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("prompt_template_get: PRT `{}` not found in SOLL.", id)
                    }],
                    "isError": true,
                    "data": { "status": "not_found" }
                }));
            }
        };

        let title = row[1].clone();
        let body_template = row[2].clone();
        let status = row[3].clone();
        let metadata: Value = serde_json::from_str(&row[4]).unwrap_or_else(|_| json!({}));
        let project_code = row[5].clone();
        let supplied_params = arguments.get("params").cloned().unwrap_or_else(|| json!({}));

        // Slice 2 — typed parameter validation + Mustache rendering.
        let spec_array = metadata
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let (effective_params, validation_errors) =
            validate_and_resolve_prompt_params(&spec_array, &supplied_params);
        if !validation_errors.is_empty() {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "prompt_template_get: parameter validation failed for PRT `{}` ({} error{}). See data.parameter_repair.errors.",
                        id,
                        validation_errors.len(),
                        if validation_errors.len() == 1 { "" } else { "s" }
                    )
                }],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "id": id,
                    "parameter_repair": {
                        "tool": "prompt_template_get",
                        "category": "param_validation_failed",
                        "errors": validation_errors,
                        "hint": "fix each entry in `errors` and retry ; consult metadata.parameters for the expected sidecar contract (CPT-AXO-90017)",
                    }
                }
            }));
        }

        let rendered_text = match render_mustache_template(&body_template, &effective_params) {
            Ok(text) => text,
            Err(err) => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "prompt_template_get: Mustache rendering failed for PRT `{}`: {}",
                            id, err
                        )
                    }],
                    "isError": true,
                    "data": {
                        "status": "tool_error",
                        "id": id,
                        "parameter_repair": {
                            "tool": "prompt_template_get",
                            "category": "mustache_render_failed",
                            "rendering_engine": "mustache_v1",
                            "hint": "check the template body for unbalanced `{{` / `}}` or invalid Mustache syntax",
                        }
                    }
                }));
            }
        };

        let display = format!(
            "## 📝 Prompt template `{}` — {}\n\n**Status** : {} · **Project** : {} · **Engine** : mustache_v1\n\n```\n{}\n```",
            id, title, status, project_code, rendered_text
        );

        Some(json!({
            "content": [{ "type": "text", "text": display }],
            "data": {
                "status": "ok",
                "id": id,
                "title": title,
                "rendered_text": rendered_text,
                "body_template": body_template,
                "params_used": effective_params,
                "params_supplied": supplied_params,
                "status_field": status,
                "project_code": project_code,
                "metadata": metadata,
                "rendering_engine": "mustache_v1",
            }
        }))
    }

    /// REQ-AXO-91582 — `mcp__axon__re_anchor(reason?, project_code?)`.
    ///
    /// Single-call "where am I" packet for LLM autonomy + memory refresh.
    /// Per CPT-AXO-90018 (re-anchor pattern), returns the canonical state
    /// snapshot an LLM needs to recover orientation after context drift,
    /// long pause, or compact. Replaces 4-6 sequential MCP calls (status +
    /// soll_query_context + soll_work_plan + session_pointer read) with
    /// one envelope. Cheap (~10ms localhost via SOLL-RAM) so the LLM can
    /// invoke it periodically without economic penalty.
    ///
    /// Returned packet :
    ///   - `active_methodology` : current Pillars + recent Decisions
    ///   - `mandated_skills` : SKI nodes with invocation_mode='MANDATED'
    ///   - `recent_revisions` : last N soll.Revision rows for the project
    ///   - `session_pointer` : body of the canonical CPT-{P}-NNN session_pointer
    ///   - `work_plan_top` : top of soll_work_plan (unblockers)
    ///   - `reason` : echo of caller's reason (for audit / telemetry)
    pub(crate) fn axon_re_anchor(&self, arguments: &Value) -> Option<Value> {
        let reason = arguments
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("unspecified")
            .to_string();
        let project_code = arguments
            .get("project_code")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| "AXO".to_string());
        let escaped_code = project_code.replace('\'', "''");

        let query_string = |sql: &str| -> Vec<Vec<String>> {
            self.graph_store
                .query_json(sql)
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
                .unwrap_or_default()
        };

        // Active Pillars + recent Decisions.
        let pillar_rows = query_string(&format!(
            "SELECT id, COALESCE(title, ''), COALESCE(status, '') \
             FROM soll.Node \
             WHERE type='Pillar' AND status='current' AND project_code='{}' \
             ORDER BY id",
            escaped_code
        ));
        let pillars: Vec<Value> = pillar_rows
            .iter()
            .filter(|r| r.len() >= 3)
            .map(|r| {
                json!({ "id": r[0], "title": r[1], "status": r[2] })
            })
            .collect();

        let decision_rows = query_string(&format!(
            "SELECT id, COALESCE(title, ''), COALESCE(status, '') \
             FROM soll.Node \
             WHERE type='Decision' AND status IN ('current','delivered') AND project_code='{}' \
             ORDER BY id DESC LIMIT 10",
            escaped_code
        ));
        let recent_decisions: Vec<Value> = decision_rows
            .iter()
            .filter(|r| r.len() >= 3)
            .map(|r| json!({ "id": r[0], "title": r[1], "status": r[2] }))
            .collect();

        // MANDATED skills (and OPTIONAL/RECOMMENDED — operator can filter).
        let skill_rows = query_string(&format!(
            "SELECT id, COALESCE(title, ''), COALESCE(metadata::text, '{{}}') \
             FROM soll.Node \
             WHERE type='Skill' AND status='current' AND project_code='{}' \
             ORDER BY id",
            escaped_code
        ));
        let mut mandated_skills: Vec<Value> = Vec::new();
        for row in &skill_rows {
            if row.len() < 3 {
                continue;
            }
            let metadata: Value = serde_json::from_str(&row[2]).unwrap_or_else(|_| json!({}));
            let mode = metadata
                .get("invocation_mode")
                .and_then(Value::as_str)
                .unwrap_or("OPTIONAL")
                .to_ascii_uppercase();
            mandated_skills.push(json!({
                "id": row[0],
                "title": row[1],
                "invocation_mode": mode,
                "applicable_to": metadata.get("applicable_to").cloned().unwrap_or(json!([])),
            }));
        }

        // Recent SOLL revisions (last 10).
        let revision_rows = query_string(&format!(
            "SELECT revision_id, COALESCE(summary, ''), committed_at \
             FROM soll.Revision \
             WHERE project_code='{}' \
             ORDER BY committed_at DESC LIMIT 10",
            escaped_code
        ));
        let recent_revisions: Vec<Value> = revision_rows
            .iter()
            .filter(|r| r.len() >= 3)
            .map(|r| {
                json!({
                    "revision_id": r[0],
                    "summary": r[1],
                    "committed_at": r[2],
                })
            })
            .collect();

        // Session pointer body — `CPT-{P}-NNN` canonical (default CPT-AXO-052 for AXO).
        let pointer_id = format!("CPT-{}-052", project_code);
        let pointer_rows = query_string(&format!(
            "SELECT id, COALESCE(title, ''), COALESCE(description, '') \
             FROM soll.Node \
             WHERE id='{}'",
            pointer_id.replace('\'', "''")
        ));
        let session_pointer = pointer_rows
            .iter()
            .filter(|r| r.len() >= 3)
            .map(|r| {
                json!({
                    "id": r[0],
                    "title": r[1],
                    "body": r[2],
                })
            })
            .next()
            .unwrap_or(json!(null));

        // Work plan top (just IDs + titles — full scoring lives in soll_work_plan).
        let work_plan_rows = query_string(&format!(
            "SELECT id, COALESCE(title, ''), COALESCE(status, ''), COALESCE(type, '') \
             FROM soll.Node \
             WHERE type IN ('Requirement','Milestone') \
               AND status='current' AND project_code='{}' \
             ORDER BY id DESC LIMIT 8",
            escaped_code
        ));
        let work_plan_top: Vec<Value> = work_plan_rows
            .iter()
            .filter(|r| r.len() >= 4)
            .map(|r| {
                json!({
                    "id": r[0],
                    "title": r[1],
                    "status": r[2],
                    "type": r[3],
                })
            })
            .collect();

        let summary_text = format!(
            "## 🧭 Re-anchor `{}` (reason: {})\n\n\
             - **Active Pillars** : {} ({})\n\
             - **Recent Decisions** : {} (last 10 current+delivered)\n\
             - **MANDATED-tagged Skills** : {} (full list in `data.mandated_skills`)\n\
             - **Recent SOLL revisions** : {} (last 10)\n\
             - **Work plan top** : {} (current REQ/MIL)\n\
             - **Session pointer** : {}\n\n\
             Call `skill_invoke` next per methodology mandate, or `soll_work_plan` for full scoring.",
            project_code,
            reason,
            pillars.len(),
            pillars
                .iter()
                .filter_map(|p| p.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", "),
            recent_decisions.len(),
            mandated_skills.len(),
            recent_revisions.len(),
            work_plan_top.len(),
            session_pointer
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("(none)"),
        );

        Some(json!({
            "content": [{ "type": "text", "text": summary_text }],
            "data": {
                "status": "ok",
                "project_code": project_code,
                "reason": reason,
                "active_methodology": {
                    "pillars": pillars,
                    "recent_decisions": recent_decisions,
                },
                "mandated_skills": mandated_skills,
                "recent_revisions": recent_revisions,
                "session_pointer": session_pointer,
                "work_plan_top": work_plan_top,
            }
        }))
    }
}
