use serde_json::{json, Value};

use super::catalog::requires_indexed_runtime;
use super::McpServer;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;
use crate::runtime_topology::{current_runtime_process_role, AxonProcessRole};

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
        let split_brain_public_authority =
            matches!(current_runtime_process_role(), AxonProcessRole::Brain)
                && matches!(
                    std::env::var("AXON_SPLIT_SHADOW_ONLY")
                        .ok()
                        .as_deref()
                        .map(str::trim),
                    Some("1") | Some("true") | Some("yes") | Some("on")
                );

        let resume_vectorization_unavailable = normalized_name == "resume_vectorization"
            && matches!(runtime_mode, AxonRuntimeMode::BrainOnly);
        if (requires_indexed_runtime(normalized_name) || resume_vectorization_unavailable)
            && !split_brain_public_authority
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
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Invalid arguments for tool: {}", normalized_name)
                    }],
                    "isError": true
                })
            }),
        ))
    }
}
