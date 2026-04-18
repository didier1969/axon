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

        if requires_indexed_runtime(normalized_name)
            && !matches!(
                runtime_profile,
                AxonRuntimeOperationalProfile::FullAutonomous
            )
        {
            let response = json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "Tool '{}' is unavailable in runtime mode '{}' with profile '{}'. Start Axon in `full_autonomous` mode for indexed graph diagnostics.",
                        normalized_name,
                        runtime_mode.as_str(),
                        runtime_profile.as_str()
                    )
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

        let response =
            if Self::mcp_mutation_jobs_enabled() && Self::is_mutating_tool(normalized_name) {
                self.launch_mutation_job(normalized_name, arguments)
            } else {
                self.execute_tool_direct(normalized_name, arguments)
            };

        Some(response.unwrap_or_else(|| {
            json!({
                "content": [{
                    "type": "text",
                    "text": format!("Invalid arguments for tool: {}", normalized_name)
                }],
                "isError": true
            })
        }))
    }
}
