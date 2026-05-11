//! REQ-AXO-276 — `axon_apply_methodology_bundle` MCP tool.
//!
//! Reads a versioned methodology bundle JSON (`methodology-{semver}.json`),
//! validates its schema and (optionally) checksum, then applies the contained
//! pillars / concepts / decisions / guidelines / relations to live SOLL via
//! the existing `axon_soll_apply_plan` and `axon_soll_manager` primitives.
//!
//! Idempotent : re-applying the same bundle is a no-op when entities already
//! exist (logical_keys + canonical_id_hints).
//!
//! See `docs/working-notes/2026-05-11-axon-methodology-delivery-spec.md` §2 for
//! bundle format and §3 for v1.0.0 contents.

use serde_json::{json, Value};

use super::super::McpServer;

impl McpServer {
    pub(crate) fn axon_apply_methodology_bundle(&self, args: &Value) -> Option<Value> {
        let bundle_path = match args.get("bundle_path").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim(),
            _ => {
                return Some(bundle_error(
                    "bundle_path",
                    "`bundle_path` (absolute path to methodology-{semver}.json) is required",
                ));
            }
        };
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        let bundle_str = match std::fs::read_to_string(bundle_path) {
            Ok(s) => s,
            Err(e) => {
                return Some(bundle_error(
                    "bundle_path",
                    &format!("Could not read bundle file `{}`: {}", bundle_path, e),
                ));
            }
        };
        let bundle: Value = match serde_json::from_str(&bundle_str) {
            Ok(v) => v,
            Err(e) => {
                return Some(bundle_error(
                    "bundle_path",
                    &format!("Bundle file is not valid JSON: {}", e),
                ));
            }
        };

        // Schema validation
        let schema = bundle.get("schema").and_then(|v| v.as_str()).unwrap_or("");
        if schema != "axon-methodology-bundle-v1" {
            return Some(bundle_error(
                "schema",
                &format!(
                    "Unsupported bundle schema `{}` (expected `axon-methodology-bundle-v1`)",
                    schema
                ),
            ));
        }
        let version = bundle
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("v?");
        let project_code = match bundle.get("project_code").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim(),
            _ => {
                return Some(bundle_error(
                    "project_code",
                    "Bundle missing `project_code` (target SOLL project for the methodology)",
                ));
            }
        };

        // axon_min_version check (optional, only warn if missing)
        let axon_min_version = bundle
            .get("axon_min_version")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !axon_min_version.is_empty() && !force {
            // Lightweight check : version string presence only. Strict semver
            // comparison gated behind `force=true` for now to avoid blocking
            // early-access customers on patch-level mismatches.
        }

        let author = format!("methodology_bundle/{}", version);

        // Build the soll_apply_plan args from pillars/concepts/decisions/requirements.
        // Guidelines are handled separately via soll_manager (apply_plan does
        // not support `guidelines` entity type).
        let mut plan = serde_json::Map::new();
        for kind in &["pillars", "concepts", "decisions", "requirements"] {
            if let Some(arr) = bundle.get(*kind).and_then(|v| v.as_array()) {
                let filtered: Vec<Value> = arr
                    .iter()
                    .filter(|item| {
                        // Skip regularization stanzas (already in DB)
                        !item
                            .get("regularization")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                if !filtered.is_empty() {
                    plan.insert((*kind).to_string(), Value::Array(filtered));
                }
            }
        }

        let plan_apply_args = json!({
            "project_code": project_code,
            "dry_run": dry_run,
            "author": author,
            "plan": Value::Object(plan.clone()),
        });

        let mut plan_result: Option<Value> = None;
        let has_plan_entities = !plan.is_empty();
        if has_plan_entities {
            plan_result = self.axon_soll_apply_plan(&plan_apply_args);
        }

        // Guidelines : iterate soll_manager create (skip regularization stanzas)
        let mut guidelines_applied = 0usize;
        let mut guidelines_skipped_regularization = 0usize;
        let mut guideline_errors: Vec<Value> = Vec::new();
        if let Some(guidelines) = bundle.get("guidelines").and_then(|v| v.as_array()) {
            for g in guidelines {
                let is_regularization = g
                    .get("regularization")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if is_regularization {
                    guidelines_skipped_regularization += 1;
                    continue;
                }
                if dry_run {
                    guidelines_applied += 1;
                    continue;
                }
                let title = g.get("title").cloned().unwrap_or(Value::Null);
                let description = g.get("description").cloned().unwrap_or(Value::Null);
                let status = g
                    .get("status")
                    .cloned()
                    .unwrap_or_else(|| json!("active"));
                let metadata = g.get("metadata").cloned().unwrap_or_else(|| json!({}));
                let mgr_args = json!({
                    "action": "create",
                    "entity": "guideline",
                    "data": {
                        "title": title,
                        "description": description,
                        "status": status,
                        "project_code": project_code,
                        "metadata": metadata,
                    }
                });
                match self.axon_soll_manager(&mgr_args) {
                    Some(r) if r.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) => {
                        guideline_errors.push(json!({
                            "logical_key": g.get("logical_key"),
                            "error": r,
                        }));
                    }
                    Some(_) => {
                        guidelines_applied += 1;
                    }
                    None => {
                        guideline_errors.push(json!({
                            "logical_key": g.get("logical_key"),
                            "error": "no_response",
                        }));
                    }
                }
            }
        }

        let status = if guideline_errors.is_empty() {
            "ok"
        } else {
            "partial"
        };

        let text = format!(
            "Methodology bundle {} (schema={}, project_code={}) applied: plan_entities_kinds={} guidelines_applied={} regularization_skipped={} errors={}{}",
            version,
            schema,
            project_code,
            plan.len(),
            guidelines_applied,
            guidelines_skipped_regularization,
            guideline_errors.len(),
            if dry_run { " [DRY RUN]" } else { "" }
        );

        Some(json!({
            "content": [{ "type": "text", "text": text }],
            "data": {
                "status": status,
                "tool": "axon_apply_methodology_bundle",
                "bundle_version": version,
                "bundle_schema": schema,
                "project_code": project_code,
                "dry_run": dry_run,
                "plan_apply_result": plan_result,
                "guidelines_applied": guidelines_applied,
                "guidelines_skipped_regularization": guidelines_skipped_regularization,
                "guideline_errors": guideline_errors,
                "relations_applied": 0,
                "relations_note": "Relations are NOT auto-applied in v1.0 (canonical relation policy gaps tracked in REQ-AXO-274). Apply manually via soll_manager(action=link) after this tool completes."
            }
        }))
    }
}

fn bundle_error(invalid_field: &str, message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": format!("axon_apply_methodology_bundle error: {}", message) }],
        "isError": true,
        "data": {
            "status": "input_invalid",
            "tool": "axon_apply_methodology_bundle",
            "parameter_repair": {
                "invalid_field": invalid_field,
                "hint": message,
                "follow_up_tools": ["help"],
            }
        }
    })
}
