use serde_json::{json, Value};

use super::catalog::tools_catalog;
use super::McpServer;

impl McpServer {
    pub(super) fn axon_help(&self, args: &Value) -> Option<Value> {
        let topic = args
            .get("topic")
            .and_then(Value::as_str)
            .unwrap_or("overview");
        let intent = args.get("intent").and_then(Value::as_str);
        if let Some(tool_name) = args.get("tool").and_then(Value::as_str) {
            return Some(tool_help_response(tool_name));
        }
        let skill_path = "docs/skills/axon-engineering-protocol/SKILL.md";
        let protocol = match intent.unwrap_or("") {
            "understand_symbol" => json!({
                "intent": "understand_symbol",
                "minimal_sequence": ["status", "project_status", "query", "inspect", "retrieve_context", "why"],
                "stop_rule": "stop after exact target, context packet, and governing rationale are available",
                "avoid": ["status full unless brief is degraded", "fs_read before inspect identifies the file"],
                "requires_explicit_input_if": ["target remains ambiguous after query", "project_code is unknown"],
                "fallbacks": [
                    {"if": "query_empty", "do": "broaden query terms or call project_status"},
                    {"if": "inspect_ambiguous", "do": "retry inspect with exact symbol or path"}
                ]
            }),
            "prepare_edit" => json!({
                "intent": "prepare_edit",
                "minimal_sequence": ["status", "project_status", "query", "inspect", "impact", "change_safety"],
                "stop_rule": "stop discovery after exact target, blast radius, and safety signal are known",
                "avoid": ["editing before impact", "status full unless brief is degraded"],
                "requires_explicit_input_if": ["business intent is missing", "change_safety reports irreversible or high-risk mutation"],
                "fallbacks": [
                    {"if": "impact_partial", "do": "call path or retrieve_context for missing edges"},
                    {"if": "safety_unknown", "do": "call change_safety with the concrete mutation summary"}
                ]
            }),
            "commit_work" => json!({
                "intent": "commit_work",
                "minimal_sequence": ["axon_pre_flight_check", "axon_commit_work"],
                "stop_rule": "commit only after preflight passes or returns a repairable rule with satisfied tests",
                "avoid": ["committing unrelated files", "inventing SOLL evidence"],
                "requires_explicit_input_if": ["preflight reports unrepaired strict guideline", "commit scope includes unknown user edits"],
                "fallbacks": [
                    {"if": "missing_tests", "do": "add or include modular test path, then rerun preflight"},
                    {"if": "bad_args", "do": "repair diff_paths/message arguments and retry"}
                ]
            }),
            "stabilize_soll" => json!({
                "intent": "stabilize_soll",
                "minimal_sequence": ["soll_query_context", "infer_soll_mutation", "entrench_nuance", "soll_validate"],
                "stop_rule": "write only when target IDs and intended nuance are explicit",
                "avoid": ["inventing canonical IDs", "mutating SOLL from ambiguous prose"],
                "requires_explicit_input_if": ["target_ids are unknown", "statement affects multiple requirements or decisions"],
                "fallbacks": [
                    {"if": "ambiguous_targets", "do": "call soll_query_context with narrower project_code"},
                    {"if": "validation_fails", "do": "repair relation/schema issues before continuing"}
                ]
            }),
            "runtime_check" => json!({
                "intent": "runtime_check",
                "minimal_sequence": ["status", "mcp_surface_diagnostics", "health"],
                "stop_rule": "stop after runtime truth is canonical and public surface is coherent",
                "avoid": ["debug unless status brief is degraded", "shell status before MCP status"],
                "requires_explicit_input_if": ["client endpoint binding is stale", "truth_status is not canonical"],
                "fallbacks": [
                    {"if": "surface_mismatch", "do": "call mcp_surface_diagnostics"},
                    {"if": "health_degraded", "do": "call status with mode=full"}
                ]
            }),
            _ => json!({
                "intent": "overview",
                "minimal_sequence": ["status", "project_status", "help(intent=...)"],
                "stop_rule": "choose one intent-specific protocol before broad exploration",
                "avoid": ["full modes by default", "parallel tool fan-out before target ambiguity is known"],
                "requires_explicit_input_if": ["project_code unknown", "business intent missing for mutation"],
                "fallbacks": [
                    {"if": "routing_unclear", "do": "call help with intent"},
                    {"if": "runtime_unclear", "do": "call status"}
                ]
            }),
        };
        let (summary, sequence, notes) = match topic {
            "routing" => (
                "Tool routing",
                vec![
                    "runtime truth: status",
                    "project truth: project_status",
                    "find target: query -> inspect",
                    "context packet: retrieve_context",
                    "blast radius/flow: impact -> path",
                    "rationale: why",
                    "risks: anomalies -> change_safety",
                ],
                vec![
                    "Prefer the first exact answer; do not fan out unless ambiguous.",
                    "Use mode=brief first; ask for full only when needed.",
                ],
            ),
            "soll" => (
                "SOLL governance",
                vec![
                    "read intent: soll_query_context",
                    "plan work: soll_work_plan",
                    "check schema: soll_relation_schema",
                    "infer mutation: infer_soll_mutation",
                    "apply exact change: soll_manager or entrench_nuance",
                    "validate: soll_validate",
                ],
                vec![
                    "Never invent canonical IDs or project_code.",
                    "Mutate SOLL only after intent is explicit.",
                ],
            ),
            "delivery" => (
                "Delivery",
                vec![
                    "preflight: axon_pre_flight_check",
                    "commit: axon_commit_work",
                    "async follow-up: job_status",
                    "release truth: status",
                ],
                vec![
                    "Tests may live in modular test files such as */tests/*.rs.",
                    "Keep commits SOLL-aware and scoped to changed paths.",
                ],
            ),
            "runtime" => (
                "Runtime",
                vec![
                    "truth: status",
                    "surface mismatch: mcp_surface_diagnostics",
                    "health: health",
                    "indexing diagnostics: diagnose_indexing",
                    "deep debug: debug",
                ],
                vec![
                    "Public MCP authority is brain.",
                    "IST writer authority is indexer.",
                    "Use status(mode=full) only for deep diagnostics.",
                ],
            ),
            _ => (
                "Axon MCP help",
                vec![
                    "start: status -> project_status",
                    "find code: query -> inspect -> retrieve_context",
                    "before edits: impact -> change_safety",
                    "intent: soll_query_context -> why",
                    "delivery: axon_pre_flight_check -> axon_commit_work",
                ],
                vec![
                    "Skill: axon-engineering-protocol",
                    "Skill path: docs/skills/axon-engineering-protocol/SKILL.md",
                    "Use brief modes first; escalate to full only for missing diagnostics.",
                ],
            ),
        };
        let text = format!(
            "## Axon Help\n\n**{}**\n\n{}\n\nNotes:\n{}\n\nProtocol: {}\n",
            summary,
            sequence
                .iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n"),
            notes
                .iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n"),
            protocol
                .get("intent")
                .and_then(Value::as_str)
                .unwrap_or("overview")
        );

        Some(json!({
            "content": [{ "type": "text", "text": text }],
            "data": {
                "topic": topic,
                "audience": "llm_clients_only",
                "skill": {
                    "name": "axon-engineering-protocol",
                    "path": skill_path,
                    "use_when": "working in Axon repo, choosing MCP tools, runtime entrypoints, SOLL mutations, qualification, or release actions"
                },
                "routing": sequence,
                "protocol": protocol,
                "notes": notes,
                "token_policy": "brief_first_full_only_when_needed",
                "next_action": {
                    "kind": "establish_runtime_truth",
                    "tool": "status",
                    "when": "now"
                }
            }
        }))
    }
}

fn tool_help_response(tool_name: &str) -> Value {
    let normalized = tool_name
        .strip_prefix("mcp_axon_")
        .or_else(|| tool_name.strip_prefix("axon_"))
        .unwrap_or(tool_name);
    let tool = tools_catalog(true)
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool.get("name").and_then(Value::as_str) == Some(normalized))
        })
        .cloned();

    let Some(tool) = tool else {
        return json!({
            "content": [{
                "type": "text",
                "text": format!("Unknown MCP tool `{}`. Call `tools/list` or `help(topic=\"routing\")`.", normalized)
            }],
            "isError": true,
            "data": {
                "problem_class": "unknown_tool",
                "requested_tool": normalized,
                "next_action": {"tool": "help", "arguments": {"topic": "routing"}},
                "parameter_repair": "Use an exact tool name from `tools/list`."
            }
        });
    };

    let examples = usage_examples_for_tool(normalized);
    let next_action = next_action_for_tool(normalized);
    let input_schema = tool
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    let schema_compact = serde_json::to_string(&input_schema).unwrap_or_default();
    let first_example = usage_examples_for_tool(normalized)
        .as_array()
        .and_then(|arr| arr.first().cloned())
        .and_then(|ex| {
            ex.get("arguments")
                .map(|args| serde_json::to_string_pretty(args).unwrap_or_default())
        })
        .unwrap_or_default();
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let text = format!(
        "## Axon Tool Help\n\nTool: `{}`\n\n{}\n\n### Input Schema\n```json\n{}\n```\n{}### Usage\nStart with the first example. If async, poll `job_status` until terminal.",
        normalized,
        description,
        schema_compact,
        if first_example.is_empty() {
            String::new()
        } else {
            format!("\n### Example\n```json\n{}\n```\n\n", first_example)
        },
    );

    json!({
        "content": [{ "type": "text", "text": text }],
        "data": {
            "tool": normalized,
            "description": tool.get("description").cloned().unwrap_or(Value::Null),
            "input_schema": tool.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"})),
            "usage_examples": examples,
            "next_action": next_action,
            "llm_usage_instruction": "Use `input_schema.required` before calling. Bad args: repair locally once, retry same tool, then follow `operator_guidance` from the response.",
            "skill": {
                "name": "axon-engineering-protocol",
                "path": "docs/skills/axon-engineering-protocol/SKILL.md"
            }
        }
    })
}

fn usage_examples_for_tool(tool_name: &str) -> Value {
    match tool_name {
        "soll_apply_plan" => json!([
            {
                "purpose": "safe preview",
                "arguments": {
                    "project_code": "AXO",
                    "author": "llm-client",
                    "dry_run": true,
                    "plan": {
                        "milestones": [{
                            "logical_key": "active-plan-example",
                            "title": "Active plan example",
                            "status": "active",
                            "description": "Short operational objective.",
                            "metadata": {"logical_key": "active-plan-example"}
                        }]
                    },
                    "relations": [{
                        "source_id": "active-plan-example",
                        "target_id": "REQ-AXO-001",
                        "relation_type": "TARGETS"
                    }]
                }
            },
            {
                "purpose": "commit after dry-run is correct",
                "arguments": {
                    "project_code": "AXO",
                    "author": "llm-client",
                    "dry_run": false,
                    "plan": {"requirements": []}
                },
                "follow_up": "poll `job_status(job_id)` until `state=completed` or `state=failed`"
            }
        ]),
        "soll_work_plan" => json!([
            {
                "purpose": "compact LLM work ordering",
                "arguments": {
                    "project_code": "AXO",
                    "limit": 8,
                    "top": 5,
                    "format": "brief"
                }
            },
            {
                "purpose": "full requirement validation details only when needed",
                "arguments": {
                    "project_code": "AXO",
                    "format": "json",
                    "include_validation_details": true
                }
            }
        ]),
        _ => json!([]),
    }
}

fn next_action_for_tool(tool_name: &str) -> Value {
    match tool_name {
        "soll_apply_plan" => json!({
            "tool": "soll_apply_plan",
            "arguments": {"project_code": "AXO", "dry_run": true, "plan": {}},
            "after_success": "poll `job_status` if the response returns `job_id`; commit only after dry-run matches intent"
        }),
        "soll_work_plan" => json!({
            "tool": "soll_work_plan",
            "arguments": {"project_code": "AXO", "limit": 8, "top": 5, "format": "brief"}
        }),
        _ => json!({
            "tool": tool_name,
            "arguments": {}
        }),
    }
}
