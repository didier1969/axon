use crate::graph::GraphStore;
use crate::project_meta::discover_project_identities;
use crate::soll_snapshot::SollSnapshotCache;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use std::thread;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
mod catalog;
mod dispatch;
mod format;
mod guidance;
mod protocol;
mod revision_docs_listener;
mod runtime_topology_support;
mod soll;
#[cfg(test)]
mod tests;
mod tool_contracts;
mod tools_context;
mod tools_data_catalog;
mod tools_dx;
mod tools_framework;
mod tools_framework_anomalies;
mod tools_framework_anomaly_heuristics;
mod tools_framework_change_safety;
mod tools_framework_conception;
mod tools_framework_path;
mod tools_framework_rationale;
mod tools_framework_runtime;
mod tools_framework_runtime_contracts;
mod tools_framework_runtime_quiescence;
mod tools_framework_runtime_status;
mod tools_framework_runtime_topology;
mod tools_framework_snapshot;
mod tools_framework_status_guidance;
mod tools_framework_support;
mod tools_framework_surface;
mod tools_framework_validation;
mod tools_governance;
mod tools_help;
mod tools_ist_algorithms;
mod tools_ist_snapshot;
mod tools_risk;
mod tools_skill;
mod tools_friction;
mod tools_soll;
pub(crate) mod tools_srs;
pub(crate) mod tools_system;
mod tools_system_debug;

use self::catalog::tools_catalog;
#[allow(unused_imports)]
pub(crate) use self::guidance::{
    attach_guidance_authoritative, attach_guidance_shadow, build_guided_response,
    classify_guidance, guidance_outcome_to_value, project_authoritative_phase1_guidance,
    GuidanceCandidates, GuidanceFact, GuidanceOutcome, SollGuidance,
};
pub use self::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use self::soll::canonical_soll_site_dir;
use crate::runtime_command_proxy::{RuntimeCommandProxy, RuntimeCommandProxyDecision};
use crate::runtime_topology::current_runtime_process_role;

pub struct McpServer {
    graph_store: Arc<GraphStore>,
    // DEC-AXO-091 / REQ-AXO-322 — in-memory SOLL snapshot cache.
    // Lazily loaded per project_code on first hot-read call.
    // Invalidated after every MCP-side mutation tool via
    // `attach_derived_docs_refresh_metadata`.
    soll_cache: Arc<SollSnapshotCache>,
    // REQ-AXO-901732 — weak self-reference, set in production via
    // `init_self_arc` once the server is wrapped in `Arc`. Lets
    // `attach_derived_docs_refresh_metadata` hand an owned `Arc<Self>` to a
    // detached background thread so the non-canonical derived-docs render
    // never blocks (or times out) the canonical mutation response
    // (PIL-AXO-009). Unset in unit tests (server used un-`Arc`'d) → the
    // render stays synchronous there, preserving the legacy "ok" contract.
    self_arc: std::sync::OnceLock<std::sync::Weak<McpServer>>,
    // REQ-AXO-901732 — coalescing guard: project_codes whose background
    // derived-docs refresh is already in flight. Prevents thread pile-up
    // under rapid mutations on the same project.
    derived_docs_refresh_inflight: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    // REQ-AXO-901732 — global render serialization. `write_if_changed` is a
    // non-atomic `fs::write`, and every project render also rewrites the
    // shared ROOT index/manifest; two concurrent background renders (distinct
    // projects) could otherwise tear the root file. Renders hold this lock so
    // at most one runs at a time across all projects (the per-project guard
    // above still bounds the number of threads spawned).
    derived_docs_render_lock: Arc<std::sync::Mutex<()>>,
}

const SUPPORTED_MCP_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

fn mcp_server_identity_name() -> &'static str {
    current_runtime_process_role().runtime_binary_name()
}

impl McpServer {
    pub(crate) const ASYNC_JOB_TOOL_NAMES: &[&str] = &["restore_soll", "soll_apply_plan"];
    pub(crate) const MONITORED_SYNC_MUTATION_TOOLS: &[&str] = &["soll_commit_revision"];
    pub(crate) const SOLL_DERIVED_DOCS_REFRESH_TOOLS: &[&str] = &[
        "restore_soll",
        "soll_apply_plan",
        "soll_commit_revision",
        "soll_attach_evidence",
        "soll_remove_evidence",
        "soll_rollback_revision",
        "soll_manager",
        "entrench_nuance",
        "init_project",
        "apply_guidelines",
    ];

    pub fn new(graph_store: Arc<GraphStore>) -> Self {
        let soll_cache = SollSnapshotCache::new(graph_store.clone());
        Self {
            graph_store,
            soll_cache,
            self_arc: std::sync::OnceLock::new(),
            derived_docs_refresh_inflight: Arc::new(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
            derived_docs_render_lock: Arc::new(std::sync::Mutex::new(())),
        }
    }

    /// REQ-AXO-901732 — wire the weak self-reference. Call exactly once, in
    /// production, immediately after wrapping the server in `Arc` (see
    /// `main_services.rs`). Enables non-blocking background derived-docs
    /// refresh; without it the refresh stays synchronous (the unit-test path).
    pub fn init_self_arc(self: &Arc<Self>) {
        let _ = self.self_arc.set(Arc::downgrade(self));
    }

    /// REQ-AXO-322 — access the in-memory SOLL snapshot for hot reads.
    pub(crate) fn soll_cache(&self) -> &Arc<SollSnapshotCache> {
        &self.soll_cache
    }

    fn public_tool_name_for(requested_name: &str, normalized_name: &str) -> String {
        if requested_name.trim().is_empty() {
            return normalized_name.to_string();
        }
        requested_name.to_string()
    }

    fn default_follow_up_tools_for(normalized_name: &str) -> &'static [&'static str] {
        // REQ-AXO-901949 — single-source interaction graph for tracer tools.
        if let Some(routing) = crate::mcp::tool_contracts::tool_routing(normalized_name) {
            return routing.follow_ups;
        }
        match normalized_name {
            "help" => &["status", "project_status"],
            "status" => &["project_status", "mcp_surface_diagnostics"],
            "mcp_surface_diagnostics" => &["status", "project_status"],
            "project_status" => &["anomalies", "why", "path"],
            "project_registry_lookup" => &["project_status", "soll_query_context"],
            "query" => &["inspect", "retrieve_context", "impact"],
            "inspect" => &["impact", "path", "why"],
            "retrieve_context" => &["inspect", "why", "path"],
            "why" => &["inspect", "path", "project_status"],
            "path" => &["impact", "why", "inspect"],
            "anomalies" => &["change_safety", "conception_view", "project_status"],
            "conception_view" => &["anomalies", "path", "why"],
            "change_safety" => &["impact", "path", "inspect"],
            "impact" => &["simulate_mutation", "path", "why"],
            "fuse" => &["why", "impact", "inspect"],
            "audit" => &["health", "anomalies", "change_safety"],
            "health" => &["status", "audit", "project_status"],
            "architectural_drift" => &["conception_view", "anomalies", "why"],
            "snapshot_history" => &["snapshot_diff", "project_status"],
            "snapshot_diff" => &["project_status", "anomalies"],
            "soll_query_context" => &["soll_work_plan", "soll_verify_requirements"],
            "soll_work_plan" => &["soll_query_context", "soll_verify_requirements"],
            "soll_validate" => &["soll_verify_requirements", "soll_relation_schema"],
            "soll_verify_requirements" => &["soll_attach_evidence", "soll_manager"],
            "soll_relation_schema" => &["soll_manager", "infer_soll_mutation"],
            "infer_soll_mutation" => &["entrench_nuance", "soll_manager"],
            "entrench_nuance" => &["soll_query_context", "soll_validate"],
            "soll_manager" => &["soll_validate", "soll_query_context"],
            "soll_apply_plan" | "restore_soll" | "resume_vectorization" => &["job_status"],
            "soll_commit_revision" => &["soll_query_context", "soll_export"],
            "soll_rollback_revision" => &["soll_query_context", "soll_validate"],
            "soll_attach_evidence" => &["soll_verify_requirements", "soll_query_context"],
            "soll_remove_evidence" => &["soll_verify_requirements", "soll_validate"],
            "init_project" | "apply_guidelines" => &["project_status", "soll_query_context"],
            "commit_work" => &["pre_flight_check", "project_status"],
            "pre_flight_check" => &["commit_work", "project_status"],
            "soll_export" => &["soll_query_context", "soll_validate"],
            "soll_generate_docs" => &["soll_export", "project_status"],
            "diagnose_indexing" => &["health", "status", "query"],
            "semantic_clones" => &["inspect", "impact", "query"],
            "bidi_trace" => &["impact", "inspect", "why"],
            "api_break_check" => &["impact", "inspect", "change_safety"],
            "simulate_mutation" => &["impact", "change_safety", "path"],
            "batch" => &["status", "mcp_surface_diagnostics"],
            "sql" => &["schema_overview", "query_examples"],
            "schema_overview" => &["query_examples", "query"],
            "query_examples" => &["query", "schema_overview"],
            "truth_check" => &["status", "project_status"],
            "fs_read" => &["inspect", "query"],
            "debug" => &["status", "mcp_surface_diagnostics"],
            _ => &["status", "mcp_surface_diagnostics"],
        }
    }

    fn default_next_action_for(
        normalized_name: &str,
        arguments: &Value,
        response: &Value,
    ) -> Value {
        let response_text = response
            .get("content")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let follow_up_tools = Self::default_follow_up_tools_for(normalized_name);

        if response_text.contains("Invalid arguments for tool") {
            return json!({
                "kind": "retry_with_valid_arguments",
                "tool": normalized_name,
                "when": "after_fixing_arguments",
                "arguments": arguments
            });
        }

        if response_text.contains("Tool not found") {
            return json!({
                "kind": "inspect_public_surface_then_retry",
                "tool": "mcp_surface_diagnostics",
                "when": "now"
            });
        }

        let tool = follow_up_tools.first().copied().unwrap_or("status");
        json!({
            "kind": if response.get("isError").and_then(Value::as_bool) == Some(true) {
                "recover_with_follow_up_tool"
            } else {
                "continue_with_follow_up_tool"
            },
            "tool": tool,
            "when": "now"
        })
    }

    fn workflow_stage_for(normalized_name: &str) -> &'static str {
        if let Some(routing) = crate::mcp::tool_contracts::tool_routing(normalized_name) {
            return routing.stage;
        }
        match normalized_name {
            "help" => "tool_routing",
            "status" | "mcp_surface_diagnostics" | "health" | "diagnose_indexing" => {
                "runtime_truth"
            }
            "project_status" | "project_registry_lookup" => "project_truth",
            "query" | "inspect" | "retrieve_context" | "fs_read" => "target_discovery",
            "impact" | "path" | "bidi_trace" | "simulate_mutation" | "api_break_check" => {
                "change_analysis"
            }
            "why"
            | "anomalies"
            | "conception_view"
            | "architectural_drift"
            | "audit"
            | "change_safety"
            | "snapshot_history"
            | "snapshot_diff" => "structural_reasoning",
            "soll_query_context"
            | "soll_work_plan"
            | "soll_validate"
            | "soll_verify_requirements"
            | "soll_relation_schema"
            | "infer_soll_mutation"
            | "entrench_nuance"
            | "soll_manager"
            | "soll_attach_evidence"
            | "soll_remove_evidence"
            | "soll_commit_revision"
            | "soll_rollback_revision"
            | "soll_apply_plan"
            | "restore_soll"
            | "soll_export"
            | "soll_generate_docs"
            | "init_project"
            | "apply_guidelines" => "intent_governance",
            "commit_work" | "pre_flight_check" => "delivery",
            _ => "general_mcp_operation",
        }
    }

    fn primary_goal_for(normalized_name: &str) -> &'static str {
        if let Some(routing) = crate::mcp::tool_contracts::tool_routing(normalized_name) {
            return routing.goal;
        }
        match normalized_name {
            "help" => "choose the smallest useful Axon MCP tool sequence",
            "status" => "establish runtime truth before trusting deeper conclusions",
            "mcp_surface_diagnostics" => {
                "verify the MCP surface and client/server contract freshness"
            }
            "project_status" => "compress project truth into one high-signal packet",
            "project_registry_lookup" => "recover canonical project identity before scoped work",
            "query" => "discover plausible targets with broad recall",
            "inspect" => "validate one exact target before acting",
            "retrieve_context" => "deliver a compact packet that saves downstream LLM tokens",
            "why" => "recover governing intent and rationale",
            "path" | "bidi_trace" => "understand source/sink or dependency flow between anchors",
            "impact" => "estimate blast radius before mutation",
            "fuse" => "fuse a symbol's governing intent (WHY) with its impact radius (HOW) in one read",
            "anomalies" => "surface structural risks worth explicit follow-up",
            "conception_view" => "read the derived architecture map",
            "change_safety" => "decide whether a mutation is safe enough to proceed",
            "audit" => "summarize governance and operational risk",
            "health" => "establish operational health before deeper diagnosis",
            "architectural_drift" => {
                "detect deviation between current structure and intended shape"
            }
            "soll_query_context" => "recover compact canonical intent",
            "soll_work_plan" => "turn intent into executable work ordering",
            "soll_validate" => "find graph consistency and completeness gaps",
            "soll_verify_requirements" => "measure requirement proof coverage",
            "soll_relation_schema" => "discover allowed SOLL link patterns before mutating",
            "infer_soll_mutation" => "scope a safe SOLL mutation before writing",
            "entrench_nuance" => "apply a bounded intent clarification",
            "soll_manager" => "perform an exact SOLL create/update/link operation",
            "soll_attach_evidence" => "attach proof to canonical intent",
            "soll_remove_evidence" => {
                "prune broken evidence rows so completeness reflects current code state"
            }
            "soll_apply_plan" => "apply a larger SOLL batch transaction safely",
            "restore_soll" => "restore canonical intent from an export",
            "commit_work" => "commit work only after SOLL-aware validation",
            "pre_flight_check" => "validate the delivery wave before commit",
            _ => "move to the next highest-signal MCP step",
        }
    }

    fn token_efficiency_hint_for(normalized_name: &str) -> &'static str {
        if let Some(routing) = crate::mcp::tool_contracts::tool_routing(normalized_name) {
            return routing.token_hint;
        }
        match normalized_name {
            "help" => "Call `help` once when routing is unclear; then follow the smallest tool chain it recommends.",
            "retrieve_context" | "project_status" | "soll_query_context" => {
                "Prefer this compact packet over reconstructing the same truth from multiple lower-level calls."
            }
            "query" => "Use `query` to widen recall only when the target anchor is still ambiguous; switch to `inspect` quickly once a candidate exists.",
            "inspect" => "Use `inspect` to collapse ambiguity before asking broader architectural questions.",
            "why" => "If `why` is weak, consume its guidance fields first instead of launching many speculative calls.",
            "impact" | "path" | "bidi_trace" => "Use graph-flow tools only after the target is stable; otherwise you spend tokens on the wrong graph slice.",
            "status" | "mcp_surface_diagnostics" => "Use runtime truth first so the client does not waste tokens reasoning over stale or degraded surfaces.",
            _ => "Follow the server-provided next step before composing additional exploratory calls."
        }
    }

    fn follow_up_reason_for(tool: &str) -> &'static str {
        if let Some(routing) = crate::mcp::tool_contracts::tool_routing(tool) {
            return routing.use_when;
        }
        match tool {
            "status" => "use when runtime truth may be stale, degraded, or operationally unclear",
            "mcp_surface_diagnostics" => {
                "use when the client/server MCP contract may be stale or mismatched"
            }
            "project_status" => {
                "use when you need compact project truth instead of stitching multiple probes"
            }
            "query" => "use when recall is too narrow and you need broader candidate discovery",
            "inspect" => "use when you already have a likely target and need exact validation",
            "retrieve_context" => {
                "use when you need a compact evidence packet that minimizes downstream LLM tokens"
            }
            "impact" => "use when you need blast radius before editing or mutating",
            "fuse" => "use when you need a symbol's governing intent and impact together in one call",
            "path" => "use when the missing truth is connectivity or source-to-sink flow",
            "why" => "use when the missing truth is rationale or governing intent",
            "anomalies" => "use when you need prioritized structural findings",
            "conception_view" => "use when you need the derived architecture map",
            "change_safety" => "use when you need an explicit safety signal before mutation",
            "soll_query_context" => "use when you need compact canonical intent",
            "soll_validate" => "use when you need graph consistency and repair guidance",
            "soll_verify_requirements" => "use when you need proof coverage and missing dimensions",
            "soll_manager" => "use when the next step is an exact canonical mutation",
            "infer_soll_mutation" => "use when you need mutation scope help before writing to SOLL",
            "job_status" => {
                "use when the current operation is asynchronous and you need terminal truth"
            }
            _ => "use when it is the next highest-signal MCP move",
        }
    }

    fn alternative_strategies_for(normalized_name: &str) -> Vec<Value> {
        Self::default_follow_up_tools_for(normalized_name)
            .iter()
            .map(|tool| {
                json!({
                    "tool": tool,
                    "use_when": Self::follow_up_reason_for(tool),
                    "value": Self::primary_goal_for(tool)
                })
            })
            .collect()
    }

    fn tool_input_contract_for(normalized_name: &str) -> Option<Value> {
        let catalog = tools_catalog(true);
        let tools = catalog.get("tools")?.as_array()?;
        let entry = tools.iter().find(|tool| {
            let Some(name) = tool.get("name").and_then(Value::as_str) else {
                return false;
            };
            name == normalized_name || name.strip_prefix("axon_") == Some(normalized_name)
        })?;
        let schema = entry.get("inputSchema")?;
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let accepted_fields = schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| props.keys().cloned().map(Value::String).collect::<Vec<_>>())
            .unwrap_or_default();
        Some(json!({
            "required_fields": required,
            "accepted_fields": accepted_fields
        }))
    }

    fn parameter_repair_guidance_for(normalized_name: &str, arguments: &Value) -> Option<Value> {
        let contract = Self::tool_input_contract_for(normalized_name)?;
        let required = contract
            .get("required_fields")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let accepted = contract
            .get("accepted_fields")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let provided_object = arguments.as_object().cloned().unwrap_or_default();
        let provided_fields = provided_object
            .keys()
            .cloned()
            .map(Value::String)
            .collect::<Vec<_>>();
        let missing_required_fields = required
            .iter()
            .filter_map(Value::as_str)
            .filter(|field| !provided_object.contains_key(*field))
            .map(|field| Value::String(field.to_string()))
            .collect::<Vec<_>>();
        let unknown_fields = provided_object
            .keys()
            .filter(|field| {
                !accepted
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|accepted_field| accepted_field == field.as_str())
            })
            .map(|field| Value::String(field.to_string()))
            .collect::<Vec<_>>();

        Some(json!({
            "required_fields": required,
            "accepted_fields": accepted,
            "provided_fields": provided_fields,
            "missing_required_fields": missing_required_fields,
            "unknown_fields": unknown_fields,
            "micro_instruction": format!("Fix `{normalized_name}` args: add missing required fields, remove unknown fields, retry once."),
            "retry_rule": format!("Retry `{normalized_name}` after the argument object matches the contract.")
        }))
    }

    /// REQ-AXO-901947 slice 2 — enrich the reactive repair form with dynamic
    /// `valid_values` the static schema cannot carry. Today: a `project`/
    /// `project_code` field gets the registered project codes (capped), so a
    /// wrong-project failure surfaces the valid set in the same response instead
    /// of forcing a `project_registry_lookup` round-trip. Closed-enum fields keep
    /// their schema-derived values (set in `parameter_form_from_schema`); this
    /// only fills fields that have no `valid_values` yet. Cheap: one indexed
    /// registry SELECT, only on the (rare) validation-failure path.
    pub(crate) fn enrich_form_dynamic_values(&self, form: &mut [Value]) {
        let has_project_field = form.iter().any(|f| {
            matches!(
                f.get("name").and_then(Value::as_str),
                Some("project") | Some("project_code")
            ) && f.get("valid_values").is_none()
        });
        if !has_project_field {
            return;
        }
        let raw = self
            .graph_store
            .query_json(
                "SELECT project_code FROM soll.ProjectCodeRegistry \
                 WHERE project_code <> '' ORDER BY project_code ASC LIMIT 20",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let codes: Vec<Value> = rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .filter(|value| value.as_str().map(|s| !s.is_empty()).unwrap_or(false))
            .collect();
        if codes.is_empty() {
            return;
        }
        for field in form.iter_mut() {
            if matches!(
                field.get("name").and_then(Value::as_str),
                Some("project") | Some("project_code")
            ) && field.get("valid_values").is_none()
            {
                field["valid_values"] = Value::Array(codes.clone());
            }
        }
    }

    fn infer_dispatch_guidance_outcome(response: &Value) -> GuidanceOutcome {
        let response_text = response
            .get("content")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default();

        if response_text.contains("Invalid arguments for tool") {
            return classify_guidance(&[GuidanceFact::problem_signal("invalid_arguments")]);
        }

        if response_text.contains("Tool not found") {
            return classify_guidance(&[GuidanceFact::problem_signal("unknown_tool_name")]);
        }

        GuidanceOutcome::none()
    }

    fn attach_default_tool_guidance(
        &self,
        normalized_name: &str,
        arguments: &Value,
        mut response: Value,
    ) -> Value {
        let response_text = response
            .get("content")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let is_error = response.get("isError").and_then(Value::as_bool) == Some(true);
        let default_next_action =
            Self::default_next_action_for(normalized_name, arguments, &response);
        let dispatch_guidance = Self::infer_dispatch_guidance_outcome(&response);
        if dispatch_guidance != GuidanceOutcome::none() {
            if Self::mcp_guidance_authoritative_enabled() {
                response = attach_guidance_authoritative(response, dispatch_guidance.clone());
            } else if Self::mcp_guidance_shadow_enabled() {
                response =
                    attach_guidance_shadow(response, guidance_outcome_to_value(&dispatch_guidance));
            }
        }

        let Some(object) = response.as_object_mut() else {
            return response;
        };

        let data = object
            .entry("data".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !data.is_object() {
            *data = json!({ "value": data.clone() });
        }
        let Some(data_object) = data.as_object_mut() else {
            return response;
        };

        let next_action = data_object
            .get("next_action")
            .cloned()
            .or_else(|| {
                data_object
                    .get("operator_guidance")
                    .and_then(|value| value.get("next_action"))
                    .cloned()
            })
            .unwrap_or(default_next_action);

        if !data_object.contains_key("next_action") {
            data_object.insert("next_action".to_string(), next_action.clone());
        }
        data_object
            .entry("canonical_sources".to_string())
            .or_insert_with(Self::canonical_sources_snapshot);

        let status = data_object
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let is_partial = status.starts_with("warn_")
            || status.contains("degraded")
            || data_object.get("problem_class").and_then(Value::as_str) == Some("degraded");

        // REQ-AXO-901947 invariant 4 — terse by default (just-in-time minimal at
        // the recency edge). A clean success carries only the answer + `next_action`
        // (+ canonical_sources). The full guidance envelope is PULL: attached on
        // error/partial (the just-in-time safety net, where attention is reliable)
        // or on explicit opt-in (`detail`/`guidance` = "full", or
        // AXON_MCP_GUIDANCE_FULL). Pushing it on every response is a per-call token
        // tax that also pollutes the LLM's mid-context (lost-in-the-middle). The
        // filet reste tirable → no capability loss for a lost LLM.
        let attach_full = Self::mcp_guidance_full_enabled()
            || is_error
            || is_partial
            || matches!(
                arguments.get("detail").and_then(Value::as_str),
                Some("full")
            )
            || matches!(
                arguments.get("guidance").and_then(Value::as_str),
                Some("full")
            );
        if attach_full {
            let follow_up_tools = Self::default_follow_up_tools_for(normalized_name)
                .iter()
                .map(|tool| Value::String((*tool).to_string()))
                .collect::<Vec<_>>();
            let recommended_action = next_action
                .get("tool")
                .and_then(Value::as_str)
                .map(|tool| {
                    if is_error {
                        format!(
                            "follow `{tool}` or retry `{normalized_name}` with corrected inputs"
                        )
                    } else if is_partial {
                        format!("treat this as partial truth and continue with `{tool}`")
                    } else {
                        format!("continue the workflow with `{tool}`")
                    }
                })
                .unwrap_or_else(|| "continue with the next guided MCP step".to_string());
            let default_blocking_factors = if is_error || is_partial {
                vec![json!({
                    "factor": if is_error { "response_requires_recovery" } else { "partial_truth_requires_follow_up" },
                    "severity": if is_error { "high" } else { "medium" },
                    "recommended_action": recommended_action
                })]
            } else {
                Vec::new()
            };
            let default_remediation_actions = default_blocking_factors
                .iter()
                .filter_map(|factor| {
                    factor
                        .get("recommended_action")
                        .and_then(Value::as_str)
                        .map(|value| Value::String(value.to_string()))
                })
                .collect::<Vec<_>>();

            let operator_guidance = data_object
                .entry("operator_guidance".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if !operator_guidance.is_object() {
                *operator_guidance = Value::Object(serde_json::Map::new());
            }
            let Some(operator_guidance_object) = operator_guidance.as_object_mut() else {
                return response;
            };

            operator_guidance_object
                .entry("actionable_now".to_string())
                .or_insert(Value::Bool(!is_error && !is_partial));
            operator_guidance_object
                .entry("blocking_factors".to_string())
                .or_insert(Value::Array(default_blocking_factors));
            operator_guidance_object
                .entry("remediation_actions".to_string())
                .or_insert(Value::Array(default_remediation_actions));
            operator_guidance_object
                .entry("follow_up_tools".to_string())
                .or_insert(Value::Array(follow_up_tools));
            operator_guidance_object
                .entry("next_action".to_string())
                .or_insert(next_action);
            operator_guidance_object
                .entry("workflow_stage".to_string())
                .or_insert(Value::String(
                    Self::workflow_stage_for(normalized_name).to_string(),
                ));
            operator_guidance_object
                .entry("primary_goal".to_string())
                .or_insert(Value::String(
                    Self::primary_goal_for(normalized_name).to_string(),
                ));
            operator_guidance_object
                .entry("token_efficiency_hint".to_string())
                .or_insert(Value::String(
                    Self::token_efficiency_hint_for(normalized_name).to_string(),
                ));
            operator_guidance_object
                .entry("alternative_strategies".to_string())
                .or_insert(Value::Array(Self::alternative_strategies_for(
                    normalized_name,
                )));
            operator_guidance_object
            .entry("llm_usage_instruction".to_string())
            .or_insert(Value::String(
                "Use `next_action` first. Bad args: fix via `parameter_repair`, retry same tool once. Partial: label partial, use `follow_up_tools`. Do not ask the client to choose MCP tools; ask only for irreversible mutation, missing intent, or unrecoverable blocker.".to_string(),
            ));
            operator_guidance_object
                .entry("llm_contract".to_string())
                .or_insert(json!({
                    "first": "next_action",
                    "bad_args": "use parameter_repair, retry same tool once",
                    "partial": "label partial, keep evidence, use follow_up_tools[0]",
                    "no_answer": "switch once to the follow-up tool matching the missing dimension",
                    "ask_user_only_if": [
                        "irreversible_mutation",
                        "missing_business_intent",
                        "unrecoverable_high_severity_blocker"
                    ],
                    "token_rule": "prefer brief mode; escalate only after a named missing dimension"
                }));
            operator_guidance_object
                .entry("fallback_strategy".to_string())
                .or_insert(json!([
                    {
                        "if": "invalid_arguments",
                        "do": "fix args from `parameter_repair`; retry same tool"
                    },
                    {
                        "if": "unknown_or_wrong_target",
                        "do": "use candidates or broaden with `query`"
                    },
                    {
                        "if": "partial_or_degraded_truth",
                        "do": "state the gap; use first relevant follow-up tool"
                    },
                    {
                        "if": "no_structural_answer",
                        "do": "switch once to the follow-up tool matching the missing dimension"
                    }
                ]));
            operator_guidance_object
            .entry("explicit_input_rule".to_string())
            .or_insert(Value::String(
                "Do not ask the client to choose MCP tools. Ask only for irreversible mutation, missing intent, or unrecoverable high-severity blocker.".to_string(),
            ));
            operator_guidance_object
                .entry("why_this_next_step".to_string())
                .or_insert(Value::String(
                    if response_text.contains("Invalid arguments for tool") {
                        "Repair args from `parameter_repair`; do not widen search first."
                            .to_string()
                    } else if response_text.contains("Tool not found") {
                        "Inspect public MCP surface, then retry with a listed tool.".to_string()
                    } else if is_error {
                        "Recover with the guided next step before blind retries.".to_string()
                    } else if is_partial {
                        "Keep partial evidence; close the missing dimension next.".to_string()
                    } else {
                        "Highest-signal follow-up for this tool family.".to_string()
                    },
                ));
            if response_text.contains("Invalid arguments for tool") {
                operator_guidance_object
                .entry("parameter_repair".to_string())
                .or_insert_with(|| {
                    Self::parameter_repair_guidance_for(normalized_name, arguments)
                        .unwrap_or_else(|| json!({
                            "retry_rule": format!("Retry `{normalized_name}` with a schema-conformant argument object.")
                        }))
                });
            }
        } // end attach_full (REQ-AXO-901947 terse-default gate)

        response
    }

    fn async_known_ids_for(&self, normalized_name: &str, reserved_ids: &Value) -> Value {
        match normalized_name {
            "soll_apply_plan" => json!({
                "project_code": reserved_ids.get("project_code").cloned().unwrap_or(json!(null)),
                "preview_id": reserved_ids.get("preview_id").cloned().unwrap_or(json!(null))
            }),
            _ => reserved_ids.clone(),
        }
    }

    fn async_result_contract_for(&self, normalized_name: &str) -> Value {
        match normalized_name {
            "restore_soll" => json!({
                "follow_up_tool": "job_status",
                "terminal_state_field": "state",
                "raw_status_field": "status",
                "terminal_states": ["completed", "failed"],
                "result_data_fields": ["restored_nodes", "restored_edges", "source_path"],
                "notes": "Le résultat terminal expose le rapport de restauration SOLL."
            }),
            "resume_vectorization" => json!({
                "follow_up_tool": "job_status",
                "terminal_state_field": "state",
                "raw_status_field": "status",
                "terminal_states": ["completed", "failed"],
                "result_data_fields": ["queued_files", "runtime_mode", "semantic_workers_enabled"],
                "error_field": "error_text",
                "notes": "Le résultat terminal expose la taille du backlog re-queue et l'état du runtime."
            }),
            "soll_apply_plan" => json!({
                "follow_up_tool": "job_status",
                "terminal_state_field": "state",
                "raw_status_field": "status",
                "terminal_states": ["completed", "failed"],
                "result_data_fields": ["preview_id", "created", "updated", "skipped", "errors"],
                "notes": "Le résultat terminal expose le preview canonique et le rapport d'application."
            }),
            _ => json!({
                "follow_up_tool": "job_status",
                "terminal_state_field": "state",
                "raw_status_field": "status",
                "terminal_states": ["completed", "failed"],
                "result_data_fields": [],
                "notes": "Consultez le résultat terminal du job pour la charge utile finale."
            }),
        }
    }

    fn async_recovery_hint_for(&self, normalized_name: &str) -> String {
        match normalized_name {
            "restore_soll" => "Relancez `job_status(job_id)` jusqu'à l'état terminal. Si le job échoue, vérifiez le chemin d'export SOLL puis relancez `restore_soll`.".to_string(),
            "resume_vectorization" => "Relancez `job_status(job_id)` jusqu'à l'état terminal. Si le job échoue, inspectez l'état runtime puis relancez `resume_vectorization`.".to_string(),
            "soll_apply_plan" => "Relancez `job_status(job_id)` jusqu'à l'état terminal. Si le job échoue, corrigez le plan ou le `project_code`, puis relancez `soll_apply_plan`.".to_string(),
            _ => "Relancez `job_status(job_id)` jusqu'à l'état terminal. En cas d'échec, corrigez les arguments et relancez la mutation.".to_string(),
        }
    }

    fn async_polling_guidance_for(&self, normalized_name: &str) -> Value {
        let max_wait_seconds = match normalized_name {
            "soll_apply_plan" => 60,
            "restore_soll" => 60,
            "resume_vectorization" => 30,
            _ => 30,
        };
        json!({
            "when_to_poll": "Call `job_status(job_id=...)` after 2 seconds, then every 2 seconds until a terminal state.",
            "poll_interval_seconds": 2,
            "until_states": ["completed", "failed"],
            "max_wait_hint_seconds": max_wait_seconds,
            "on_completed": "Read `data.result.data` from the terminal `job_status` response.",
            "on_failed": "Read `data.error_text`, fix the arguments, then retry the original mutation."
        })
    }

    fn job_state(status: &str) -> &'static str {
        match status {
            "queued" => "queued",
            "running" => "running",
            "succeeded" => "completed",
            "failed" => "failed",
            _ => "unknown",
        }
    }

    fn terminal_result_data_alias(result: &Option<Value>) -> Value {
        result
            .as_ref()
            .and_then(|value| value.get("data"))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn should_refresh_derived_docs_for_tool(normalized_name: &str) -> bool {
        Self::SOLL_DERIVED_DOCS_REFRESH_TOOLS.contains(&normalized_name)
    }

    fn project_code_from_soll_entity_id(entity_id: &str) -> Option<String> {
        let mut parts = entity_id.split('-');
        let _prefix = parts.next()?;
        let project_code = parts.next()?.trim();
        if project_code.len() == 3
            && project_code
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() && !ch.is_ascii_lowercase())
        {
            Some(project_code.to_string())
        } else {
            None
        }
    }

    fn derive_docs_refresh_project_code(
        &self,
        normalized_name: &str,
        arguments: &Value,
        result: &Value,
    ) -> Option<String> {
        let candidate = result
            .get("data")
            .and_then(|value| value.get("project_code"))
            .and_then(|value| value.as_str())
            .or_else(|| {
                result
                    .get("data")
                    .and_then(|value| value.get("known_ids"))
                    .and_then(|value| value.get("project_code"))
                    .and_then(|value| value.as_str())
            })
            .or_else(|| {
                arguments
                    .get("project_code")
                    .and_then(|value| value.as_str())
            })
            .or_else(|| {
                arguments
                    .get("data")
                    .and_then(|value| value.get("project_code"))
                    .and_then(|value| value.as_str())
            });

        if let Some(project_code) = candidate {
            return self.resolve_project_code(project_code).ok();
        }

        match normalized_name {
            "soll_attach_evidence" | "soll_remove_evidence" => arguments
                .get("entity_id")
                .and_then(|value| value.as_str())
                .and_then(Self::project_code_from_soll_entity_id)
                .and_then(|project_code| self.resolve_project_code(&project_code).ok()),
            _ => None,
        }
    }

    fn attach_derived_docs_refresh_metadata(
        &self,
        normalized_name: &str,
        arguments: &Value,
        result: Value,
    ) -> Value {
        if !Self::should_refresh_derived_docs_for_tool(normalized_name)
            || result
                .get("isError")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        {
            return result;
        }

        let mut enriched = result;
        let refresh_payload = if let Some(project_code) =
            self.derive_docs_refresh_project_code(normalized_name, arguments, &enriched)
        {
            // DEC-AXO-091 / REQ-AXO-322 — drop the cached in-memory
            // snapshot for this project so the next hot-read call
            // reloads from PG and reflects this mutation.
            self.soll_cache.invalidate(&project_code);
            if let Some(site_root) = canonical_soll_site_dir() {
                // PIL-AXO-009 / REQ-AXO-901732 — in production the server is
                // `Arc`-wrapped and `self_arc` is set, so render the
                // non-canonical derived docs on a detached background thread:
                // the canonical mutation has already committed, and a slow
                // render must never block (or time out) its response. In unit
                // tests `self_arc` is unset → render synchronously so the
                // legacy "ok" + immediate-file contract still holds.
                match self.self_arc.get().and_then(std::sync::Weak::upgrade) {
                    Some(server) => self.schedule_background_derived_docs_refresh(
                        server,
                        project_code.clone(),
                        site_root,
                    ),
                    None => self.render_derived_docs_sync_payload(&project_code, &site_root),
                }
            } else {
                json!({
                    "status": "failed",
                    "project_code": project_code,
                    "stale_docs": true,
                    "error_text": "Impossible de résoudre docs/derived/soll pour le refresh automatique."
                })
            }
        } else {
            json!({
                "status": "skipped",
                "stale_docs": false,
                "reason": format!("No canonical project scope detected for `{}`.", normalized_name)
            })
        };

        if !enriched
            .get("data")
            .map(|value| value.is_object())
            .unwrap_or(false)
        {
            enriched["data"] = json!({});
        }
        enriched["data"]["derived_docs_refresh"] = refresh_payload;
        enriched
    }

    /// REQ-AXO-901732 — synchronous derived-docs render + response payload.
    /// Used in unit tests (server not `Arc`-wrapped) and as the body the
    /// background thread runs in production. Same logic as before; extracted
    /// so the sync and async paths share one implementation (DRY).
    fn render_derived_docs_sync_payload(
        &self,
        project_code: &str,
        site_root: &std::path::Path,
    ) -> Value {
        match self.generate_soll_derived_docs(
            project_code,
            Some(site_root),
            &site_root.join(project_code),
        ) {
            Ok(summary) => json!({
                "status": "ok",
                "project_code": summary.project_code,
                "site_root": if summary.site_root.is_empty() { Value::Null } else { json!(summary.site_root) },
                "output_root": summary.project_output_root,
                "manifest_path": summary.project_manifest_path,
                "root_manifest_path": if summary.root_manifest_path.is_empty() { Value::Null } else { json!(summary.root_manifest_path) },
                "root_index_path": if summary.root_index_path.is_empty() { Value::Null } else { json!(summary.root_index_path) },
                "refresh_mode": summary.refresh_mode,
                "pages_total": summary.pages_total,
                "pages_written": summary.pages_written,
                "pages_unchanged": summary.pages_unchanged,
                "pages_deleted": summary.pages_deleted,
                "deleted_paths": summary.deleted_paths,
                "root_written": summary.root_written,
                "stale_docs": summary.stale_docs,
            }),
            Err(error) => json!({
                "status": "failed",
                "project_code": project_code,
                "stale_docs": true,
                "error_text": error,
            }),
        }
    }

    /// REQ-AXO-901732 / PIL-AXO-009 — schedule the derived-docs render on a
    /// detached background thread and return immediately, so the canonical
    /// mutation response is never blocked by the non-canonical render. A
    /// coalescing guard keyed by `project_code` prevents thread pile-up: if a
    /// refresh for the same project is already in flight, the request is
    /// dropped (the in-flight render reads the latest committed state).
    fn schedule_background_derived_docs_refresh(
        &self,
        server: Arc<Self>,
        project_code: String,
        site_root: std::path::PathBuf,
    ) -> Value {
        {
            let mut inflight = self
                .derived_docs_refresh_inflight
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if !inflight.insert(project_code.clone()) {
                return json!({
                    "status": "coalesced",
                    "project_code": project_code,
                    "stale_docs": false,
                    "render": "background",
                });
            }
        }
        let inflight = self.derived_docs_refresh_inflight.clone();
        let render_lock = self.derived_docs_render_lock.clone();
        let pc = project_code.clone();
        std::thread::spawn(move || {
            let output_root = site_root.join(&pc);
            {
                // Serialize the actual render across all projects (shared root
                // index is rewritten by every project render; fs::write is not
                // atomic). The per-project guard already bounds thread count.
                let _render = render_lock
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let _ = server.generate_soll_derived_docs(&pc, Some(&site_root), &output_root);
            }
            inflight
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(&pc);
        });
        json!({
            "status": "scheduled",
            "project_code": project_code,
            "stale_docs": false,
            "render": "background",
        })
    }

    /// REQ-AXO-309 (DEC-AXO-901640) — regenerate the non-canonical derived docs
    /// for one project, reusing the shared inflight-coalescing set + render-lock
    /// (via `schedule_background_derived_docs_refresh`) so a journal-driven
    /// refresh never double-renders or races the per-tool-hook render. Invoked by
    /// the `soll_revision_committed` subscriber.
    pub(crate) fn regenerate_derived_docs_for(self: &Arc<Self>, project_code: String) {
        if let Some(site_root) = canonical_soll_site_dir() {
            let _ = self.schedule_background_derived_docs_refresh(
                self.clone(),
                project_code,
                site_root,
            );
        }
    }

    /// REQ-AXO-309 (DEC-AXO-901640) — spawn the SOLL revision-committed journal
    /// subscriber: regenerates the derived autodoc site on any SOLL mutation,
    /// decoupled from the per-tool hooks (one emitter / N subscribers). Call once
    /// at serve time after `init_self_arc`. The subscriber reuses THIS serving
    /// instance, so its render coalesces with the legacy hook.
    pub fn spawn_revision_docs_subscriber(self: &Arc<Self>, database_url: String) {
        revision_docs_listener::spawn(self.clone(), database_url);
    }

    #[allow(dead_code)]
    pub(crate) fn mcp_prewarm_enabled() -> bool {
        std::env::var("AXON_MCP_PREWARM")
            .ok()
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(true)
    }

    #[allow(dead_code)]
    pub(crate) fn mcp_blocking_prewarm_enabled() -> bool {
        std::env::var("AXON_MCP_PREWARM_BLOCKING")
            .ok()
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(true)
    }

    pub(crate) fn mcp_guidance_shadow_enabled() -> bool {
        std::env::var("AXON_MCP_GUIDANCE_SHADOW")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    /// REQ-AXO-901947 invariant 4 — env override forcing the full guidance
    /// envelope on every response (debug / legacy consumers). Default off: the
    /// surface is terse-by-default, full guidance is pulled on error/partial or
    /// via `detail`/`guidance` = "full".
    pub(crate) fn mcp_guidance_full_enabled() -> bool {
        std::env::var("AXON_MCP_GUIDANCE_FULL")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    pub(crate) fn mcp_guidance_authoritative_enabled() -> bool {
        std::env::var("AXON_MCP_GUIDANCE_AUTHORITATIVE")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    #[allow(dead_code)]
    pub(crate) fn startup_project_code(&self) -> Option<String> {
        let current_dir = std::env::current_dir().ok();
        let identities = discover_project_identities();
        current_dir
            .as_ref()
            .and_then(|dir| {
                identities
                    .iter()
                    .find(|identity| &identity.project_path == dir)
            })
            .or_else(|| identities.iter().find(|identity| identity.code == "AXO"))
            .or_else(|| identities.first())
            .map(|identity| identity.code.clone())
    }

    #[allow(dead_code)]
    pub(crate) fn startup_project_probe(&self) -> Option<(String, String, String)> {
        let project_code = self.startup_project_code()?;
        let escaped_project = project_code.replace('\'', "''");
        let query = format!(
            "SELECT id, name
             FROM Symbol
             WHERE project_code = '{escaped_project}'
               AND kind IN ('function', 'method')
             ORDER BY
               CASE
                 WHEN name = 'Axon.Scanner.scan' THEN 0
                 WHEN name = 'Axon.Watcher.Application.start' THEN 1
                 WHEN name = 'main' THEN 2
                 WHEN lower(name) LIKE '%scan%' THEN 3
                 WHEN lower(name) LIKE '%start%' THEN 4
                 ELSE 10
               END,
               tested ASC,
               name ASC
             LIMIT 1"
        );
        let raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let exact_symbol = rows.first()?.first()?.as_str()?.to_string();
        let symbol = rows.first()?.get(1)?.as_str()?.to_string();
        Some((project_code, symbol, exact_symbol))
    }

    #[allow(dead_code)]
    pub(crate) fn prewarm_observer_caches(&self) {
        if !Self::mcp_prewarm_enabled() {
            return;
        }
        let Some(project_code) = self.startup_project_code() else {
            let _ = self.axon_status(&json!({ "mode": "brief" }));
            return;
        };

        let _ = self.axon_status(&json!({ "mode": "brief" }));
        let _ = self.axon_anomalies(&json!({ "project": project_code, "mode": "brief" }));
        let _ = self.axon_soll_query_context(&json!({ "project_code": project_code, "limit": 5 }));
        let _ =
            self.axon_conception_view(&json!({ "project_code": project_code, "mode": "brief" }));
        let _ = self.axon_project_status(&json!({ "project_code": project_code, "mode": "brief" }));
        let Some((project_code, symbol, exact_symbol)) = self.startup_project_probe() else {
            return;
        };
        let _ = self.axon_retrieve_context(&json!({
            "project": project_code,
            "question": format!("Where is {} wired?", symbol),
            "token_budget": 900,
            "mode": "brief"
        }));
        let _ =
            self.axon_why(&json!({ "project": project_code, "symbol": symbol, "mode": "brief" }));
        let _ = self.axon_impact(
            &json!({ "project": project_code, "symbol": exact_symbol, "mode": "brief" }),
        );
        let _ = self.axon_change_safety(&json!({
            "project_code": project_code,
            "target": exact_symbol,
            "target_type": "symbol",
            "mode": "brief"
        }));
        let _ = self
            .axon_inspect(&json!({ "project": project_code, "symbol": symbol, "mode": "brief" }));
        let _ = self.axon_path(
            &json!({ "project": project_code, "source": exact_symbol, "mode": "brief" }),
        );
    }

    fn spawn_prewarm_threads(mcp_server: Arc<McpServer>) -> Vec<std::thread::JoinHandle<()>> {
        let primary = mcp_server.clone();
        let why_server = mcp_server.clone();
        vec![
            thread::spawn(move || {
                primary.prewarm_observer_caches();
            }),
            thread::spawn(move || {
                if let Some((project_code, symbol, _exact_symbol)) =
                    why_server.startup_project_probe()
                {
                    let _ = why_server.axon_why(
                        &json!({ "project": project_code, "symbol": symbol, "mode": "brief" }),
                    );
                }
            }),
        ]
    }

    pub fn startup_prewarm(mcp_server: Arc<McpServer>) {
        if !Self::mcp_prewarm_enabled() {
            return;
        }

        if Self::mcp_blocking_prewarm_enabled() {
            for handle in Self::spawn_prewarm_threads(mcp_server) {
                let _ = handle.join();
            }
            return;
        }

        for handle in Self::spawn_prewarm_threads(mcp_server) {
            std::mem::forget(handle);
        }
    }

    #[allow(dead_code)]
    pub async fn run_stdio(&self) -> Result<()> {
        let mut stdin = BufReader::new(tokio::io::stdin());
        let mut stdout = tokio::io::stdout();
        let mut line = String::new();

        while let Ok(bytes_read) = stdin.read_line(&mut line).await {
            if bytes_read == 0 {
                break;
            }

            match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(request) => {
                    let response = self.handle_request(request);
                    let mut response_str = serde_json::to_string(&response)?;
                    response_str.push('\n');
                    let _ = stdout.write_all(response_str.as_bytes()).await;
                }
                Err(e) => {
                    let error_response = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(json!({
                            "code": -32700,
                            "message": "Parse error",
                            "data": e.to_string()
                        })),
                        id: None,
                    };
                    if let Ok(mut response_str) = serde_json::to_string(&error_response) {
                        response_str.push('\n');
                        let _ = stdout.write_all(response_str.as_bytes()).await;
                    }
                }
            }
            line.clear();
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn send_notification(&self, method: &str, params: Option<Value>) -> JsonRpcNotification {
        JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        }
    }

    pub fn execute_raw_sql(&self, query: &str) -> anyhow::Result<String> {
        self.graph_store.execute_raw_sql_gateway(query)
    }

    pub(crate) fn mcp_mutation_jobs_enabled() -> bool {
        std::env::var("AXON_MCP_MUTATION_JOBS")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    pub(crate) fn is_async_job_tool(name: &str) -> bool {
        Self::ASYNC_JOB_TOOL_NAMES.contains(&name)
    }

    #[allow(dead_code)]
    pub(crate) fn is_mutating_tool(name: &str) -> bool {
        matches!(
            name,
            "restore_soll"
                | "soll_apply_plan"
                | "soll_commit_revision"
                | "soll_attach_evidence"
                | "soll_remove_evidence"
                | "soll_rollback_revision"
                | "soll_export"
                | "soll_generate_docs"
                | "soll_manager"
                | "entrench_nuance"
                | "apply_guidelines"
                | "commit_work"
                | "resume_vectorization"
        )
    }

    pub(crate) fn execute_tool_direct(
        &self,
        normalized_name: &str,
        arguments: &Value,
    ) -> Option<Value> {
        match normalized_name {
            "help" => self.axon_help(arguments),
            "fs_read" => self.axon_fs_read(arguments),
            "restore_soll" => self.axon_restore_soll(arguments),
            "soll_validate" => self.axon_validate_soll(arguments),
            "soll_acyclic_audit" => self.axon_soll_acyclic_audit(arguments),
            "ist_snapshot_warm" => self.axon_ist_snapshot_warm(arguments),
            "ist_centrality_pagerank" => self.axon_ist_centrality_pagerank(arguments),
            "ist_structural_sccs" => self.axon_ist_structural_sccs(arguments),
            "ist_shortest_path" => self.axon_ist_shortest_path(arguments),
            "infer_soll_mutation" => self.axon_infer_soll_mutation(arguments),
            "entrench_nuance" => self.axon_entrench_nuance(arguments),
            "soll_apply_plan" => self.axon_soll_apply_plan(arguments),
            "soll_commit_revision" => self.axon_soll_commit_revision(arguments),
            "soll_query_context" => self.axon_soll_query_context(arguments),
            "soll_work_plan" => self.axon_soll_work_plan(arguments),
            "soll_attach_evidence" => self.axon_soll_attach_evidence(arguments),
            "soll_remove_evidence" => self.axon_soll_remove_evidence(arguments),
            "document_intent" => self.axon_document_intent(arguments),
            "soll_verify_requirements" => self.axon_soll_verify_requirements(arguments),
            // REQ-AXO-902031 (N3) — queryable tech-debt inventory (migrations +
            // HAS_REMNANT remnants + progress).
            "tech_debt_inventory" => self.axon_tech_debt_inventory(arguments),
            // REQ-AXO-902051 — advisory residue detector: scans the IST
            // (code-anchored) and (re)links HAS_REMNANT for seeded migrations.
            "detect_remnants" => self.axon_detect_remnants(arguments),
            // REQ-AXO-902017 slice 1 — data-artifact catalog (data/CATALOG.json
            // pivot) inventory for data-centric projects.
            "data_catalog" => self.axon_data_catalog(arguments),
            "soll_rollback_revision" => self.axon_soll_rollback_revision(arguments),
            "retrieve_context" => self.axon_retrieve_context(arguments),
            // REQ-AXO-264 Phase A — layered envelope (intent + code + recent
            // bands in one MCP call). v0 wraps `axon_retrieve_context`; future
            // iterations will harden each band per CPT-AXO-050 philosophy.
            "retrieve_context_layered" => self.axon_retrieve_context_layered(arguments),
            "query" => self.axon_query(arguments),
            "soll_manager" => self.axon_soll_manager(arguments),
            "init_project" => self.axon_init_project(arguments),
            "apply_guidelines" => self.axon_apply_guidelines(arguments),
            "apply_methodology_bundle" => self.axon_apply_methodology_bundle(arguments),
            "commit_work" => self.axon_commit_work(arguments),
            "pre_flight_check" => self.axon_pre_flight_check(arguments),
            "soll_export" => self.axon_export_soll(arguments),
            "soll_generate_docs" => self.axon_soll_generate_docs(arguments),
            "diagnose_indexing" => self.axon_diagnose_indexing(arguments),
            "embedding_status" => self.axon_embedding_status(arguments),
            "embed_provider" => self.axon_embed_provider(arguments),
            "inspect" => self.axon_inspect(arguments),
            "audit" => self.axon_audit(arguments),
            "impact" => self.axon_impact(arguments),
            "fuse" => self.axon_fuse(arguments),
            "health" => self.axon_health(arguments),
            "status" => self.axon_status(arguments),
            "mcp_surface_diagnostics" => self.axon_mcp_surface_diagnostics(arguments),
            "project_status" => self.axon_project_status(arguments),
            "project_registry_lookup" => self.axon_project_registry_lookup(arguments),
            "soll_relation_schema" => self.axon_soll_relation_schema(arguments),
            "snapshot_history" => self.axon_snapshot_history(arguments),
            "snapshot_diff" => self.axon_snapshot_diff(arguments),
            "conception_view" => self.axon_conception_view(arguments),
            "change_safety" => self.axon_change_safety(arguments),
            "why" => self.axon_why(arguments),
            "path" => self.axon_path(arguments),
            "anomalies" => self.axon_anomalies(arguments),
            "diff" => self.axon_diff(arguments),
            "batch" => self.axon_batch(arguments),
            "sql" => self.axon_sql(arguments),
            "semantic_clones" => self.axon_semantic_clones(arguments),
            "architectural_drift" => self.axon_architectural_drift(arguments),
            "bidi_trace" => self.axon_bidi_trace(arguments),
            "api_break_check" => self.axon_api_break_check(arguments),
            "simulate_mutation" => self.axon_simulate_mutation(arguments),
            "debug" => self.axon_debug_with_args(arguments),
            "schema_overview" => self.axon_schema_overview(arguments),
            "query_examples" => self.axon_query_examples(arguments),
            // REQ-AXO-901957 — closed-loop friction report + resolution.
            "mcp_friction_report" => self.axon_mcp_friction_report(arguments),
            // REQ-AXO-901961 — usage + latency analytics over the call-stat rollup.
            "mcp_telemetry_report" => self.axon_mcp_telemetry_report(arguments),
            // REQ-AXO-901966 — voluntary LLM feedback / doléance (content-rich).
            "mcp_feedback" => self.axon_mcp_feedback(arguments),
            // REQ-AXO-902020 — content-rich READ/triage counterpart to mcp_feedback.
            "mcp_feedback_report" => self.axon_mcp_feedback_report(arguments),
            "truth_check" => self.axon_truth_check(arguments),
            "resume_vectorization" => self.axon_resume_vectorization(arguments),
            // REQ-AXO-901676 — proportionate recovery: force delta / full rescan
            // of a project subtree without restarting the indexer.
            "rescan_project" => self.axon_rescan_project(arguments),
            "job_status" => self.axon_job_status(arguments),
            // REQ-AXO-91580/91581 — SKI + PRT MCP surface.
            "skill_list" => self.axon_skill_list(arguments),
            "skill_invoke" => self.axon_skill_invoke(arguments),
            "prompt_template_get" => self.axon_prompt_template_get(arguments),
            // REQ-AXO-91582 — re_anchor for LLM autonomy + memory refresh.
            "re_anchor" => self.axon_re_anchor(arguments),
            _ => Some(
                json!({ "content": [{ "type": "text", "text": "Tool not found" }], "isError": true }),
            ),
        }
    }

    pub(crate) fn now_unix_ms() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};

        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    /// REQ-AXO-210: monotonic counter that pairs with `now_unix_ms` to
    /// build a job id immune to same-millisecond collisions. The
    /// previous `JOB-{ms}` form crashed the brain on PRIMARY_McpJob_0
    /// when two `mcp.submit_async_job` calls arrived within the same
    /// millisecond — observed deterministically on 2026-05-06 burst
    /// MCP submissions.
    fn next_job_seq() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        SEQ.fetch_add(1, Ordering::Relaxed)
    }

    /// REQ-AXO-210 — public for unit tests; combines monotonic ms with
    /// the atomic counter so two calls in the same ms differ in the
    /// suffix.
    pub(crate) fn next_job_id() -> String {
        let ms = Self::now_unix_ms();
        let seq = Self::next_job_seq();
        format!("JOB-{ms}-{seq:08}")
    }

    fn reserve_mutation_ids(&self, normalized_name: &str, arguments: &Value) -> Value {
        match normalized_name {
            "soll_apply_plan" => {
                let Some(project_code) = arguments
                    .get("project_code")
                    .and_then(|value| value.as_str())
                else {
                    return json!({
                        "reservation_error": "`project_code` est obligatoire pour `soll_apply_plan`. Le serveur attribue ensuite `preview_id`."
                    });
                };
                match self.next_server_numeric_id(project_code, "preview") {
                    Ok((canonical_project_code, project_code, _, next_num)) => json!({
                        "project_code": canonical_project_code,
                        "preview_id": format!("PRV-{project_code}-{next_num:03}")
                    }),
                    Err(error) => json!({ "reservation_error": error.to_string() }),
                }
            }
            _ => json!({}),
        }
    }

    fn inject_reserved_ids(
        &self,
        normalized_name: &str,
        arguments: &Value,
        reserved_ids: &Value,
    ) -> Value {
        let mut patched = arguments.clone();
        match normalized_name {
            "soll_apply_plan" => {
                if let Some(preview_id) = reserved_ids
                    .get("preview_id")
                    .and_then(|value| value.as_str())
                {
                    patched["reserved_preview_id"] = json!(preview_id);
                }
            }
            _ => {}
        }
        patched
    }

    fn persist_mcp_job(
        graph_store: &GraphStore,
        job_id: &str,
        tool_name: &str,
        status: &str,
        submitted_at: i64,
        request_json: &Value,
        reserved_ids_json: &Value,
    ) -> anyhow::Result<()> {
        graph_store.execute_param(
            "INSERT INTO soll.McpJob (job_id, tool_name, status, submitted_at, request_json, reserved_ids_json) VALUES (?, ?, ?, ?, ?, ?)",
            &json!([
                job_id,
                tool_name,
                status,
                submitted_at,
                request_json.to_string(),
                reserved_ids_json.to_string()
            ]),
        )
    }

    fn launch_mutation_job(
        &self,
        requested_tool_name: &str,
        normalized_name: &str,
        arguments: &Value,
    ) -> Option<Value> {
        let reserved_ids = self.reserve_mutation_ids(normalized_name, arguments);
        if let Some(error) = reserved_ids
            .get("reservation_error")
            .and_then(|value| value.as_str())
        {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("Mutation job reservation failed: {error}\nAction suivante: fournissez le scope projet canonique requis (`project_code`) ou l'identifiant serveur attendu (`preview_id`), puis relancez la mutation.") }],
                "isError": true
            }));
        }

        let submitted_at = Self::now_unix_ms();
        // REQ-AXO-210: collision-proof job id (atomic seq paired with ms).
        let job_id = Self::next_job_id();
        let public_tool_name = Self::public_tool_name_for(requested_tool_name, normalized_name);
        let known_ids = self.async_known_ids_for(normalized_name, &reserved_ids);
        let mut request_json = json!({
            "tool_name": public_tool_name,
            "arguments": arguments,
        });
        let mut proxy_response_data: Option<Value> = None;
        let proxy_request =
            if normalized_name == "resume_vectorization" && RuntimeCommandProxy::enabled() {
                Some(RuntimeCommandProxy::request_for_resume_vectorization(
                    arguments,
                ))
            } else {
                None
            };
        let proxy_ownership = proxy_request
            .as_ref()
            .map(RuntimeCommandProxy::ownership_for_request);
        let proxy_timeout = proxy_request
            .as_ref()
            .map(RuntimeCommandProxy::timeout_for_request);
        let proxy_retry_policy = proxy_request
            .as_ref()
            .map(|request| RuntimeCommandProxy::retry_policy_for_timeout(request.timeout_ms));
        let proxy_result_contract = proxy_request
            .as_ref()
            .map(|_| RuntimeCommandProxy::result_contract_for_resume_vectorization());
        if let Some(request) = proxy_request.as_ref() {
            let runtime_truth = crate::service_guard::current_runtime_truth_feed();
            match RuntimeCommandProxy::decision_for_resume_vectorization(&runtime_truth, arguments)
            {
                RuntimeCommandProxyDecision::Refused(refusal) => {
                    let ownership = proxy_ownership.as_ref().unwrap();
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "Runtime command proxy refused `{}` because the indexer feed is {}. Refresh runtime truth before retrying.",
                                public_tool_name,
                                refusal.reason
                            )
                        }],
                        "isError": true,
                        "data": {
                            "outcome": "refused",
                            "request": request,
                            "ownership": ownership,
                            "refusal": refusal,
                            "retry_policy": {
                                "retryable": refusal.retryable,
                                "max_attempts": 0,
                                "idempotent": true,
                                "duplicate_execution_prevented": true,
                                "recommended_delay_ms": refusal.stale_after_ms
                            }
                        }
                    }));
                }
                RuntimeCommandProxyDecision::TimedOut(timeout) => {
                    let ownership = proxy_ownership.as_ref().unwrap();
                    let retry_policy = proxy_retry_policy.as_ref().unwrap();
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "Runtime command proxy timed out after {} ms in {} mode while targeting `{}`. The timeout is simulated_test_only and does not represent a measured indexer latency.",
                                timeout.timeout_ms,
                                timeout.timeout_kind,
                                public_tool_name
                            )
                        }],
                        "isError": true,
                        "data": {
                            "outcome": "timeout",
                            "request": request,
                            "ownership": ownership,
                            "timeout": timeout,
                            "retry_policy": retry_policy,
                            "result_contract": proxy_result_contract.as_ref().unwrap()
                        }
                    }));
                }
                RuntimeCommandProxyDecision::Accepted(accepted) => {
                    request_json["runtime_command_proxy"] = json!({
                        "enabled": true,
                        "mode": accepted.proxy["mode"].clone(),
                        "transport": accepted.proxy["transport"].clone(),
                        "target_role": "indexer",
                        "timeout_kind": accepted.timeout.timeout_kind.clone(),
                        "request": accepted.request.clone(),
                        "ownership": accepted.ownership.clone(),
                        "timeout": accepted.timeout.clone(),
                        "retry_policy": accepted.retry_policy.clone(),
                        "result_contract": accepted.result_contract.clone()
                    });
                    proxy_response_data = Some(json!({
                        "request": request,
                        "ownership": proxy_ownership.as_ref().unwrap(),
                        "timeout": proxy_timeout.as_ref().unwrap(),
                        "retry_policy": proxy_retry_policy.as_ref().unwrap(),
                        "runtime_command_proxy": {
                            "enabled": true,
                            "mode": accepted.proxy["mode"].clone(),
                            "transport": accepted.proxy["transport"].clone(),
                            "target_role": "indexer",
                            "timeout_kind": accepted.timeout.timeout_kind.clone(),
                        }
                    }));
                }
            }
        }

        if let Err(error) = Self::persist_mcp_job(
            self.graph_store.as_ref(),
            &job_id,
            &public_tool_name,
            "queued",
            submitted_at,
            &request_json,
            &reserved_ids,
        ) {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("Failed to enqueue mutation job: {error}") }],
                "isError": true
            }));
        }

        let graph_store = self.graph_store.clone();
        let normalized_name = normalized_name.to_string();
        let response_contract_name = normalized_name.clone();
        let accepted_tool_name = public_tool_name.clone();
        let queued_args = self.inject_reserved_ids(&normalized_name, arguments, &reserved_ids);
        let job_id_for_thread = job_id.clone();
        let proxy_request_for_thread = proxy_request.clone();
        thread::spawn(move || {
            let server = McpServer::new(graph_store.clone());
            let started_at = McpServer::now_unix_ms();
            let _ = graph_store.execute_param(
                "UPDATE soll.McpJob SET status = $status, started_at = $started_at WHERE job_id = $job_id",
                &json!({
                    "status": "running",
                    "started_at": started_at,
                    "job_id": job_id_for_thread
                }),
            );

            let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if cfg!(test)
                    && std::env::var("AXON_RUNTIME_COMMAND_PROXY_TEST_PANIC")
                        .ok()
                        .as_deref()
                        == Some("1")
                    && normalized_name == "resume_vectorization"
                {
                    panic!("runtime_command_proxy_test_panic");
                }
                if normalized_name == "resume_vectorization"
                    && proxy_request_for_thread.is_some()
                    && RuntimeCommandProxy::use_external_bridge()
                {
                    match RuntimeCommandProxy::dispatch_resume_vectorization(
                        proxy_request_for_thread.as_ref().unwrap(),
                    ) {
                        Ok(result) => Some(result),
                        Err(error) => Some(json!({
                            "content": [{ "type": "text", "text": format!("Runtime command proxy bridge error: {error}") }],
                            "isError": true
                        })),
                    }
                } else {
                    server.execute_tool_direct(&normalized_name, &queued_args)
                }
            }));

            match execution {
                Ok(Some(result)) => {
                    let result = server.attach_derived_docs_refresh_metadata(
                        &normalized_name,
                        &queued_args,
                        result,
                    );
                    let finished_at = McpServer::now_unix_ms();
                    let is_error = result
                        .get("isError")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false);
                    let status = if is_error { "failed" } else { "succeeded" };
                    let error_text = if is_error {
                        result
                            .get("content")
                            .and_then(|value| value.as_array())
                            .and_then(|items| items.first())
                            .and_then(|item| item.get("text"))
                            .and_then(|value| value.as_str())
                            .unwrap_or("Mutation job failed")
                            .to_string()
                    } else {
                        String::new()
                    };
                    let _ = graph_store.execute_param(
                        "UPDATE soll.McpJob SET status = $status, finished_at = $finished_at, result_json = $result_json, error_text = $error_text WHERE job_id = $job_id",
                        &json!({
                            "status": status,
                            "finished_at": finished_at,
                            "result_json": result.to_string(),
                            "error_text": error_text,
                            "job_id": job_id_for_thread
                        }),
                    );
                }
                Ok(None) => {
                    let finished_at = McpServer::now_unix_ms();
                    let _ = graph_store.execute_param(
                        "UPDATE soll.McpJob SET status = $status, finished_at = $finished_at, error_text = $error_text WHERE job_id = $job_id",
                        &json!({
                            "status": "failed",
                            "finished_at": finished_at,
                            "error_text": format!("Invalid arguments for tool: {normalized_name}"),
                            "job_id": job_id_for_thread
                        }),
                    );
                }
                Err(_) => {
                    let finished_at = McpServer::now_unix_ms();
                    let _ = graph_store.execute_param(
                        "UPDATE soll.McpJob SET status = $status, finished_at = $finished_at, error_text = $error_text WHERE job_id = $job_id",
                        &json!({
                            "status": "failed",
                            "finished_at": finished_at,
                            "error_text": format!("Mutation worker panicked while running `{normalized_name}`"),
                            "job_id": job_id_for_thread
                        }),
                    );
                }
            }
        });

        let mut data = json!({
            "accepted": true,
            "job_id": job_id,
            "tool_name": accepted_tool_name,
            "status": "queued",
            "state": "queued",
            "reserved_ids": reserved_ids,
            "known_ids": known_ids,
            "next_action": {
                "tool": "job_status",
                "arguments": {
                    "job_id": job_id
                }
            },
            "result_contract": proxy_result_contract
                .as_ref()
                .cloned()
                .unwrap_or_else(|| self.async_result_contract_for(response_contract_name.as_str())),
            "polling_guidance": self.async_polling_guidance_for(response_contract_name.as_str()),
            "recovery_hint": self.async_recovery_hint_for(response_contract_name.as_str())
        });
        if let Some(proxy_data) = proxy_response_data {
            data["request"] = proxy_data.get("request").cloned().unwrap_or(Value::Null);
            data["ownership"] = proxy_data.get("ownership").cloned().unwrap_or(Value::Null);
            data["timeout"] = proxy_data.get("timeout").cloned().unwrap_or(Value::Null);
            data["retry_policy"] = proxy_data
                .get("retry_policy")
                .cloned()
                .unwrap_or(Value::Null);
            data["runtime_command_proxy"] = proxy_data
                .get("runtime_command_proxy")
                .cloned()
                .unwrap_or(Value::Null);
        }
        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Mutation job accepted: {job_id} for tool `{accepted_tool_name}`. Call `job_status(job_id=\"{job_id}\")` after 2 seconds, then every 2 seconds until `state=completed` or `state=failed`."
                )
            }],
            "data": data
        }))
    }

    pub(crate) fn axon_job_status(&self, args: &Value) -> Option<Value> {
        let job_id = args.get("job_id")?.as_str()?;
        // REQ-AXO-146 — optional event-driven wait. Default polling unchanged.
        // wait=true blocks the call until the job reaches a terminal state
        // (completed|failed) OR `timeout_ms` elapses, eliminating the need
        // for the LLM to issue N round-trips. Timeout returns a partial
        // snapshot with `data.next_action.kind = continue_polling_until_terminal_state`
        // so the existing polling guidance still applies.
        let wait = args.get("wait").and_then(|v| v.as_bool()).unwrap_or(false);
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(30_000);
        let poll_interval_ms = args
            .get("poll_interval_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(250)
            .max(10);
        let started = std::time::Instant::now();
        let mut polls = 0u64;

        loop {
            polls += 1;
            let mut snapshot = self.job_status_snapshot(job_id)?;
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let state_str = snapshot
                .pointer("/data/state")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let is_terminal = matches!(state_str.as_str(), "completed" | "failed");
            let timed_out = wait && elapsed_ms >= timeout_ms;
            if wait {
                let wait_meta = json!({
                    "wait": true,
                    "polls": polls,
                    "elapsed_ms": elapsed_ms,
                    "timeout_ms": timeout_ms,
                    "poll_interval_ms": poll_interval_ms,
                    "timed_out": timed_out,
                    "reached_terminal": is_terminal,
                });
                if let Some(data) = snapshot.get_mut("data").and_then(|v| v.as_object_mut()) {
                    data.insert("wait_metadata".to_string(), wait_meta);
                }
            }
            if !wait || is_terminal || timed_out {
                return Some(snapshot);
            }
            let remaining = timeout_ms.saturating_sub(elapsed_ms);
            let sleep_ms = poll_interval_ms.min(remaining).max(1);
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
        }
    }

    fn job_status_snapshot(&self, job_id: &str) -> Option<Value> {
        let rows = self
            .graph_store
            .query_json_param(
                "SELECT job_id, tool_name, status, submitted_at, started_at, finished_at, reserved_ids_json, request_json, result_json, error_text \
                 FROM soll.McpJob WHERE job_id = $job_id LIMIT 1",
                &json!({ "job_id": job_id }),
            )
            .ok()?;
        let parsed: Vec<Vec<Value>> = serde_json::from_str(&rows).ok()?;
        let row = parsed.first()?;
        let tool_name = row
            .get(1)
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let raw_status = row
            .get(2)
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let state = Self::job_state(raw_status);
        let reserved_ids = row
            .get(6)
            .and_then(|value| value.as_str())
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
            .unwrap_or_else(|| json!({}));
        let known_ids = self.async_known_ids_for(tool_name, &reserved_ids);
        let request_json = row
            .get(7)
            .and_then(|value| value.as_str())
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
            .unwrap_or_else(|| json!({}));
        let proxy_contract = request_json.get("runtime_command_proxy").cloned();
        let request_value = Some(request_json.clone());
        let ownership_value = proxy_contract
            .as_ref()
            .and_then(|value| value.get("ownership"))
            .cloned();
        let timeout_value = proxy_contract
            .as_ref()
            .and_then(|value| value.get("timeout"))
            .cloned();
        let retry_policy_value = proxy_contract
            .as_ref()
            .and_then(|value| value.get("retry_policy"))
            .cloned();
        let proxy_value = proxy_contract.clone();
        let result_contract = proxy_contract
            .as_ref()
            .and_then(|value| value.get("result_contract"))
            .cloned()
            .unwrap_or_else(|| self.async_result_contract_for(tool_name));
        let polling_guidance = self.async_polling_guidance_for(tool_name);
        let recovery_hint = self.async_recovery_hint_for(tool_name);
        let result = row
            .get(8)
            .and_then(|value| value.as_str())
            .and_then(|value| serde_json::from_str::<Value>(value).ok());
        let result_data = Self::terminal_result_data_alias(&result);
        let error_text = row.get(9).and_then(|value| value.as_str()).unwrap_or("");
        let next_action = match state {
            "queued" | "running" => json!({
                "tool": "job_status",
                "arguments": {
                    "job_id": job_id
                },
                "when": "continue_polling_until_terminal_state"
            }),
            "completed" => json!({
                "kind": "read_terminal_result",
                "path": "data.result.data",
                "when": "now"
            }),
            "failed" => json!({
                "kind": "fix_and_retry_original_mutation",
                "when": "after_reviewing_error_text"
            }),
            _ => Value::Null,
        };

        let mut data = json!({
            "job_id": row.first().and_then(|value| value.as_str()).unwrap_or(job_id),
            "tool_name": tool_name,
            "status": raw_status,
            "state": state,
            "submitted_at": row.get(3).cloned().unwrap_or(json!(null)),
            "started_at": row.get(4).cloned().unwrap_or(json!(null)),
            "finished_at": row.get(5).cloned().unwrap_or(json!(null)),
            "reserved_ids": reserved_ids,
            "known_ids": known_ids,
            "next_action": next_action,
            "result_contract": result_contract,
            "polling_guidance": polling_guidance,
            "recovery_hint": recovery_hint,
            "result": result,
            "result_data": result_data,
            "error_text": error_text
        });
        if proxy_contract.is_some() {
            data["request"] = request_value.unwrap_or(Value::Null);
            data["ownership"] = ownership_value.unwrap_or(Value::Null);
            data["timeout"] = timeout_value.unwrap_or(Value::Null);
            data["retry_policy"] = retry_policy_value.unwrap_or(Value::Null);
            data["runtime_command_proxy"] = proxy_value.unwrap_or(Value::Null);
        }

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Job {} status={} tool={}",
                    row.first().and_then(|value| value.as_str()).unwrap_or(job_id),
                    raw_status,
                    tool_name
                )
            }],
            "data": data
        }))
    }

    pub fn handle_notification(&self, request: JsonRpcRequest) -> bool {
        if request.id.is_some() {
            return false;
        }

        matches!(request.method.as_str(), "notifications/initialized")
    }

    pub fn negotiate_protocol_version(request: &JsonRpcRequest) -> &'static str {
        let requested = request
            .params
            .as_ref()
            .and_then(|params| params.get("protocolVersion"))
            .and_then(|value| value.as_str());

        if let Some(version) = requested {
            if let Some(supported) = SUPPORTED_MCP_PROTOCOL_VERSIONS
                .iter()
                .copied()
                .find(|supported| *supported == version)
            {
                return supported;
            }
        }

        SUPPORTED_MCP_PROTOCOL_VERSIONS[0]
    }

    pub fn handle_request(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if request.id.is_none() {
            return None;
        }

        let result = match request.method.as_str() {
            "initialize" => Some(json!({
                "protocolVersion": Self::negotiate_protocol_version(&request),
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": { "name": mcp_server_identity_name(), "version": "2.2.0" }
            })),
            "tools/list" => {
                let include_internal = request
                    .params
                    .as_ref()
                    .and_then(|params| params.get("include_internal"))
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                Some(tools_catalog(include_internal))
            }
            "tools/call" => self.handle_call_tool(request.params),
            _ => None,
        };

        if let Some(res) = result {
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(res),
                error: None,
                id: request.id,
            })
        } else {
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(json!({
                    "code": -32601,
                    "message": "Method not found"
                })),
                id: request.id,
            })
        }
    }
}
