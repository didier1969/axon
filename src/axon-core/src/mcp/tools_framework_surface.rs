use serde_json::{json, Value};

use super::catalog::tools_catalog;
use super::McpServer;
use crate::runtime_command_proxy::RuntimeCommandProxy;

pub(super) fn axon_mcp_surface_diagnostics_impl() -> Value {
    let public_tools = tools_catalog(false)
        .get("tools")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let public_tool_names = public_tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(|value| value.as_str()))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let async_allowlisted_tools = McpServer::ASYNC_JOB_TOOL_NAMES
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let monitored_sync_mutation_tools = McpServer::MONITORED_SYNC_MUTATION_TOOLS
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let public_host = std::env::var("AXON_PUBLIC_HOST").unwrap_or_default();
    let public_host_source =
        std::env::var("AXON_PUBLIC_HOST_SOURCE").unwrap_or_else(|_| "unresolved".to_string());
    let advertised_mcp_url = std::env::var("AXON_MCP_PUBLIC_URL").unwrap_or_default();
    let advertised_sql_url = std::env::var("AXON_SQL_PUBLIC_URL").unwrap_or_default();
    let advertised_dashboard_url = std::env::var("AXON_DASHBOARD_PUBLIC_URL").unwrap_or_default();
    let advertised_available = std::env::var("AXON_PUBLIC_ENDPOINTS_AVAILABLE").unwrap_or_default()
        == "1"
        && !advertised_mcp_url.is_empty();

    json!({
        "content": [{
            "type": "text",
            "text": "Surface MCP diagnostics assembled. Server truth is authoritative for catalog and dispatch. Client session freshness is explicit below; refresh the client session when a freshly advertised tool is missing locally."
        }],
        "data": {
            "server_truth": {
                "public_tool_count": public_tool_names.len(),
                "critical_tools": [
                    "status",
                    "job_status",
                    "project_registry_lookup",
                    "axon_init_project",
                    "soll_apply_plan",
                    "soll_commit_revision"
                ],
                "public_tools": public_tool_names
            },
            "async_policy": {
                "mode": "allowlist",
                "sync_by_default": true,
                "latency_target_p95_ms": 200,
                "allowlisted_tools": async_allowlisted_tools,
                "monitored_sync_mutation_tools": monitored_sync_mutation_tools,
                "semantic_async_triggers": [
                    "batch",
                    "restore_import",
                    "queue_pipeline",
                    "vectorization_indexation",
                    "deep_analytics"
                ]
            },
            "async_contract": {
                "canonical_follow_up_tool": "job_status",
                "acceptance_fields": ["job_id", "known_ids", "next_action", "result_contract", "polling_guidance", "recovery_hint"],
                "runtime_command_proxy": {
                    "enabled": RuntimeCommandProxy::enabled(),
                    "mode": RuntimeCommandProxy::proxy_mode(),
                    "timeout_ms": RuntimeCommandProxy::timeout_ms(),
                    "timeout_kind": RuntimeCommandProxy::timeout_kind(),
                    "ownership": {
                        "proxy_role": "brain",
                        "execution_role": "indexer",
                        "mutation_owner": "indexer",
                        "duplicate_execution_prevented": true
                    },
                    "retry_policy": {
                        "retryable": true,
                        "max_attempts": 1,
                        "idempotent": true,
                        "duplicate_execution_prevented": true
                    }
                },
                "preferred_identity_tools": ["project_registry_lookup", "axon_init_project"]
            },
            "advertised_endpoints": {
                "available": advertised_available,
                "public_host": public_host,
                "public_host_source": public_host_source,
                "mcp_url": advertised_mcp_url,
                "sql_url": advertised_sql_url,
                "dashboard_url": advertised_dashboard_url
            },
            "client_binding_notes": {
                "stale_client_binding_possible": true,
                "session_freshness_status": "unknown_outside_server",
                "operator_action": "If a freshly advertised public tool is not callable in the current client session, refresh or restart the client session and compare again.",
                "canonical_refresh_instruction": "Refresh or reconnect the MCP client session, then compare its visible tool surface with `mcp_surface_diagnostics.server_truth.public_tools`.",
                "safe_to_rely_on_now": [
                    "server truth for catalog and dispatch",
                    "advertised_endpoints for isolated clients",
                    "existing tools already visible in the active session"
                ],
                "may_require_client_refresh": [
                    "newly added public tools",
                    "freshly promoted endpoint changes",
                    "session-local tool bindings cached by the client"
                ],
                "guarantee_boundary": "The server guarantees catalog truth, dispatch truth, and advertised endpoint truth. Client session bindings are outside direct server control.",
                "external_endpoint_rule": "Do not use instance_identity.*_url as an external endpoint. Isolated clients must prefer advertised_endpoints.* when available."
            }
        }
    })
}
