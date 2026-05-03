use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract};
use super::tools_framework::WHY_CACHE_TTL_MS;
use super::tools_framework_status_guidance::project_status_operator_guidance;
use super::tools_framework_support::{
    cache_read, cache_write, load_structural_snapshots, persist_structural_snapshot,
    structural_history_path,
};
use super::McpServer;

impl McpServer {
    pub(super) fn axon_project_status_impl(&self, args: &Value) -> Option<Value> {
        let mode = args.get("mode").and_then(|value| value.as_str());
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");

        let status = self.axon_status(&json!({ "mode": mode.unwrap_or("brief") }))?;
        let status_data = status.get("data").cloned().unwrap_or_else(|| json!({}));

        let anomalies_data = json!({
            "summary": { "note": "Anomalies calculation decoupled to prevent timeout. Use 'anomalies' tool directly." },
            "findings": [],
            "recommendations": []
        });
        let soll_context = self.axon_soll_query_context(&json!({
            "project_code": project_code,
            "limit": 5
        }))?;
        let soll_data = soll_context
            .get("data")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let conception = self.cached_conception_view(project_code);
        let vision = soll_data
            .get("visions")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
            .and_then(|value| value.as_str())
            .map(Self::parse_soll_vision_entry)
            .unwrap_or_else(|| {
                json!({
                    "id": "unavailable",
                    "title": "unavailable",
                    "status": "unknown",
                    "description": "unavailable",
                    "source": "SOLL"
                })
            });

        let anomaly_summary = anomalies_data
            .get("summary")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let previous_snapshot = load_structural_snapshots(project_code).into_iter().last();
        let previous_summary = previous_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.get("anomaly_summary"));
        let snapshot_id = format!("project-status-{}-{}", project_code, Self::now_unix_ms());
        let generated_at = Self::now_unix_ms();
        let delta_vs_previous =
            Self::build_project_status_delta(previous_summary, &anomaly_summary);
        let degraded_notes = status_data
            .pointer("/availability/degraded_notes")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect::<Vec<_>>();
        let snapshot_record = json!({
            "snapshot_id": snapshot_id,
            "generated_at": generated_at,
            "project_code": project_code,
            "anomaly_summary": anomaly_summary,
            "conception_summary": {
                "module_count": conception.get("module_count").cloned().unwrap_or_else(|| json!(0)),
                "interface_count": conception.get("interface_count").cloned().unwrap_or_else(|| json!(0)),
                "contract_count": conception.get("contract_count").cloned().unwrap_or_else(|| json!(0)),
                "flow_count": conception.get("flow_count").cloned().unwrap_or_else(|| json!(0))
            },
            "provenance": "aggregated",
            "confidence": "medium"
        });
        let snapshot_storage = match persist_structural_snapshot(project_code, &snapshot_record) {
            Ok(()) => json!({
                "scope": "derived_non_canonical",
                "path": structural_history_path(project_code).to_string_lossy().to_string(),
                "persisted": true
            }),
            Err(error) => {
                let mut notes = degraded_notes.clone();
                notes.push(format!("snapshot_persistence_failed:{error}"));
                json!({
                    "scope": "derived_non_canonical",
                    "path": structural_history_path(project_code).to_string_lossy().to_string(),
                    "persisted": false,
                    "error": error,
                    "degraded_notes": notes
                })
            }
        };
        let operator_guidance =
            project_status_operator_guidance(&degraded_notes, &snapshot_storage, &vision);
        let next_best_action = operator_guidance
            .get("next_action")
            .cloned()
            .unwrap_or(Value::Null);
        let mut proof_gaps = Vec::<Value>::new();
        if anomaly_summary.get("validation_coverage_score").is_none() {
            proof_gaps.push(json!("validation_coverage_unknown"));
        }
        if vision
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("unavailable")
            == "unavailable"
        {
            proof_gaps.push(json!("canonical_vision_unavailable"));
        }
        if snapshot_storage
            .get("persisted")
            .and_then(|value| value.as_bool())
            == Some(false)
        {
            proof_gaps.push(json!("snapshot_storage_not_persisted"));
        }
        let truth_cockpit = json!({
            "current_blocker": degraded_notes
                .first()
                .cloned()
                .map(Value::String)
                .unwrap_or(Value::Null),
            "next_best_action": next_best_action,
            "confidence": "high",
            "freshness": {
                "state": if degraded_notes.is_empty() { "fresh" } else { "degraded" },
                "degraded_notes": degraded_notes,
                "runtime_truth_status": status_data.get("truth_status").cloned().unwrap_or(Value::Null)
            },
            "proof_gaps": proof_gaps,
            "llm_instruction": "Use `next_best_action` first; if freshness is degraded, label project-wide conclusions partial and follow the named MCP tool."
        });
        let public_tools = status_data
            .get("public_tools")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect::<Vec<_>>();
        let brief_mode = mode.unwrap_or("brief") == "brief";
        let public_tools_evidence = if public_tools.is_empty() {
            "unknown".to_string()
        } else if brief_mode {
            format!("{} tools (use `status` for list)", public_tools.len())
        } else {
            public_tools.join(", ")
        };
        let runtime_data = if brief_mode {
            json!({
                "runtime_mode": status_data.get("runtime_mode").cloned().unwrap_or(Value::Null),
                "runtime_profile": status_data.get("runtime_profile").cloned().unwrap_or(Value::Null),
                "truth_status": status_data.get("truth_status").cloned().unwrap_or(Value::Null),
                "drain_state": status_data.get("drain_state").cloned().unwrap_or(Value::Null),
                "availability": status_data.get("availability").cloned().unwrap_or_else(|| json!({})),
                "runtime_version": status_data.get("runtime_version").cloned().unwrap_or_else(|| json!({})),
                "runtime_state": status_data.pointer("/runtime_authority/runtime_state").cloned().unwrap_or_else(|| json!({})),
                "file_vectorization_queue": status_data.get("file_vectorization_queue").cloned().unwrap_or_else(|| json!({})),
                "public_tool_count": public_tools.len(),
                "mode": "brief_compact"
            })
        } else {
            status_data.clone()
        };

        let evidence = format!(
            "**Vision:** `{}` - {}\n\
**Vision status:** `{}`\n\
**Runtime mode/profile:** `{}` / `{}`\n\
**Drain state:** `{}`\n\
**Public tools:** {}\n\
**Wrappers / Orphan code / Orphan intent:** {} / {} / {}\n\
**Validation coverage:** {}\n\
**Degradation notes:** {}\n",
            vision
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or("unavailable"),
            vision
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or("unavailable"),
            vision
                .get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("runtime_mode")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("runtime_profile")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("drain_state")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            public_tools_evidence,
            anomaly_summary
                .get("wrapper_count")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            anomaly_summary
                .get("orphan_code_count")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            anomaly_summary
                .get("orphan_intent_count")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            anomaly_summary
                .get("validation_coverage_score")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            if degraded_notes.is_empty() {
                "none".to_string()
            } else {
                degraded_notes.join(", ")
            }
        );
        let report = format!(
            "## 🧭 Project Status\n\n{}",
            format_standard_contract(
                "ok",
                "live project situation assembled from MCP read surfaces",
                &format!("project:{}", project_code),
                &evidence_by_mode(&evidence, mode),
                &[
                    "use `why` on a specific symbol to inspect rationale",
                    "use `path` for source/sink flow",
                    "use `anomalies` for the full structural findings payload"
                ],
                "high",
            )
        );

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "project_code": project_code,
                "snapshot_id": snapshot_id,
                "generated_at": generated_at,
                "delta_vs_previous": delta_vs_previous,
                "vision": vision,
                "conception": conception,
                "runtime": runtime_data,
                "truth_cockpit": truth_cockpit,
                "anomalies": {
                    "summary": anomaly_summary,
                    "findings": anomalies_data.get("findings").cloned().unwrap_or_else(|| json!([])),
                    "recommendations": anomalies_data.get("recommendations").cloned().unwrap_or_else(|| json!([]))
                },
                "snapshot_storage": snapshot_storage,
                "operator_guidance": operator_guidance.clone(),
                "next_action": operator_guidance
                    .get("next_action")
                    .cloned()
                    .unwrap_or(Value::Null),
                "soll_context": {
                    "visions": soll_data.get("visions").cloned().unwrap_or_else(|| json!([])),
                    "requirements": soll_data.get("requirements").cloned().unwrap_or_else(|| json!([])),
                    "decisions": soll_data.get("decisions").cloned().unwrap_or_else(|| json!([])),
                    "revisions": soll_data.get("revisions").cloned().unwrap_or_else(|| json!([]))
                },
                "canonical_sources": Self::canonical_sources_snapshot()
            }
        }))
    }

    pub(super) fn axon_why_impl(&self, args: &Value) -> Option<Value> {
        let mode = args
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("brief");
        let cache_key = format!(
            "{}::{}::{}::{}",
            args.get("symbol")
                .and_then(|value| value.as_str())
                .or_else(|| args.get("question").and_then(|value| value.as_str()))
                .unwrap_or("*"),
            args.get("project")
                .and_then(|value| value.as_str())
                .unwrap_or("*"),
            mode,
            args.get("include_graph")
                .and_then(|value| value.as_bool())
                .unwrap_or(mode != "brief")
        );
        let now_ms = Self::now_unix_ms();
        if let Some(cached) = cache_read(Self::why_cache(), &cache_key, now_ms, WHY_CACHE_TTL_MS) {
            return Some(cached);
        }
        let include_graph = args
            .get("include_graph")
            .and_then(|value| value.as_bool())
            .unwrap_or(mode != "brief");
        // REQ-AXO-043 — `symbol=""` previously produced a malformed
        // "Why does  exist?" question (double space) that retrieve_context
        // happily processed, returning Status: ok with arbitrary supporting
        // evidence. Trim and reject empty symbol BEFORE falling through.
        let question = args
            .get("question")
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                args.get("symbol")
                    .and_then(|value| value.as_str())
                    .map(|symbol| symbol.trim().to_string())
                    .filter(|symbol| !symbol.is_empty())
                    .map(|symbol| format!("Why does {} exist?", symbol))
            });
        let question = match question {
            Some(value) => value,
            None => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": "why requires a non-empty `symbol` or `question`. Pass either a target symbol id/name or a free-form question (example: symbol=\"axon_query\" or question=\"why does the queue admission policy reject?\")."
                    }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "missing_field": "symbol_or_question",
                        "next_action": "supply at least one of `symbol` or `question`",
                        "operator_guidance": {
                            "problem_class": "input_invalid",
                            "likely_cause": "empty_or_whitespace_symbol_and_question",
                            "next_best_actions": [
                                "supply a non-empty `symbol` argument (canonical id or name)",
                                "or supply a non-empty `question` describing the rationale you want",
                            ],
                            "follow_up_tools": ["query", "inspect", "retrieve_context"],
                            "confidence": "high",
                        },
                        "parameter_repair": {
                            "invalid_field": "symbol|question",
                            "accepted_aliases": ["symbol", "question"],
                            "follow_up_tools": ["query", "inspect", "retrieve_context"],
                            "hint": "supply at least one of `symbol` (canonical id or name) or `question` (free-form rationale prompt); example: symbol=\"axon_query\" or question=\"why does the queue admission policy reject?\""
                        }
                    }
                }));
            }
        };
        let mut response = self.axon_retrieve_context(&json!({
            "question": question,
            "project": args.get("project").and_then(|value| value.as_str()),
            "mode": mode,
            "top_k": args.get("top_k").cloned().unwrap_or_else(|| json!(if mode == "brief" { 3 } else { 6 })),
            "token_budget": args.get("token_budget").cloned().unwrap_or_else(|| json!(if mode == "brief" { 700 } else { 1400 })),
            "include_soll": true,
            "include_graph": include_graph
        }))?;
        if let Some(data) = response
            .get_mut("data")
            .and_then(|value| value.as_object_mut())
        {
            data.insert("framework_alias".to_string(), json!("why"));
        }
        Self::summarize_why_response(args, &mut response);
        cache_write(Self::why_cache(), cache_key, now_ms, &response);
        Some(response)
    }
}
