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

        // REQ-AXO-901961 S1 — time the whole tool execution for per-call
        // telemetry (recorded best-effort below, never affects the response).
        let started = std::time::Instant::now();
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

        let final_response = self.attach_default_tool_guidance(
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
                let top_level_missing: Vec<String> = required_fields
                    .iter()
                    .filter(|f| !supplied_keys.contains(*f))
                    .cloned()
                    .collect();
                // REQ-AXO-901949 inv.3 — top-level `required` is satisfied by
                // `{action, entity, data}` even when the per-action field
                // (data.id for update, data.source_id for link, …) is absent.
                // Read the schema's own allOf if/then conditionals so the real
                // missing field is surfaced + stubbed, not lost.
                // REQ-AXO-901990 — per-action clauses now come from the dedicated
                // `conditional_clauses_for` source (the advertised schema is flat
                // so every client can bind soll_manager). `schema` still supplies
                // type labels for the repair stubs.
                let conditional_clauses =
                    super::tool_contracts::conditional_clauses_for(normalized_name);
                let conditional_missing: Vec<(String, String)> = schema
                    .as_ref()
                    .map(|s| {
                        super::tool_contracts::conditional_missing_fields(
                            s,
                            &conditional_clauses,
                            arguments,
                        )
                    })
                    .unwrap_or_default();
                // Combined view for reporting (`data.id` paths included).
                let missing_required: Vec<String> = top_level_missing
                    .iter()
                    .cloned()
                    .chain(conditional_missing.iter().map(|(p, _)| p.clone()))
                    .collect();
                let first_invalid_field = missing_required
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "arguments".to_string());

                // REQ-AXO-901949 — repair-as-data: hand the LLM the corrected
                // call ready to emit, not a prose "compare and fix". Start from
                // the supplied arguments and stub each missing required field
                // with a typed placeholder pulled from the (now schemars-derived)
                // input schema, so a single field-fill round-trip succeeds.
                let mut corrected_arguments = arguments.as_object().cloned().unwrap_or_default();
                for field in &top_level_missing {
                    let prop = schema
                        .as_ref()
                        .and_then(|s| s.get("properties"))
                        .and_then(|p| p.get(field));
                    // REQ-AXO-901947 — a closed-enum field stubs with its allowed
                    // values (`<FILL:one-of:a|b|c>`), not a bare `<FILL:type>`, so
                    // the LLM fills the right vocabulary in one round-trip.
                    let stub = match prop.and_then(super::tool_contracts::closed_enum_values) {
                        Some(values) => {
                            let opts: Vec<String> = values
                                .iter()
                                .map(|v| {
                                    v.as_str()
                                        .map(str::to_string)
                                        .unwrap_or_else(|| v.to_string())
                                })
                                .collect();
                            format!("<FILL:one-of:{}>", opts.join("|"))
                        }
                        None => {
                            let expected_type = prop
                                .and_then(|f| f.get("type"))
                                .and_then(Value::as_str)
                                .unwrap_or("value");
                            format!("<FILL:{expected_type}>")
                        }
                    };
                    corrected_arguments
                        .entry(field.clone())
                        .or_insert_with(|| Value::String(stub));
                }
                // Nested stubs for per-action conditional fields (`data.<field>`).
                for (path, expected_type) in &conditional_missing {
                    let Some(field) = path.strip_prefix("data.") else {
                        continue;
                    };
                    let data_entry = corrected_arguments
                        .entry("data".to_string())
                        .or_insert_with(|| json!({}));
                    if let Some(obj) = data_entry.as_object_mut() {
                        obj.entry(field.to_string())
                            .or_insert_with(|| Value::String(format!("<FILL:{expected_type}>")));
                    }
                }
                let corrected_call = json!({
                    "tool": normalized_name,
                    "arguments": corrected_arguments
                });
                // REQ-AXO-901947 (DEC-AXO-901638 slice 1) — the full reactive form:
                // every field with required/type and, for closed enums, the
                // allowed values spelled out. Folded into the text channel below
                // so HTTP/curl clients (which surface only content[0].text) get the
                // vocabulary too.
                let mut fields_form = super::tool_contracts::parameter_form_from_schema(
                    schema.as_ref(),
                    &required_fields,
                );
                // REQ-AXO-901947 slice 2 — fill dynamic valid_values (project codes
                // from the registry) the static schema can't carry.
                self.enrich_form_dynamic_values(&mut fields_form);
                let form_text = super::tool_contracts::render_parameter_form(&fields_form);
                let parameter_repair = json!({
                    "invalid_field": first_invalid_field,
                    "tool": normalized_name,
                    "missing_required_fields": missing_required,
                    "required_fields": required_fields,
                    "fields": fields_form,
                    "supplied_arguments": arguments,
                    "input_schema": schema,
                    "corrected_call": corrected_call,
                    "follow_up_tools": ["help"],
                    "hint": format!(
                        "emit `corrected_call`: it is your arguments with each \
                         missing required field stubbed `<FILL:type>`. Replace the \
                         stubs and re-call `{}` — no need to diff the schema yourself",
                        normalized_name
                    ),
                });
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "Invalid arguments for tool `{}`.\n\nYou sent:\n```json\n{}\n```\n\nExpected schema:\n```json\n{}\n```\n\nFix: check required fields and types, then retry.{}",
                            normalized_name, args_str, schema_str, form_text
                        )
                    }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "problem_class": "invalid_arguments",
                        "tool": normalized_name,
                        "received_arguments": arguments,
                        "input_schema": schema,
                        "repair_instruction": "Emit `parameter_repair.corrected_call` — it is your arguments with missing required fields stubbed. Fill the stubs and re-call. No schema diffing needed.",
                        "next_action": {
                            "tool": "help",
                            "arguments": { "tool": normalized_name }
                        },
                        "parameter_repair": parameter_repair
                    }
                })
            }),
        );
        // REQ-AXO-902143 (MBX réactivité niveau-2) — awareness piggyback: attach
        // the unread mailbox signal to the envelope at this single chokepoint so
        // an active session sees its mail on the next tool call. Targeted to the
        // session's project, no-op when unread==0, signal-only.
        let final_response =
            self.attach_mailbox_unread_banner(normalized_name, arguments, final_response);
        // REQ-AXO-901957 — closed-loop friction capture at the single dispatch
        // chokepoint every tool response passes through. Best-effort, records
        // only the problem SHAPE (never arg content), only when the response
        // carries a non-null problem_class.
        self.record_mcp_friction(normalized_name, &final_response);
        // REQ-AXO-901961 S1 — per-call telemetry (every call, ok or error) with
        // latency, at the same chokepoint. Best-effort, signature-only.
        self.record_mcp_call(
            normalized_name,
            &final_response,
            started.elapsed().as_millis() as i64,
        );
        Some(final_response)
    }
}
