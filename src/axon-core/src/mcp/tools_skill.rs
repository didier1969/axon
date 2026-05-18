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

use super::McpServer;

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
    /// Resolves a PRT node by canonical id and returns the rendered body.
    /// This first cut returns the raw template body without parameter
    /// substitution — Mustache rendering is a followup slice (full design
    /// in CPT-AXO-90017 : Mustache logic-less + typed parameter sidecar +
    /// validation rules). The `params` argument is captured for audit ;
    /// future iteration will validate against metadata.parameters spec and
    /// render via the Mustache engine.
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
        let params = arguments.get("params").cloned().unwrap_or_else(|| json!({}));

        // First cut : return raw body (no Mustache substitution yet — slice 2).
        // Capture params for future rendering + audit.
        let rendered_text = body_template.clone();

        let display = format!(
            "## 📝 Prompt template `{}` — {}\n\n**Status** : {} · **Project** : {}\n\n```\n{}\n```\n\n_Note: Mustache parameter substitution is a follow-up slice (REQ-AXO-91581 slice 2). Raw template returned ; params captured in data._",
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
                "params_used": params,
                "status_field": status,
                "project_code": project_code,
                "metadata": metadata,
                "rendering_engine": "raw_passthrough_v0",
            }
        }))
    }
}
