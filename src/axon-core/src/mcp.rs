use crate::graph::GraphStore;
use crate::project_meta::discover_project_identities;
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
mod soll;
#[cfg(test)]
mod tests;
mod tools_context;
mod tools_dx;
mod tools_framework;
mod tools_governance;
mod tools_risk;
mod tools_soll;
mod tools_system;

use self::catalog::tools_catalog;
#[allow(unused_imports)]
pub(crate) use self::guidance::{
    attach_guidance_authoritative, attach_guidance_shadow, build_guided_response,
    classify_guidance, guidance_outcome_to_value, project_authoritative_phase1_guidance,
    GuidanceCandidates, GuidanceFact, GuidanceOutcome, SollGuidance,
};
pub use self::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

pub struct McpServer {
    graph_store: Arc<GraphStore>,
}

const SUPPORTED_MCP_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

impl McpServer {
    pub fn new(graph_store: Arc<GraphStore>) -> Self {
        Self { graph_store }
    }

    fn public_tool_name_for(requested_name: &str, normalized_name: &str) -> String {
        if requested_name.trim().is_empty() {
            return normalized_name.to_string();
        }
        requested_name.to_string()
    }

    fn async_known_ids_for(&self, normalized_name: &str, reserved_ids: &Value) -> Value {
        match normalized_name {
            "soll_manager" => json!({
                "project_code": reserved_ids.get("project_code").cloned().unwrap_or(json!(null)),
                "entity_id": reserved_ids.get("entity_id").cloned().unwrap_or(json!(null))
            }),
            "soll_apply_plan" => json!({
                "project_code": reserved_ids.get("project_code").cloned().unwrap_or(json!(null)),
                "preview_id": reserved_ids.get("preview_id").cloned().unwrap_or(json!(null))
            }),
            "soll_commit_revision" => json!({
                "project_code": reserved_ids.get("project_code").cloned().unwrap_or(json!(null)),
                "revision_id": reserved_ids.get("revision_id").cloned().unwrap_or(json!(null))
            }),
            "init_project" => json!({
                "project_code": reserved_ids.get("project_code").cloned().unwrap_or(json!(null)),
                "project_name": reserved_ids.get("project_name").cloned().unwrap_or(json!(null)),
                "project_path": reserved_ids.get("project_path").cloned().unwrap_or(json!(null))
            }),
            _ => reserved_ids.clone(),
        }
    }

    fn async_result_contract_for(&self, normalized_name: &str) -> Value {
        match normalized_name {
            "init_project" => json!({
                "follow_up_tool": "job_status",
                "terminal_state_field": "state",
                "raw_status_field": "status",
                "terminal_states": ["completed", "failed"],
                "result_data_fields": ["project_code", "project_name", "project_path"],
                "notes": "Le résultat terminal expose l'identité projet canonique attribuée par le serveur."
            }),
            "soll_apply_plan" => json!({
                "follow_up_tool": "job_status",
                "terminal_state_field": "state",
                "raw_status_field": "status",
                "terminal_states": ["completed", "failed"],
                "result_data_fields": ["preview_id", "created", "updated", "skipped", "errors"],
                "notes": "Le résultat terminal expose le preview canonique et le rapport d'application."
            }),
            "soll_commit_revision" => json!({
                "follow_up_tool": "job_status",
                "terminal_state_field": "state",
                "raw_status_field": "status",
                "terminal_states": ["completed", "failed"],
                "result_data_fields": ["revision_id", "preview_id", "status"],
                "notes": "Le résultat terminal expose la révision canonique créée par le serveur."
            }),
            "soll_manager" => json!({
                "follow_up_tool": "job_status",
                "terminal_state_field": "state",
                "raw_status_field": "status",
                "terminal_states": ["completed", "failed"],
                "result_data_fields": ["id", "status", "project_code"],
                "notes": "Le résultat terminal expose l'entité canonique créée ou modifiée."
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
            "init_project" => "Relancez `job_status(job_id)` jusqu'à `completed` ou `failed`. Si le job échoue, corrigez les arguments projet puis relancez `axon_init_project`.".to_string(),
            "soll_apply_plan" => "Relancez `job_status(job_id)` jusqu'à l'état terminal. Si le job échoue, corrigez le plan ou le `project_code`, puis relancez `soll_apply_plan`.".to_string(),
            "soll_commit_revision" => "Relancez `job_status(job_id)` jusqu'à l'état terminal. Si le job échoue, vérifiez `preview_id` puis relancez `soll_commit_revision`.".to_string(),
            _ => "Relancez `job_status(job_id)` jusqu'à l'état terminal. En cas d'échec, corrigez les arguments et relancez la mutation.".to_string(),
        }
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

    fn project_code_from_preview_id(&self, preview_id: &str) -> Result<String> {
        let escaped_preview = preview_id.replace('\'', "''");
        let raw = self.graph_store.query_json(&format!(
            "SELECT payload FROM soll.RevisionPreview WHERE preview_id = '{escaped_preview}'"
        ))?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let payload_raw = rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .ok_or_else(|| anyhow::anyhow!("Preview introuvable: {}", preview_id))?;
        let payload: Value = serde_json::from_str(&payload_raw)
            .map_err(|error| anyhow::anyhow!("Preview invalide `{}`: {}", preview_id, error))?;
        payload
            .get("project_code")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Preview `{}` sans `project_code`: impossible de réserver `revision_id`",
                    preview_id
                )
            })
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

    pub(crate) fn is_mutating_tool(name: &str) -> bool {
        matches!(
            name,
            "restore_soll"
                | "soll_apply_plan"
                | "soll_commit_revision"
                | "soll_attach_evidence"
                | "soll_rollback_revision"
                | "soll_export"
                | "soll_manager"
                | "init_project"
                | "apply_guidelines"
                | "commit_work"
        )
    }

    pub(crate) fn execute_tool_direct(
        &self,
        normalized_name: &str,
        arguments: &Value,
    ) -> Option<Value> {
        match normalized_name {
            "refine_lattice" => self.axon_refine_lattice(arguments),
            "fs_read" => self.axon_fs_read(arguments),
            "restore_soll" => self.axon_restore_soll(arguments),
            "soll_validate" => self.axon_validate_soll(arguments),
            "soll_apply_plan" => self.axon_soll_apply_plan(arguments),
            "soll_commit_revision" => self.axon_soll_commit_revision(arguments),
            "soll_query_context" => self.axon_soll_query_context(arguments),
            "soll_work_plan" => self.axon_soll_work_plan(arguments),
            "soll_attach_evidence" => self.axon_soll_attach_evidence(arguments),
            "soll_verify_requirements" => self.axon_soll_verify_requirements(arguments),
            "soll_rollback_revision" => self.axon_soll_rollback_revision(arguments),
            "retrieve_context" => self.axon_retrieve_context(arguments),
            "query" => self.axon_query(arguments),
            "soll_manager" => self.axon_soll_manager(arguments),
            "init_project" => self.axon_init_project(arguments),
            "apply_guidelines" => self.axon_apply_guidelines(arguments),
            "commit_work" => self.axon_commit_work(arguments),
            "pre_flight_check" => self.axon_pre_flight_check(arguments),
            "soll_export" => self.axon_export_soll(arguments),
            "diagnose_indexing" => self.axon_diagnose_indexing(arguments),
            "inspect" => self.axon_inspect(arguments),
            "audit" => self.axon_audit(arguments),
            "impact" => self.axon_impact(arguments),
            "health" => self.axon_health(arguments),
            "status" => self.axon_status(arguments),
            "project_status" => self.axon_project_status(arguments),
            "project_registry_lookup" => self.axon_project_registry_lookup(arguments),
            "snapshot_history" => self.axon_snapshot_history(arguments),
            "snapshot_diff" => self.axon_snapshot_diff(arguments),
            "conception_view" => self.axon_conception_view(arguments),
            "change_safety" => self.axon_change_safety(arguments),
            "why" => self.axon_why(arguments),
            "path" => self.axon_path(arguments),
            "anomalies" => self.axon_anomalies(arguments),
            "diff" => self.axon_diff(arguments),
            "batch" => self.axon_batch(arguments),
            "cypher" => self.axon_cypher(arguments),
            "semantic_clones" => self.axon_semantic_clones(arguments),
            "architectural_drift" => self.axon_architectural_drift(arguments),
            "bidi_trace" => self.axon_bidi_trace(arguments),
            "api_break_check" => self.axon_api_break_check(arguments),
            "simulate_mutation" => self.axon_simulate_mutation(arguments),
            "debug" => self.axon_debug_with_args(arguments),
            "schema_overview" => self.axon_schema_overview(arguments),
            "list_labels_tables" => self.axon_list_labels_tables(arguments),
            "query_examples" => self.axon_query_examples(arguments),
            "truth_check" => self.axon_truth_check(arguments),
            "resume_vectorization" => self.axon_resume_vectorization(arguments),
            "job_status" => self.axon_job_status(arguments),
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

    fn reserve_mutation_ids(&self, normalized_name: &str, arguments: &Value) -> Value {
        match normalized_name {
            "soll_manager" => {
                if arguments.get("action").and_then(|value| value.as_str()) != Some("create") {
                    return json!({});
                }
                let Some(entity) = arguments.get("entity").and_then(|value| value.as_str()) else {
                    return json!({});
                };
                let Some(project_code) = arguments
                    .get("data")
                    .and_then(|value| value.get("project_code"))
                    .and_then(|value| value.as_str())
                else {
                    return json!({
                        "reservation_error": "`project_code` est obligatoire pour `soll_manager create`. Le serveur attribue ensuite l'ID canonique."
                    });
                };
                match self.next_soll_numeric_id(project_code, entity) {
                    Ok((canonical_project_code, project_code, prefix, next_num)) => json!({
                        "project_code": canonical_project_code,
                        "entity_id": format!("{prefix}-{project_code}-{next_num:03}")
                    }),
                    Err(error) => json!({ "reservation_error": error.to_string() }),
                }
            }
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
            "soll_commit_revision" => {
                let Some(preview_id) = arguments.get("preview_id").and_then(|value| value.as_str())
                else {
                    return json!({
                        "reservation_error": "`preview_id` est obligatoire pour `soll_commit_revision`. Le serveur attribue ensuite `revision_id`."
                    });
                };
                let project_code = match self.project_code_from_preview_id(preview_id) {
                    Ok(code) => code,
                    Err(error) => return json!({ "reservation_error": error.to_string() }),
                };
                match self.next_server_numeric_id(&project_code, "revision") {
                    Ok((canonical_project_code, project_code, _, next_num)) => json!({
                        "project_code": canonical_project_code,
                        "revision_id": format!("REV-{project_code}-{next_num:03}")
                    }),
                    Err(error) => json!({ "reservation_error": error.to_string() }),
                }
            }
            "init_project" => {
                let Some(project_path) = arguments
                    .get("project_path")
                    .and_then(|value| value.as_str())
                else {
                    return json!({
                        "reservation_error": "`project_path` est obligatoire pour `axon_init_project`."
                    });
                };
                let project_name = match self.derive_project_name_from_path(project_path) {
                    Ok(name) => name,
                    Err(error) => return json!({ "reservation_error": error.to_string() }),
                };
                let project_code =
                    match self.assign_project_code_for_init(&project_name, project_path) {
                        Ok(code) => code,
                        Err(error) => return json!({ "reservation_error": error.to_string() }),
                    };
                if let Some(requested_code) = arguments
                    .get("project_code")
                    .and_then(|value| value.as_str())
                {
                    match self.validate_explicit_canonical_project_code(
                        Some(requested_code),
                        "axon_init_project",
                    ) {
                        Ok(requested) if requested == project_code => {}
                        Ok(_) => {
                            return json!({
                                "reservation_error": format!("`project_code` est attribué par le serveur. Omettez-le ou utilisez `{}` pour ce projet.", project_code)
                            })
                        }
                        Err(error) => return json!({ "reservation_error": error.to_string() }),
                    }
                }
                json!({
                    "project_code": project_code,
                    "project_name": project_name,
                    "project_path": project_path
                })
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
            "soll_manager" => {
                if let Some(entity_id) = reserved_ids
                    .get("entity_id")
                    .and_then(|value| value.as_str())
                {
                    patched["reserved_id"] = json!(entity_id);
                }
            }
            "soll_apply_plan" => {
                if let Some(preview_id) = reserved_ids
                    .get("preview_id")
                    .and_then(|value| value.as_str())
                {
                    patched["reserved_preview_id"] = json!(preview_id);
                }
            }
            "soll_commit_revision" => {
                if let Some(revision_id) = reserved_ids
                    .get("revision_id")
                    .and_then(|value| value.as_str())
                {
                    patched["reserved_revision_id"] = json!(revision_id);
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
        let job_id = format!("JOB-{submitted_at}");
        let public_tool_name = Self::public_tool_name_for(requested_tool_name, normalized_name);
        let known_ids = self.async_known_ids_for(normalized_name, &reserved_ids);
        let request_json = json!({
            "tool_name": public_tool_name,
            "arguments": arguments,
        });

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

            match server.execute_tool_direct(&normalized_name, &queued_args) {
                Some(result) => {
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
                None => {
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
            }
        });

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!("Mutation job accepted: {job_id} for tool `{accepted_tool_name}`")
            }],
            "data": {
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
                "result_contract": self.async_result_contract_for(response_contract_name.as_str()),
                "recovery_hint": self.async_recovery_hint_for(response_contract_name.as_str())
            }
        }))
    }

    pub(crate) fn axon_job_status(&self, args: &Value) -> Option<Value> {
        let job_id = args.get("job_id")?.as_str()?;
        let rows = self
            .graph_store
            .query_json_param(
                "SELECT job_id, tool_name, status, submitted_at, started_at, finished_at, reserved_ids_json, result_json, error_text \
                 FROM soll.McpJob WHERE job_id = $job_id LIMIT 1",
                &json!({ "job_id": job_id }),
            )
            .ok()?;
        let parsed: Vec<Vec<Value>> = serde_json::from_str(&rows).ok()?;
        let row = parsed.first()?;
        let reserved_ids = row
            .get(6)
            .and_then(|value| value.as_str())
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
            .unwrap_or_else(|| json!({}));
        let result = row
            .get(7)
            .and_then(|value| value.as_str())
            .and_then(|value| serde_json::from_str::<Value>(value).ok());
        let error_text = row.get(8).and_then(|value| value.as_str()).unwrap_or("");

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Job {} status={} tool={}",
                    row.first().and_then(|value| value.as_str()).unwrap_or(job_id),
                    row.get(2).and_then(|value| value.as_str()).unwrap_or("unknown"),
                    row.get(1).and_then(|value| value.as_str()).unwrap_or("unknown")
                )
            }],
            "data": {
                "job_id": row.first().and_then(|value| value.as_str()).unwrap_or(job_id),
                "tool_name": row.get(1).and_then(|value| value.as_str()).unwrap_or("unknown"),
                "status": row.get(2).and_then(|value| value.as_str()).unwrap_or("unknown"),
                "state": Self::job_state(row.get(2).and_then(|value| value.as_str()).unwrap_or("unknown")),
                "submitted_at": row.get(3).cloned().unwrap_or(json!(null)),
                "started_at": row.get(4).cloned().unwrap_or(json!(null)),
                "finished_at": row.get(5).cloned().unwrap_or(json!(null)),
                "reserved_ids": reserved_ids,
                "result": result,
                "error_text": error_text
            }
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
                "serverInfo": { "name": "axon-core", "version": "2.2.0" }
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
