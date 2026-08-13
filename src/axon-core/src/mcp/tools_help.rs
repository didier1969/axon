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
            "author_soll" => json!({
                "intent": "author_soll",
                "minimal_sequence": ["soll_query_context", "soll_apply_plan", "soll_commit_revision"],
                "stop_rule": "write a derived multi-node SOLL subtree (vision->pillars->...->requirements) in ONE atomic idempotent soll_apply_plan, not N sequential soll_manager round-trips",
                "avoid": ["N separate soll_manager calls for one subtree", "inventing canonical IDs (the server allocates them)"],
                "requires_explicit_input_if": ["the subtree shape is still ambiguous (run /bootstrap-soll or grill-me first)"],
                "fallbacks": [
                    {"if": "plan_validation_fails", "do": "soll_apply_plan with dry_run=true to preview, fix relations, then soll_commit_revision"},
                    {"if": "single_node_change", "do": "use soll_manager create/update for one node instead of a plan"}
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
                    "author a multi-node subtree atomically: soll_apply_plan (logical_key, dry_run) -> soll_commit_revision",
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
                "Axon \u{2014} Structural Intelligence MCP Server",
                vec![
                    "Axon gives you indexed code structure, intentional requirements (SOLL), and project memory that persist across sessions.",
                    "It replaces grep/read for: symbol lookup, blast radius, dependency flow, architectural rationale, and delivery governance.",
                    "1. status \u{2192} runtime truth and project context",
                    "2. query/inspect \u{2192} find symbols, files, modules",
                    "3. retrieve_context \u{2192} compact evidence packet for your question",
                    "4. impact/path \u{2192} blast radius and source-sink flow",
                    "5. soll_query_context \u{2192} why the code exists (intent layer)",
                    "6. axon_pre_flight_check \u{2192} axon_commit_work \u{2192} delivery",
                ],
                vec![
                    "Call status() first \u{2014} it returns your project_code and next best action.",
                    "Use help(tool=X) to see any tool's JSON input schema and examples.",
                    "Use mode=brief first; escalate to full only for missing diagnostics.",
                    "Skill: axon-engineering-protocol",
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

/// REQ-AXO-902289 — Levenshtein distance, iterative two-row form.
///
/// Small and local on purpose: the only caller compares one mistyped tool name
/// against ~100 catalog entries, all short ASCII. A crate dependency for that
/// would cost more than it saves.
fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right_chars.len()).collect();
    let mut current = vec![0usize; right_chars.len() + 1];
    for (i, lc) in left.chars().enumerate() {
        current[0] = i + 1;
        for (j, rc) in right_chars.iter().enumerate() {
            let substitution = previous[j] + usize::from(lc != *rc);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right_chars.len()]
}

/// REQ-AXO-902289 — the catalog names closest to what the caller asked for.
///
/// Substring containment ranks first: the common miss is a PARTIAL name
/// (`friction_report` for `mcp_friction_report`), where edit distance alone
/// would bury the right answer under same-length neighbours. Typos then fall
/// back to edit distance, bounded so an unrelated word suggests nothing rather
/// than something confidently wrong.
fn closest_tool_names(requested: &str, limit: usize) -> Vec<String> {
    let max_distance = 3.max(requested.len() / 3);
    let mut scored: Vec<(usize, usize, String)> = tools_catalog(true)
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                .filter_map(|name| {
                    let contains = name.contains(requested) || requested.contains(name);
                    let distance = edit_distance(requested, name);
                    if !contains && distance > max_distance {
                        return None;
                    }
                    // Containment sorts ahead of pure edit-distance matches;
                    // within a tier, closest first, then shortest name.
                    Some((usize::from(!contains), distance, name.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.len().cmp(&b.2.len())));
    scored.into_iter().take(limit).map(|(_, _, name)| name).collect()
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
        // REQ-AXO-902289 — "call tools/list" was the whole repair, which asks an
        // agent to re-read 100+ names to fix one word. Name the closest matches
        // instead: a wrong tool name is nearly always a near-miss (a partial name
        // like `friction_report`, or a typo), and the catalog knows the answer.
        let suggestions = closest_tool_names(normalized, 3);
        let suggestion_text = if suggestions.is_empty() {
            String::new()
        } else {
            format!(" Closest: {}.", suggestions.join(", "))
        };
        return json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Unknown MCP tool `{}`.{} Otherwise call `tools/list` or `help(topic=\"routing\")`.",
                    normalized, suggestion_text
                )
            }],
            "isError": true,
            "data": {
                "problem_class": "unknown_tool",
                "requested_tool": normalized,
                "next_action": {"tool": "help", "arguments": {"topic": "routing"}},
                "parameter_repair": {
                    "invalid_field": "tool",
                    "suggestions": suggestions,
                    "hint": "retry `help(tool=…)` with one of `suggestions`, or list the surface with `tools/list`"
                }
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
        // REQ-AXO-902153 — governed how-to-work memory (practice_*) examples so the
        // PRIMARY memory channel is discoverable from help(), not just the catalog.
        "practice_recall" => json!([
            {
                "purpose": "recall practices for the current situation (FIRST at init)",
                "arguments": {
                    "query": "starting a session, what's the working discipline here",
                    "top_k": 8
                }
            }
        ]),
        "practice_put" => json!([
            {
                "purpose": "store a durable cross-tenant lesson",
                "arguments": {
                    "scope": "*",
                    "context": "long op (>30s: build, bench, tests)",
                    "practice": "run it in background + do useful parallel work; never idle-wait",
                    "evidence": "feedback_never_block_on_long_ops",
                    "perishability": "durable"
                }
            },
            {
                "purpose": "project + agent-role private practice (REQ-AXO-902149); failure-mode framing dodges write-gate over-reject (REQ-AXO-902154)",
                "arguments": {
                    "scope": "AXO",
                    "role": "coder",
                    "context": "SQL on soll.Node",
                    "practice": "SELECT label breaks (42703) — the column is title; that's the failure to avoid"
                }
            }
        ]),
        // REQ-AXO-902153 — cross-project mailbox (REQ-AXO-902114) examples.
        "mcp_inbox_read" => json!([
            {
                "purpose": "drain this project's unread inbox (advances the read cursor)",
                "arguments": {
                    "mode": "unread",
                    "limit": 20
                }
            }
        ]),
        "mcp_outbox_send" => json!([
            {
                "purpose": "hand off to another project (dense, pointer-bearing body; idempotency_key dedups re-sends)",
                "arguments": {
                    "to_project": "OPV",
                    "subject": "handoff: see CPT-AXO-052",
                    "body_dense": "Resume from session_pointer CPT-AXO-052; 3 next-actions inside.",
                    "idempotency_key": "handoff-axo-2026-06-29-s93"
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

#[cfg(test)]
mod unknown_tool_tests {
    use super::*;

    // REQ-AXO-902289 — `help unknown_tool` (23 occ) sent the caller back to
    // `tools/list` to fix a single word. The catalog already knows the answer.
    #[test]
    fn unknown_tool_names_the_closest_catalog_entries() {
        // Partial name — the common miss. Edit distance alone would rank
        // same-length neighbours above the tool actually asked for.
        let partial = tool_help_response("friction_report");
        assert_eq!(partial["data"]["problem_class"].as_str(), Some("unknown_tool"));
        let suggestions: Vec<&str> = partial["data"]["parameter_repair"]["suggestions"]
            .as_array()
            .expect("suggestions array")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            suggestions.first(),
            Some(&"mcp_friction_report"),
            "a partial name must surface its containing tool first, got {suggestions:?}"
        );
        assert!(
            partial["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("mcp_friction_report")),
            "the suggestion belongs in the text channel too — curl clients read only that"
        );

        // Typo — one transposed character.
        let typo = tool_help_response("inpsect");
        let typo_suggestions: Vec<&str> = typo["data"]["parameter_repair"]["suggestions"]
            .as_array()
            .expect("suggestions array")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            typo_suggestions.contains(&"inspect"),
            "a one-character typo must suggest the real tool, got {typo_suggestions:?}"
        );

        // A word unrelated to any tool suggests nothing rather than something
        // confidently wrong.
        let unrelated = tool_help_response("zzzzzzzzzzzzzzzzzzzz");
        assert!(
            unrelated["data"]["parameter_repair"]["suggestions"]
                .as_array()
                .is_some_and(|s| s.is_empty()),
            "no near match — say nothing rather than guess"
        );
    }

    #[test]
    fn known_tool_still_answers_with_its_contract() {
        let known = tool_help_response("query");
        assert_ne!(known["data"]["problem_class"].as_str(), Some("unknown_tool"));
    }
}
