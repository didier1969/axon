use serde_json::{json, Value};

use super::catalog::requires_indexed_runtime;
use super::McpServer;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;

impl McpServer {
    pub(crate) fn handle_call_tool(&self, params: Option<Value>) -> Option<Value> {
        let params = params?;
        let name = params.get("name")?.as_str()?;
        let normalized_name = name
            .strip_prefix("mcp_axon_")
            .or_else(|| name.strip_prefix("axon_"))
            .unwrap_or(name);
        let arguments = params.get("arguments")?;

        let runtime_mode = AxonRuntimeMode::from_env();
        let runtime_profile = AxonRuntimeOperationalProfile::from_mode_and_strings(
            runtime_mode.as_str(),
            std::env::var("AXON_ENABLE_AUTONOMOUS_INGESTOR")
                .ok()
                .as_deref(),
        );
        let resume_vectorization_unavailable = normalized_name == "resume_vectorization"
            && matches!(runtime_mode, AxonRuntimeMode::BrainOnly);
        if (requires_indexed_runtime(normalized_name) || resume_vectorization_unavailable)
            && !matches!(
                runtime_profile,
                AxonRuntimeOperationalProfile::IndexerFullAutonomous
            )
        {
            let response_text = if normalized_name == "resume_vectorization" {
                format!(
                    "Indexing operation '{}' is unavailable from the public brain authority while runtime mode is '{}' with profile '{}'. Run it on the active indexer authority, or start Axon in `indexer_full` mode with autonomous ingestion.",
                    normalized_name,
                    runtime_mode.as_str(),
                    runtime_profile.as_str()
                )
            } else {
                format!(
                    "Indexed operation '{}' is unavailable in runtime mode '{}' with profile '{}'. Start Axon in `indexer_full` mode with autonomous ingestion, or route the request through the split brain authority.",
                    normalized_name,
                    runtime_mode.as_str(),
                    runtime_profile.as_str()
                )
            };
            let response = json!({
                "content": [{
                    "type": "text",
                    "text": response_text
                }],
                "isError": true
            });
            let guidance =
                crate::mcp::classify_guidance(&[crate::mcp::GuidanceFact::problem_signal(
                    "tool_unavailable",
                )]);
            return Some(if Self::mcp_guidance_authoritative_enabled() {
                crate::mcp::attach_guidance_authoritative(response, guidance)
            } else if Self::mcp_guidance_shadow_enabled() {
                crate::mcp::attach_guidance_shadow(
                    response,
                    crate::mcp::guidance_outcome_to_value(&guidance),
                )
            } else {
                response
            });
        }

        let response = if Self::mcp_mutation_jobs_enabled()
            && Self::is_async_job_tool(normalized_name)
        {
            self.launch_mutation_job(name, normalized_name, arguments)
        } else {
            self.execute_tool_direct(normalized_name, arguments)
                .map(|result| {
                    self.attach_derived_docs_refresh_metadata(normalized_name, arguments, result)
                })
        };

        Some(self.attach_default_tool_guidance(
            normalized_name,
            arguments,
            response.unwrap_or_else(|| {
                // Build repair instruction with tool schema so the LLM can self-correct.
                let schema = super::catalog::tools_catalog(true)
                    .get("tools")
                    .and_then(Value::as_array)
                    .and_then(|tools| {
                        tools
                            .iter()
                            .find(|t| t.get("name").and_then(Value::as_str) == Some(normalized_name))
                    })
                    .and_then(|t| t.get("inputSchema").cloned());
                let schema_str = schema
                    .as_ref()
                    .map(|s| serde_json::to_string(s).unwrap_or_default())
                    .unwrap_or_default();
                let args_str = serde_json::to_string(arguments).unwrap_or_default();

                // REQ-AXO-139 slice — derive missing-required-fields and the
                // first invalid_field from the schema so the LLM can fix one
                // field per round-trip without diffing schema vs args itself.
                let supplied_keys: std::collections::HashSet<String> = arguments
                    .as_object()
                    .map(|map| map.keys().cloned().collect())
                    .unwrap_or_default();
                let required_fields: Vec<String> = schema
                    .as_ref()
                    .and_then(|s| s.get("required"))
                    .and_then(|r| r.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let missing_required: Vec<String> = required_fields
                    .iter()
                    .filter(|f| !supplied_keys.contains(*f))
                    .cloned()
                    .collect();
                let first_invalid_field = missing_required
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "arguments".to_string());
                let parameter_repair = json!({
                    "invalid_field": first_invalid_field,
                    "tool": normalized_name,
                    "missing_required_fields": missing_required,
                    "required_fields": required_fields,
                    "supplied_arguments": arguments,
                    "input_schema": schema,
                    "follow_up_tools": ["help"],
                    "hint": format!(
                        "compare `supplied_arguments` against `input_schema.required`; \
                         call `help(tool=\"{}\")` for the contract and retry once the \
                         missing/invalid fields are filled",
                        normalized_name
                    ),
                });
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "Invalid arguments for tool `{}`.\n\nYou sent:\n```json\n{}\n```\n\nExpected schema:\n```json\n{}\n```\n\nFix: check required fields and types, then retry.",
                            normalized_name, args_str, schema_str
                        )
                    }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "problem_class": "invalid_arguments",
                        "tool": normalized_name,
                        "received_arguments": arguments,
                        "input_schema": schema,
                        "repair_instruction": "Compare your arguments against input_schema. Fix required fields and types, then retry the same tool.",
                        "next_action": {
                            "tool": "help",
                            "arguments": { "tool": normalized_name }
                        },
                        "parameter_repair": parameter_repair
                    }
                })
            }),
        ))
    }
}
