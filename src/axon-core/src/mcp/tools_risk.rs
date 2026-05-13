// Copyright (c) Didier Stadelmann. All rights reserved.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
use super::McpServer;

#[allow(dead_code)]
type ImpactCache = BTreeMap<String, (i64, Value)>;

#[allow(dead_code)]
static IMPACT_CACHE: OnceLock<Mutex<ImpactCache>> = OnceLock::new();

#[allow(dead_code)]
const IMPACT_CACHE_TTL_MS: i64 = 60_000;

impl McpServer {
    #[cfg(not(test))]
    fn impact_cache() -> &'static Mutex<ImpactCache> {
        IMPACT_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    #[cfg(not(test))]
    fn read_impact_cache(key: &str, now_ms: i64) -> Option<Value> {
        let guard = Self::impact_cache().lock().ok()?;
        let (stored_at, value) = guard.get(key)?;
        if now_ms.saturating_sub(*stored_at) > IMPACT_CACHE_TTL_MS {
            return None;
        }
        Some(value.clone())
    }

    #[cfg(test)]
    fn read_impact_cache(_key: &str, _now_ms: i64) -> Option<Value> {
        None
    }

    #[cfg(not(test))]
    fn write_impact_cache(key: String, now_ms: i64, value: &Value) {
        if let Ok(mut guard) = Self::impact_cache().lock() {
            guard.insert(key, (now_ms, value.clone()));
        }
    }

    #[cfg(test)]
    fn write_impact_cache(_key: String, _now_ms: i64, _value: &Value) {}

    fn resolve_scoped_symbol_id(&self, symbol: &str, project: Option<&str>) -> Option<String> {
        self.resolve_scoped_symbol_id_canonical(symbol, project)
    }

    fn suggest_scoped_symbols(&self, symbol: &str, project: Option<&str>, limit: usize) -> String {
        self.suggest_scoped_symbols_canonical(symbol, project, limit)
    }

    fn build_local_projection_section(
        &self,
        _symbol: &str,
        anchor: &str,
        depth: u64,
    ) -> Option<String> {
        let radius = depth.clamp(1, 2);
        let anchor_id = self
            .graph_store
            .refresh_symbol_projection(anchor, radius)
            .ok()??;
        let projection_res = self
            .graph_store
            .query_graph_projection("symbol", &anchor_id, radius)
            .ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&projection_res).unwrap_or_default();
        if rows.len() <= 1 {
            return None;
        }

        Some(format!(
            "\n\n### Derived Local Projection\n\n**Status:** derived neighborhood view, useful for local context; does not replace the canonical `CALLS` truth.\n\n{}",
            format_table_from_json(
                &projection_res,
                &["Target Type", "Target ID", "Link Type", "Distance", "Label", "URI"]
            )
        ))
    }

    pub(crate) fn axon_impact(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let cache_key = format!(
            "{}|{}|{}|{}",
            project.unwrap_or("*"),
            symbol,
            depth,
            mode.unwrap_or("brief")
        );
        if let Some(cached) = Self::read_impact_cache(&cache_key, now_ms) {
            return Some(cached);
        }
        let Some(target_id) = self.resolve_scoped_symbol_id(symbol, project) else {
            return self.axon_impact_without_calls(symbol, project, depth);
        };

        let query = if let Some(project_code) = project {
            let escaped_project = project_code.replace('\'', "''");
            format!(
                "WITH RECURSIVE bridge_edges AS (
                    SELECT s1.id AS source_id, s2.id AS target_id
                    FROM Symbol s1
                    JOIN Symbol s2 ON s1.name = s2.name AND s1.id <> s2.id
                    WHERE s1.project_code = '{project}'
                      AND s2.project_code = '{project}'
                      AND (COALESCE(s1.is_nif, FALSE) = TRUE OR COALESCE(s2.is_nif, FALSE) = TRUE)
                ),
                all_edges AS (
                    SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS WHERE project_code = '{project}'
                    UNION ALL
                    SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF WHERE project_code = '{project}'
                    UNION ALL
                    SELECT source_id, target_id, 'bridge_name' AS edge_type FROM bridge_edges
                ),
                traverse(caller, callee, depth, edge_type) AS (
                    SELECT source_id, target_id, 1 as depth, edge_type FROM all_edges WHERE target_id = $target_id
                    UNION ALL
                    SELECT c.source_id, c.target_id, t.depth + 1, c.edge_type
                    FROM all_edges c JOIN traverse t ON c.target_id = t.caller
                    WHERE t.depth < {depth}
                )
                SELECT t.caller, t.edge_type, COALESCE(f.path, 'Unknown') AS origin, s.name, s.kind
                FROM traverse t
                JOIN Symbol s ON t.caller = s.id
                LEFT JOIN CONTAINS con ON s.id = con.target_id AND con.project_code = '{project}'
                LEFT JOIN File f ON f.path = con.source_id",
                project = escaped_project,
                depth = depth
            )
        } else {
            format!(
            "WITH RECURSIVE bridge_edges AS (
                SELECT s1.id AS source_id, s2.id AS target_id
                FROM Symbol s1
                JOIN Symbol s2 ON s1.name = s2.name AND s1.id <> s2.id
                WHERE (COALESCE(s1.is_nif, FALSE) = TRUE OR COALESCE(s2.is_nif, FALSE) = TRUE)
            ),
            all_edges AS (
                SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                UNION ALL
                SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
                UNION ALL
                SELECT source_id, target_id, 'bridge_name' AS edge_type FROM bridge_edges
            ),
            traverse(caller, callee, depth, edge_type) AS (
                SELECT source_id, target_id, 1 as depth, edge_type FROM all_edges WHERE target_id = $target_id
                UNION ALL
                SELECT c.source_id, c.target_id, t.depth + 1, c.edge_type
                FROM all_edges c JOIN traverse t ON c.target_id = t.caller
                WHERE t.depth < {}
            )
            SELECT t.caller, t.edge_type, COALESCE(f.path, 'Unknown') AS origin, s.name, s.kind
            FROM traverse t
            JOIN Symbol s ON t.caller = s.id
            LEFT JOIN CONTAINS con ON s.id = con.target_id
            LEFT JOIN File f ON f.path = con.source_id",
            depth
        )
        };
        let params = json!({ "target_id": target_id });

        // MIL-AXO-015 B.3: under PG with AXON_AGE_READ=true, try the
        // AGE Cypher equivalent first. The AGE form returns the same
        // 5-column shape so the downstream parsing is unchanged.
        // Falls back to the SQL recursive form on empty / error.
        //
        // REQ-AXO-251: under PG age-only-relations, the SQL CALLS / CALLS_NIF
        // / CONTAINS tables are empty/dropped — bypass the SQL fallback so
        // an empty AGE result yields an empty caller set instead of querying
        // a missing relation table.
        let skip_sql_relations = self.graph_store.skip_sql_relations();
        // MIL-AXO-017 slice 5 (REQ-AXO-299) — prefer the unified
        // public.Edge SQL function. Falls through to AGE Cypher on PG
        // when the SQL path returns empty (transitional dual-write
        // window per REQ-AXO-297) so this slice is non-destructive.
        // AGE branch is removed in slice 6 (REQ-AXO-300).
        let public_edge_result = if self.graph_store.is_postgres_backend() {
            self.impact_callers_via_public_edge(&target_id, project, depth)
                .map(Ok)
        } else {
            None
        };
        // MIL-AXO-017 slice 6B: AGE retired ; fall through to legacy SQL only.
        let query_outcome = public_edge_result.unwrap_or_else(|| {
            if skip_sql_relations {
                Ok("[]".to_string())
            } else {
                self.graph_store.query_json_param(&query, &params)
            }
        });

        match query_outcome {
            Ok(res) => {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
                let mut impact_rows = BTreeMap::<String, (String, String, String)>::new();
                let mut impacted_symbol_ids = BTreeSet::<String>::new();
                let mut impacted_symbol_names = BTreeSet::<String>::new();
                let mut direct_edges = 0_i64;
                let mut nif_edges = 0_i64;
                let mut inferred_edges = 0_i64;

                for row in &rows {
                    let caller_id = row
                        .first()
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let edge_type = row.get(1).and_then(|v| v.as_str()).unwrap_or("unknown");
                    let origin = row
                        .get(2)
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    let name = row
                        .get(3)
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string();
                    let kind = row
                        .get(4)
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string();

                    if !caller_id.is_empty() {
                        impacted_symbol_ids.insert(caller_id.clone());
                        impact_rows
                            .entry(caller_id)
                            .or_insert_with(|| (origin, name.clone(), kind));
                    }
                    if !name.is_empty() {
                        impacted_symbol_names.insert(name);
                    }
                    match edge_type {
                        "calls" => direct_edges += 1,
                        "calls_nif" => nif_edges += 1,
                        "bridge_name" => inferred_edges += 1,
                        _ => {}
                    }
                }

                let impact_radius = impacted_symbol_ids.len() as i64;
                if rows.is_empty() && impact_radius == 0 {
                    return self.axon_impact_without_calls(symbol, project, depth);
                }

                let display_rows = impact_rows
                    .values()
                    .map(|(origin, name, kind)| json!([origin, name, kind]))
                    .collect::<Vec<_>>();
                let display_raw =
                    serde_json::to_string(&display_rows).unwrap_or_else(|_| "[]".to_string());
                let mut table = if display_rows.len() > 15 {
                    format!(
                        "_Report aggregated because {} symbols are impacted. Only major architectural impacts are detailed below._\n\n",
                        display_rows.len()
                    )
                } else {
                    format_table_from_json(
                        &display_raw,
                        &["File / Project", "Impacted Symbol", "Type"],
                    )
                };

                impacted_symbol_ids.insert(target_id.clone());
                impacted_symbol_names.insert(symbol.to_string());
                let ids_sql = impacted_symbol_ids
                    .iter()
                    .map(|id| format!("'{}'", id.replace('\'', "''")))
                    .collect::<Vec<_>>()
                    .join(", ");
                let names_sql = impacted_symbol_names
                    .iter()
                    .map(|name| format!("'{}'", name.replace('\'', "''")))
                    .collect::<Vec<_>>()
                    .join(", ");
                let soll_query = format!(
                    "WITH RECURSIVE soll_entry_points AS (
                        SELECT DISTINCT t.soll_entity_id as id
                        FROM soll.Traceability t
                        WHERE t.artifact_type = 'Symbol'
                          AND (t.artifact_ref IN ({ids_sql}) OR t.artifact_ref IN ({names_sql}))
                    ),
                    soll_traverse(id, depth) AS (
                        SELECT id, 1 as depth FROM soll_entry_points
                        UNION ALL
                        SELECT e.target_id, st.depth + 1
                        FROM soll.Edge e
                        JOIN soll_traverse st ON e.source_id = st.id
                        WHERE st.depth < 10
                    )
                    SELECT DISTINCT n.id, n.type, n.title
                    FROM soll_traverse st
                    JOIN soll.Node n ON st.id = n.id
                    ORDER BY n.type DESC, n.id"
                );
                let soll_raw = self
                    .graph_store
                    .query_json(&soll_query)
                    .unwrap_or_else(|_| "[]".to_string());
                let soll_rows: Vec<Vec<Value>> =
                    serde_json::from_str(&soll_raw).unwrap_or_default();

                if !soll_rows.is_empty() {
                    table.push_str("\n### 🏛️ SOLL Impact (Architecture Compromise)\n\n| Entity | Type | Title |\n| --- | --- | --- |\n");
                    for row in soll_rows {
                        let id = row.first().and_then(|v| v.as_str()).unwrap_or("-");
                        let t = row.get(1).and_then(|v| v.as_str()).unwrap_or("-");
                        let title = row.get(2).and_then(|v| v.as_str()).unwrap_or("-");
                        table.push_str(&format!("| `{}` | `{}` | {} |\n", id, t, title));
                    }
                    table.push('\n');
                }

                let confidence_label = if direct_edges + nif_edges > 0 {
                    "high"
                } else if inferred_edges > 0 {
                    "medium"
                } else {
                    "low"
                };

                let mut evidence = String::new();
                if let Some(note) = self.project_scope_truth_note(project) {
                    evidence.push_str(&note);
                    evidence.push('\n');
                }
                if let Some(note) =
                    self.degraded_truth_note(self.degraded_symbol_count(symbol, project))
                {
                    evidence.push_str(&note);
                    evidence.push('\n');
                }
                evidence.push_str(&format!(
                    "**Impact Radius (depth {}):** {} components affected across the Lattice.\n\n",
                    depth, impact_radius
                ));
                evidence.push_str(&format!(
                    "**Coverage:** confidence={} (direct_calls={}, calls_nif={}, inferred_bridge={})\n\n",
                    confidence_label, direct_edges, nif_edges, inferred_edges
                ));
                evidence.push_str(&table);
                if let Some(section) =
                    self.build_local_projection_section(symbol, &target_id, depth)
                {
                    evidence.push_str(&section);
                }
                let scope = project
                    .map(|p| format!("project:{}", p))
                    .unwrap_or_else(|| "workspace:*".to_string());
                let report = format!(
                    "## 💥 Cross-Cutting Impact Analysis: {}\n\n{}",
                    symbol,
                    format_standard_contract(
                        "ok",
                        "impact analysis computed",
                        &scope,
                        &evidence_by_mode(&evidence, mode),
                        &[
                            "review top impacted symbols",
                            "run simulate_mutation before editing"
                        ],
                        confidence_label,
                    )
                );

                let mut blocking_factors = Vec::<Value>::new();
                if direct_edges + nif_edges == 0 && inferred_edges > 0 {
                    blocking_factors.push(json!({
                        "factor": "impact_inferred_from_bridge_edges",
                        "severity": "medium",
                        "recommended_action": "treat the impact graph as partially inferred and confirm critical hops with inspect/path before mutation"
                    }));
                }
                let remediation_actions = blocking_factors
                    .iter()
                    .filter_map(|factor| {
                        factor
                            .get("recommended_action")
                            .and_then(|value| value.as_str())
                            .map(|value| Value::from(value.to_string()))
                    })
                    .collect::<Vec<_>>();
                let next_action = json!({
                    "kind": "simulate_mutation_before_editing",
                    "tool": "simulate_mutation",
                    "when": "now"
                });
                let response = json!({
                    "content": [{ "type": "text", "text": report }],
                    "data": {
                        "symbol": symbol,
                        "project": project,
                        "depth": depth,
                        "impact_radius": impact_radius,
                        "summary": {
                            "confidence": confidence_label,
                            "direct_edges": direct_edges,
                            "calls_nif_edges": nif_edges,
                            "inferred_bridge_edges": inferred_edges
                        },
                        "operator_guidance": {
                            "actionable_now": blocking_factors.is_empty(),
                            "blocking_factors": blocking_factors,
                            "remediation_actions": remediation_actions,
                            "follow_up_tools": ["simulate_mutation", "path", "why"],
                            "next_action": next_action
                        },
                        "next_action": next_action
                    }
                });
                Self::write_impact_cache(cache_key, now_ms, &response);
                Some(response)
            }
            Err(e) => Some(json!({
                "content": [{ "type": "text", "text": format!("Impact Analysis Error: {}", e) }],
                "isError": true,
                "data": {
                    "status": "internal_error",
                    "parameter_repair": {
                        "invalid_field": "symbol",
                        "follow_up_tools": ["inspect", "query", "status"],
                        "hint": "impact computation failed; verify the symbol resolves via `inspect` and the runtime is healthy via `status`"
                    },
                    "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                }
            })),
        }
    }

    fn axon_impact_without_calls(
        &self,
        symbol: &str,
        project: Option<&str>,
        depth: u64,
    ) -> Option<Value> {
        let (query, params) = if let Some(project) = project {
            (
                "SELECT name, kind, COALESCE(project_code, 'unknown') \
                 FROM Symbol \
                 WHERE (name = $sym OR id = $sym) AND project_code = $project \
                 LIMIT 5",
                json!({ "sym": symbol, "project": project }),
            )
        } else {
            (
                "SELECT name, kind, COALESCE(project_code, 'unknown') \
                 FROM Symbol \
                 WHERE name = $sym OR id = $sym \
                 LIMIT 5",
                json!({ "sym": symbol }),
            )
        };
        let symbol_res = self
            .graph_store
            .query_json_param(query, &params)
            .unwrap_or_else(|_| "[]".to_string());
        let symbol_rows: Vec<Vec<Value>> = serde_json::from_str(&symbol_res).unwrap_or_default();
        if symbol_rows.is_empty() {
            let suggestions = self.suggest_scoped_symbols(symbol, project, 8);
            let suggestions_table =
                format_table_from_json(&suggestions, &["Suggested Symbol", "Type", "Project"]);
            let suggestions_rows: Vec<Vec<Value>> =
                serde_json::from_str(&suggestions).unwrap_or_default();
            // REQ-AXO-043 — same dead-end as inspect/path: "retry with one
            // suggested symbol" is unactionable when there are no suggestions.
            let has_suggestions = !suggestions_rows.is_empty();
            let next_actions: &[&str] = if has_suggestions {
                &[
                    "retry with one suggested symbol",
                    "use query/inspect to validate exact name",
                ]
            } else {
                &[
                    "broaden the search via `query` with a less specific term",
                    "verify spelling and project scope",
                    "or pass the exact canonical symbol id",
                ]
            };
            let suggestions = suggestions_rows
                .iter()
                .filter_map(|row| row.first().and_then(Value::as_str))
                .map(|value| Value::from(value.to_string()))
                .collect::<Vec<_>>();
            let recommended_action = if has_suggestions {
                "retry with one suggested symbol or validate the target with query/inspect first"
            } else {
                "broaden the search via `query` with a less specific term, or verify spelling and project scope"
            };
            let next_action_kind = if has_suggestions {
                "select_valid_symbol_then_retry_impact"
            } else {
                "broaden_search"
            };
            let next_action_tool = if has_suggestions { "inspect" } else { "query" };
            let next_action_when = if has_suggestions {
                "after_symbol_selection"
            } else {
                "after_widening_or_correcting_the_search"
            };
            let follow_up_tools: Vec<&str> = if has_suggestions {
                vec!["inspect"]
            } else {
                vec!["query", "inspect"]
            };
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "## 💥 Cross-Cutting Impact Analysis: {}\n\n{}",
                        symbol,
                        format_standard_contract(
                            "warn_input_not_found",
                            "symbol not found in current scope",
                            &project.map(|p| format!("project:{}", p)).unwrap_or_else(|| "workspace:*".to_string()),
                            &format!("No exact matching symbol found in current scope.\n\n### Suggestions\n\n{}", suggestions_table),
                            next_actions,
                            "medium",
                        )
                    )
                }],
                "data": {
                    "symbol": symbol,
                    "project": project,
                    "impact_available": false,
                    "suggestions": suggestions,
                    "operator_guidance": {
                        "actionable_now": false,
                        "blocking_factors": [{
                            "factor": "symbol_not_found_in_scope",
                            "severity": "high",
                            "recommended_action": recommended_action
                        }],
                        "remediation_actions": next_actions.iter().map(|s| Value::from(*s)).collect::<Vec<_>>(),
                        "follow_up_tools": follow_up_tools,
                        "next_action": {
                            "kind": next_action_kind,
                            "tool": next_action_tool,
                            "when": next_action_when
                        }
                    },
                    "next_action": {
                        "kind": next_action_kind,
                        "tool": next_action_tool,
                        "when": next_action_when
                    }
                }
            }));
        }
        let degraded_note = self.degraded_truth_note(self.degraded_symbol_count(symbol, project));
        let project_note = self.project_scope_truth_note(project);

        // REQ-AXO-251: under PG age-only-relations, the SQL CALLS / CALLS_NIF
        // tables are empty/dropped — treat as 0 (the structure-empty branch
        // below produces the right LLM response: "call graph not yet
        // available, run path/inspect"). The AGE call graph is queried by
        // `axon_impact` proper higher up in this same tool; this fallthrough
        // is only reached when the symbol resolves but no impact rows came
        // back — same outcome under either backend.
        let calls_count = if self.graph_store.skip_sql_relations() {
            0
        } else {
            self.graph_store
                .query_count(
                    "SELECT (SELECT count(*) FROM CALLS) + (SELECT count(*) FROM CALLS_NIF)",
                )
                .unwrap_or(0)
        };
        if calls_count > 0 {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "## 💥 Cross-Cutting Impact Analysis: {}\n\n{}{}No impact computed at depth {}.",
                        symbol,
                        project_note.clone().unwrap_or_default(),
                        degraded_note.clone().unwrap_or_default(),
                        depth
                    )
                }],
                "data": {
                    "symbol": symbol,
                    "project": project,
                    "depth": depth,
                    "impact_available": false,
                    "operator_guidance": {
                        "actionable_now": false,
                        "blocking_factors": [{
                            "factor": "no_impact_resolved_at_requested_depth",
                            "severity": "medium",
                            "recommended_action": "increase depth or inspect local dependency flow before assuming there is no impact"
                        }],
                        "remediation_actions": [
                            "increase depth or inspect local dependency flow before assuming there is no impact"
                        ],
                        "follow_up_tools": ["path", "inspect"],
                        "next_action": {
                            "kind": "inspect_local_dependency_flow",
                            "tool": "path",
                            "when": "now"
                        }
                    },
                    "next_action": {
                        "kind": "inspect_local_dependency_flow",
                        "tool": "path",
                        "when": "now"
                    }
                }
            }));
        }

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "## 💥 Cross-Cutting Impact Analysis: {}\n\n{}{}Symbol exists, but the call graph is not yet available in this live database.\n\n{}\n\n**Status:** CALLS is empty; impact radius cannot yet be reliably computed.",
                    symbol,
                    project_note.unwrap_or_default(),
                    degraded_note.unwrap_or_default(),
                    format_table_from_json(&symbol_res, &["Name", "Type", "Project"])
                )
            }],
            "data": {
                "symbol": symbol,
                "project": project,
                "depth": depth,
                "impact_available": false,
                "operator_guidance": {
                    "actionable_now": false,
                    "blocking_factors": [{
                        "factor": "call_graph_not_available",
                        "severity": "high",
                        "recommended_action": "wait for live call-graph truth before relying on impact for risky mutation"
                    }],
                    "remediation_actions": [
                        "wait for live call-graph truth before relying on impact for risky mutation"
                    ],
                    "follow_up_tools": ["inspect", "query"],
                    "next_action": {
                        "kind": "wait_for_call_graph_truth",
                        "tool": "inspect",
                        "when": "after_indexing_progress"
                    }
                },
                "next_action": {
                    "kind": "wait_for_call_graph_truth",
                    "tool": "inspect",
                    "when": "after_indexing_progress"
                }
            }
        }))
    }

    pub(crate) fn axon_diff(&self, args: &Value) -> Option<Value> {
        let diff = args.get("diff_content")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(10, 500) as usize)
            .unwrap_or(120);
        let mut files = std::collections::HashSet::new();
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                files.insert(path.to_string());
            } else if let Some(path) = line.strip_prefix("--- a/") {
                if path != "/dev/null" {
                    files.insert(path.to_string());
                }
            }
        }

        let mut all_results = Vec::new();
        for file in files {
            let query = format!(
                "SELECT s.name, s.kind FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id WHERE f.path LIKE '%{}%' LIMIT {}",
                file.replace("'", "''"),
                limit
            );
            if let Ok(res) = self.graph_store.query_json(&query) {
                all_results.push(format!("File: {}\nSymbols:\n{}", file, res));
            }
        }
        let mut joined = all_results.join("\n\n");
        let truncated = if joined.len() > 60_000 {
            joined.truncate(60_000);
            true
        } else {
            false
        };
        let evidence = if truncated {
            format!("{}\n\n[truncated=true, max_chars=60000]", joined)
        } else {
            joined
        };
        let report = format!(
            "## 🧬 Diff Impact\n\n{}",
            format_standard_contract(
                "ok",
                "diff symbol extraction completed",
                "workspace:*",
                &evidence_by_mode(&evidence, mode),
                &[
                    "increase `limit` if needed",
                    "run impact on selected symbols for blast radius"
                ],
                if truncated { "medium" } else { "high" },
            )
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_simulate_mutation(&self, args: &Value) -> Option<Value> {
        let symbol = match args.get("symbol").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                return Some(json!({
                    "content": [{ "type": "text", "text": "Missing required argument: symbol" }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "parameter_repair": {
                            "invalid_field": "symbol",
                            "follow_up_tools": ["help", "query"],
                            "hint": "supply a non-empty `symbol`; use `query` to discover symbol names"
                        }
                    }
                }));
            }
        };
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2);
        let target_id = match self.resolve_scoped_symbol_id(symbol, project) {
            Some(id) => id,
            None => {
                // REQ-AXO-043 — same dead-end as inspect/path/impact: when
                // suggestion table is empty, "retry with one suggested
                // symbol" is unactionable. Tailor recovery to actual state.
                let suggestions = self.suggest_scoped_symbols(symbol, project, 8);
                let suggestions_table =
                    format_table_from_json(&suggestions, &["Suggested Symbol", "Type", "Project"]);
                let suggestion_rows: Vec<Vec<Value>> =
                    serde_json::from_str(&suggestions).unwrap_or_default();
                let has_suggestions = !suggestion_rows.is_empty();
                let next_actions: &[&str] = if has_suggestions {
                    &[
                        "retry with one suggested symbol",
                        "run inspect to validate symbol name",
                    ]
                } else {
                    &[
                        "broaden the search via `query` with a less specific term",
                        "verify spelling and project scope",
                        "or pass the exact canonical symbol id",
                    ]
                };
                let suggestion_strs: Vec<Value> = suggestion_rows
                    .iter()
                    .filter_map(|row| row.first().and_then(Value::as_str))
                    .map(|value| Value::from(value.to_string()))
                    .collect();
                let next_action_kind = if has_suggestions {
                    "select_valid_symbol_then_retry_simulate"
                } else {
                    "broaden_search"
                };
                let next_action_tool = if has_suggestions { "inspect" } else { "query" };
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "## 🔮 Dry-Run Mutation: {}\n\n{}",
                            symbol,
                            format_standard_contract(
                                "warn_input_not_found",
                                "symbol not found in current scope",
                                &project.map(|p| format!("project:{}", p)).unwrap_or_else(|| "workspace:*".to_string()),
                                &evidence_by_mode(
                                    &format!("No exact symbol found in current scope.\n\n### Suggestions\n\n{}", suggestions_table),
                                    mode,
                                ),
                                next_actions,
                                "medium",
                            )
                        )
                    }],
                    "data": {
                        "symbol": symbol,
                        "project": project,
                        "symbol_found": false,
                        "suggestions": suggestion_strs,
                        "next_action": {
                            "kind": next_action_kind,
                            "tool": next_action_tool,
                        }
                    }
                }));
            }
        };
        // REQ-AXO-251: under PG age-only-relations, the SQL CALLS table is
        // empty/dropped — `simulate_mutation` is a quick blast-radius probe
        // that degrades to "0 components" gracefully. Operators wanting a
        // real estimate use `axon_impact` (already AGE-aware above) /
        // `axon_path` for the full traversal.
        if self.graph_store.skip_sql_relations() {
            let report = format!(
                "## 🔮 Dry-Run Mutation: {}\n\n{}",
                symbol,
                format_standard_contract(
                    "ok",
                    "mutation blast-radius estimated",
                    &project
                        .map(|p| format!("project:{}", p))
                        .unwrap_or_else(|| "workspace:*".to_string()),
                    &evidence_by_mode(
                        &format!(
                            "Modifying '{}' will cascade-impact ~0 components in the SQL relation tables (PG age-only mode). Run `impact` for the AGE-backed traversal.",
                            symbol
                        ),
                        mode,
                    ),
                    &["run `impact` for the AGE-backed blast-radius estimate"],
                    "medium",
                )
            );
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        }
        let query = format!(
            "WITH RECURSIVE traverse(caller, callee, depth) AS ( \
                SELECT source_id, target_id, 1 as depth FROM CALLS WHERE target_id = $target_id \
                UNION ALL \
                SELECT c.source_id, c.target_id, t.depth + 1 \
                FROM CALLS c JOIN traverse t ON c.target_id = t.caller \
                WHERE t.depth < {} \
            ) \
            SELECT count(DISTINCT caller) FROM traverse",
            depth
        );
        let params = json!({"target_id": target_id});

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let count: i64 = res.trim().parse().unwrap_or(0);
                let report = format!(
                    "## 🔮 Dry-Run Mutation: {}\n\n{}",
                    symbol,
                    format_standard_contract(
                        "ok",
                        "mutation blast-radius estimated",
                        &project.map(|p| format!("project:{}", p)).unwrap_or_else(|| "workspace:*".to_string()),
                        &evidence_by_mode(
                            &format!("Modifying '{}' will cascade-impact ~{} components in the architecture.", symbol, count),
                            mode,
                        ),
                        &["review impact output for precise affected components"],
                        "high",
                    )
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(json!({
                "content": [{ "type": "text", "text": format!("Simulation Error: {}", e) }],
                "isError": true,
                "data": {
                    "status": "internal_error",
                    "parameter_repair": {
                        "invalid_field": "symbol",
                        "follow_up_tools": ["inspect", "impact", "status"],
                        "hint": "mutation simulation failed; verify symbol resolves via `inspect`, runtime is healthy via `status`, and depth is reasonable (≤6 typical)"
                    },
                    "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                }
            })),
        }
    }

    /// MIL-AXO-015 B.3: AGE Cypher implementation of axon_impact's
    /// caller traversal. Returns the same 5-column JSON shape as the
    /// SQL WITH RECURSIVE form: rows of `[caller_id, edge_type,
    /// origin_path, name, kind]`.
    ///
    /// Three sub-patterns combined via UNION:
    /// - CALLS variable-length traversal up to `depth`
    /// - CALLS_NIF variable-length traversal up to `depth`
    /// - bridge_name (same Symbol.name across project, NIF flag)
    ///
    /// Returns `None` on identifier validation failure, AGE query
    /// error, or empty result so the caller falls back to SQL —
    /// covers AGE empty (dual-write opt-in not enabled), schema gaps,
    /// or AGE quirks we haven't covered yet.
    /// MIL-AXO-017 slice 5 (REQ-AXO-299) — query `public.callers_of`
    /// SQL function (REQ-AXO-296) for reverse-traversal callers. Joins
    /// with `public.Symbol` + `public.Chunk` to surface the 5-column
    /// shape `axon_impact` expects (caller_id, edge_type, origin, name,
    /// kind), keeping the JSON contract identical to the AGE-based
    /// `impact_callers_via_age` helper. Returns `None` on empty / error
    /// so the caller falls through to AGE for diagnosis during the
    /// transition.
    fn impact_callers_via_public_edge(
        &self,
        target_id: &str,
        project: Option<&str>,
        depth: u64,
    ) -> Option<String> {
        let depth_clamped = depth.clamp(1, 10) as i32;
        let project_code_param = project.unwrap_or("");
        // Uses Axon's positional `?` placeholders (`expand_named_params`
        // inlines escaped string literals before dispatch). The graph
        // SQL function `public.callers_of` returns (source_id, distance,
        // relation_type) — we map relation_type to the lowercased
        // canonical labels (`calls`, `calls_nif`) that `axon_impact`
        // parses for direct vs nif edge accounting.
        let sql = format!(
            "WITH callers AS (\
                 SELECT source_id, distance, relation_type FROM callers_of(?, {depth_clamped}::INT, ?)\
             ),\
             enriched AS (\
                 SELECT c.source_id AS caller_id,\
                        CASE c.relation_type\
                            WHEN 'CALLS'     THEN 'calls'\
                            WHEN 'CALLS_NIF' THEN 'calls_nif'\
                            ELSE lower(c.relation_type)\
                        END AS edge_type,\
                        COALESCE(s.name, '-') AS name,\
                        COALESCE(s.kind, '-') AS kind,\
                        COALESCE(MIN(ch.file_path), '-') AS origin\
                 FROM callers c\
                 LEFT JOIN public.Symbol s ON s.id = c.source_id\
                 LEFT JOIN public.Chunk ch ON ch.source_id = c.source_id AND ch.source_type = 'symbol'\
                 GROUP BY c.source_id, c.relation_type, s.name, s.kind\
             )\
             SELECT caller_id, edge_type, origin, name, kind FROM enriched"
        );
        let params = serde_json::json!([target_id, project_code_param]);
        let raw = match self.graph_store.query_json_param(&sql, &params) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("impact_callers_via_public_edge: SQL query failed: {}", e);
                return None;
            }
        };
        if raw.trim() == "[]" {
            return None;
        }
        Some(raw)
    }

    // MIL-AXO-017 slice 6B: AGE helper impact_callers_via_age removed ; public.callers_of is canonical.
}
