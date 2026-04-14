use crate::graph::GraphStore;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use std::thread;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
mod catalog;
mod dispatch;
mod format;
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
pub use self::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

pub struct McpServer {
    graph_store: Arc<GraphStore>,
}

impl McpServer {
    pub fn new(graph_store: Arc<GraphStore>) -> Self {
        Self { graph_store }
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
                let project_code = arguments
                    .get("data")
                    .and_then(|value| value.get("project_code"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("AXO");
                match self.next_soll_numeric_id(project_code, entity) {
                    Ok((canonical_project_code, project_code, prefix, next_num)) => json!({
                        "project_code": canonical_project_code,
                        "entity_id": format!("{prefix}-{project_code}-{next_num:03}")
                    }),
                    Err(error) => json!({ "reservation_error": error.to_string() }),
                }
            }
            "soll_apply_plan" => {
                let project_code = arguments
                    .get("project_code")
                    .and_then(|value| value.as_str())
                    .unwrap_or("AXO");
                match self.next_server_numeric_id(project_code, "preview") {
                    Ok((canonical_project_code, project_code, _, next_num)) => json!({
                        "project_code": canonical_project_code,
                        "preview_id": format!("PRV-{project_code}-{next_num:03}")
                    }),
                    Err(error) => json!({ "reservation_error": error.to_string() }),
                }
            }
            "soll_commit_revision" => {
                let project_code = arguments
                    .get("project_code")
                    .and_then(|value| value.as_str())
                    .unwrap_or("AXO");
                match self.next_server_numeric_id(project_code, "revision") {
                    Ok((canonical_project_code, project_code, _, next_num)) => json!({
                        "project_code": canonical_project_code,
                        "revision_id": format!("REV-{project_code}-{next_num:03}")
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
            "soll_manager" => {
                if let Some(entity_id) = reserved_ids
                    .get("entity_id")
                    .and_then(|value| value.as_str())
                {
                    patched["reserved_id"] = json!(entity_id);
                }
                if let Some(project_code) = reserved_ids
                    .get("project_code")
                    .and_then(|value| value.as_str())
                {
                    patched["project_code"] = json!(project_code);
                }
            }
            "soll_apply_plan" => {
                if let Some(preview_id) = reserved_ids
                    .get("preview_id")
                    .and_then(|value| value.as_str())
                {
                    patched["reserved_preview_id"] = json!(preview_id);
                }
                if let Some(project_code) = reserved_ids
                    .get("project_code")
                    .and_then(|value| value.as_str())
                {
                    patched["project_code"] = json!(project_code);
                }
            }
            "soll_commit_revision" => {
                if let Some(revision_id) = reserved_ids
                    .get("revision_id")
                    .and_then(|value| value.as_str())
                {
                    patched["reserved_revision_id"] = json!(revision_id);
                }
                if reserved_ids.get("project_code").is_some()
                    && patched.get("project_code").is_none()
                {
                    if let Some(project_code) = reserved_ids
                        .get("project_code")
                        .and_then(|value| value.as_str())
                    {
                        patched["project_code"] = json!(project_code);
                    }
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

    fn launch_mutation_job(&self, normalized_name: &str, arguments: &Value) -> Option<Value> {
        let reserved_ids = self.reserve_mutation_ids(normalized_name, arguments);
        if let Some(error) = reserved_ids
            .get("reservation_error")
            .and_then(|value| value.as_str())
        {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("Mutation job reservation failed: {error}") }],
                "isError": true
            }));
        }

        let submitted_at = Self::now_unix_ms();
        let job_id = format!("JOB-{submitted_at}");
        let request_json = json!({
            "tool_name": normalized_name,
            "arguments": arguments,
        });

        if let Err(error) = Self::persist_mcp_job(
            self.graph_store.as_ref(),
            &job_id,
            normalized_name,
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
        let accepted_tool_name = normalized_name.clone();
        let queued_args = self.inject_reserved_ids(&normalized_name, arguments, &reserved_ids);
        let job_id_for_thread = job_id.clone();
        thread::spawn(move || {
            let server = McpServer::new(graph_store.clone());
            let started_at = McpServer::now_unix_ms();
            let _ = graph_store.execute_param(
                "UPDATE soll.McpJob SET status = ?, started_at = ? WHERE job_id = ?",
                &json!(["running", started_at, job_id_for_thread]),
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
                        "UPDATE soll.McpJob SET status = ?, finished_at = ?, result_json = ?, error_text = ? WHERE job_id = ?",
                        &json!([status, finished_at, result.to_string(), error_text, job_id_for_thread]),
                    );
                }
                None => {
                    let finished_at = McpServer::now_unix_ms();
                    let _ = graph_store.execute_param(
                        "UPDATE soll.McpJob SET status = ?, finished_at = ?, error_text = ? WHERE job_id = ?",
                        &json!(["failed", finished_at, format!("Invalid arguments for tool: {normalized_name}"), job_id_for_thread]),
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
                "reserved_ids": reserved_ids
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
                "submitted_at": row.get(3).cloned().unwrap_or(json!(null)),
                "started_at": row.get(4).cloned().unwrap_or(json!(null)),
                "finished_at": row.get(5).cloned().unwrap_or(json!(null)),
                "reserved_ids": reserved_ids,
                "result": result,
                "error_text": error_text
            }
        }))
    }

    pub fn handle_request(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if request.id.is_none() {
            return None;
        }

        let result = match request.method.as_str() {
            "initialize" => Some(json!({
                "protocolVersion": "2024-11-05",
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
