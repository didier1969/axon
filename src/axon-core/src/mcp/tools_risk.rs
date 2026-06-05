// Copyright (c) Didier Stadelmann. All rights reserved.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
use super::McpServer;
use crate::ist_snapshot::{process_view, RelationType};

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
        project: Option<&str>,
    ) -> Option<String> {
        let radius = depth.clamp(1, 2);
        let columns = ["Target Type", "Target ID", "Link Type", "Distance", "Label", "URI"];

        // REQ-AXO-901884 / feedback_trimodal_use_ram_graph_not_pg — RAM-first
        // (PIL-AXO-9002): when the per-project CSR is warm, derive the local
        // neighborhood (forward ∪ reverse reach) from the in-memory graph. The
        // PG `query_graph_projection` (ist.impact + ist.callers_of SQL) is the
        // degraded cold/unscoped fallback ONLY. RAM rows carry target_id as
        // label + empty uri — name/file enrichment is a PG-only join — matching
        // the structural_neighbors RAM contract (tools_context.rs, edge_kind
        // "ram_csr").
        if let Some(p) = project {
            let view = process_view();
            if view.is_warm(p) {
                let cap = 200usize;
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut ids: Vec<String> = Vec::new();
                for reach in [
                    view.forward_at_radius(p, anchor, radius as u32, cap, &[]),
                    view.reverse_at_radius(p, anchor, radius as u32, cap, &[]),
                ]
                .into_iter()
                .flatten()
                {
                    for id in reach {
                        if id != anchor && seen.insert(id.clone()) {
                            ids.push(id);
                        }
                    }
                }
                if ids.len() <= 1 {
                    return None;
                }
                let json_rows: Vec<Vec<Value>> = ids
                    .into_iter()
                    .map(|id| {
                        vec![
                            Value::String("symbol".to_string()),
                            Value::String(id.clone()),
                            Value::String("ram_csr".to_string()),
                            Value::Number(0.into()),
                            Value::String(id),
                            Value::String(String::new()),
                        ]
                    })
                    .collect();
                let projection_res = serde_json::to_string(&json_rows).ok()?;
                return Some(format!(
                    "\n\n### Derived Local Projection\n\n**Status:** derived neighborhood view (RAM CSR), useful for local context; does not replace the canonical `CALLS` truth.\n\n{}",
                    format_table_from_json(&projection_res, &columns)
                ));
            }
        }

        // Cold cache OR unscoped (project=None) → canonical PG fallback.
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
            format_table_from_json(&projection_res, &columns)
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

        // REQ-AXO-91512 — RAM-first via IstGraphView (PIL-AXO-9002,
        // feedback_trimodal_use_ram_graph_not_pg). When the cache is
        // warm, the reverse-traversal runs entirely in RAM ; the PG
        // path (`impact_callers_via_ist_edge` → REQ-AXO-296 SQL fn)
        // is the degraded fallback for cold cache or unscoped queries.
        // Inferred `bridge_name` edges are a PG-text-matching artifact
        // not represented in the IST snapshot ; when RAM serves the
        // query, `inferred_bridge_edges` is reported as 0 and a
        // `surfaces_degraded` hint flags the gap.
        let view = process_view();
        let ram_attempted = project.map(|p| view.is_warm(p)).unwrap_or(false);
        let mut surfaces_used: Vec<&'static str> = Vec::new();
        let mut surfaces_degraded: Vec<&'static str> = Vec::new();

        let query_outcome: Result<String, anyhow::Error> = if ram_attempted {
            surfaces_used.push("graph_ram");
            surfaces_degraded.push("inferred_bridge_edges_unavailable_in_ram_v1");
            Ok(self.build_impact_rows_from_ram(
                &view,
                project.unwrap_or(""),
                &target_id,
                depth,
            ))
        } else {
            surfaces_used.push("graph_pg");
            surfaces_degraded.push("graph_ram_unavailable");
            self.impact_callers_via_ist_edge(&target_id, project, depth)
                .map(Ok)
                .unwrap_or_else(|| Ok("[]".to_string()))
        };

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
                    self.build_local_projection_section(symbol, &target_id, depth, project)
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
                // REQ-AXO-901753 — SRS slice 3: legacy proximity from
                // target + impacted symbols.
                let legacy_proximity_value = self.detect_impact_legacy_proximity(
                    project,
                    &target_id,
                    &impacted_symbol_ids,
                );

                // REQ-AXO-91512 — tri-modal envelope (GUI-AXO-1003).
                let total_available = impact_radius as u64;
                let mut response = json!({
                    "content": [{ "type": "text", "text": report }],
                    "data": {
                        "surfaces_used": surfaces_used,
                        "surfaces_degraded": surfaces_degraded,
                        "total_available": total_available,
                        "next_call_hint": format!("simulate_mutation symbol={symbol}"),
                        "pagination": {
                            "offset": 0,
                            "limit": total_available,
                            "next_offset": Value::Null,
                        },
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
                if let Some(lp) = legacy_proximity_value {
                    response["data"]["legacy_proximity"] = lp;
                }
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

    /// REQ-AXO-901753 — SRS slice 3: detect legacy proximity for
    /// the target symbol and its impacted dependents.
    fn detect_impact_legacy_proximity(
        &self,
        project: Option<&str>,
        target_id: &str,
        impacted_ids: &BTreeSet<String>,
    ) -> Option<Value> {
        use std::collections::HashSet;
        let project_code = project?;
        let snapshot = self.soll_cache().snapshot(project_code).ok()?;

        let mut all_nodes: Vec<super::tools_srs::LegacyNode> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        let mut check = |artifact: &str| {
            if let Some(prox) = super::tools_srs::detect_legacy_proximity(artifact, &snapshot) {
                for node in prox.nodes {
                    if seen.insert(node.id.clone()) {
                        all_nodes.push(node);
                    }
                }
            }
        };

        check(target_id);
        for id in impacted_ids {
            check(id);
        }

        if all_nodes.is_empty() {
            return None;
        }

        let direction = all_nodes
            .first()
            .map(|n| n.strategy.direction_hint())
            .unwrap_or("review legacy linkage")
            .to_string();
        let confidence = if all_nodes.iter().all(|n| n.successor.is_some()) {
            "high"
        } else {
            "medium"
        };
        Some(json!({
            "nodes": all_nodes.iter().map(|n| json!({
                "id": n.id,
                "strategy": n.strategy,
                "successor": n.successor,
                "superseded_at": n.superseded_at,
            })).collect::<Vec<_>>(),
            "direction": direction,
            "confidence": confidence,
        }))
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

        // REQ-AXO-350 : ist.Edge replaces legacy CALLS / CALLS_NIF.
        let calls_count = self
            .graph_store
            .query_count(
                "SELECT count(*) FROM ist.Edge WHERE relation_type IN ('CALLS', 'CALLS_NIF')",
            )
            .unwrap_or(0);
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
                "SELECT s.name, s.kind FROM Symbol s LEFT JOIN Chunk ch ON ch.source_id = s.id AND ch.source_type = 'symbol' WHERE ch.file_path LIKE '%{}%' LIMIT {}",
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
        // REQ-AXO-91520 — GUI-AXO-1003 tri-modal envelope. `axon_diff`
        // parses a git diff and resolves the touched files to Symbol
        // rows via batched `Symbol JOIN CONTAINS JOIN File` queries —
        // a workspace-wide PG scan. RAM migration would require an
        // `IstGraph::symbols_in_file(path)` reverse index (file →
        // symbols) which the current snapshot does not maintain ;
        // additive surface for a follow-up slice. Envelope flags the
        // PG surface honestly.
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "surfaces_used": ["graph_pg"],
                "total_available": all_results.len() as u64,
                "truncated": truncated,
                "next_call_hint": "impact symbol=<diff-touched-symbol> for blast radius"
            }
        }))
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
        // REQ-AXO-91515 (MIL-AXO-019 vague 1d) — RAM-first via
        // IstGraphView. The blast-radius probe is a reverse BFS of
        // CALLS edges up to `depth`; the in-memory CSR walks that
        // in O(N+M) without a PG roundtrip, then falls back to
        // `ist.callers_of` SQL when the per-project cache is cold.
        let view = crate::ist_snapshot::process_view();
        let ram_attempted = project.map(|p| view.is_warm(p)).unwrap_or(false);
        let mut surfaces_used: Vec<&'static str> = Vec::new();
        let mut surfaces_degraded: Vec<&'static str> = Vec::new();

        let count: i64 = if ram_attempted {
            surfaces_used.push("graph_ram");
            let project_key = project.unwrap_or("");
            let depth_u32 = depth.clamp(1, 10) as u32;
            let callers = view
                .reverse_at_radius(project_key, &target_id, depth_u32, 10_000, &[])
                .unwrap_or_default();
            callers.len() as i64
        } else {
            // REQ-AXO-271 slice 2d : legacy CALLS table dropped ;
            // ist.Edge is canonical. `ist.callers_of` SQL
            // function (MIL-AXO-017 slice 4) wraps the WITH RECURSIVE
            // walk over `ist.Edge WHERE relation_type='CALLS'`.
            surfaces_used.push("graph_pg");
            surfaces_degraded.push("graph_ram_unavailable");
            let depth_i = depth.clamp(1, 10) as i64;
            let safe_target = target_id.replace('\'', "''");
            let sql = format!(
                "SELECT count(*) FROM ist.callers_of('{safe_target}', {depth_i}, NULL)"
            );
            self.graph_store
                .query_count(&sql)
                .unwrap_or(0)
        };

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
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "symbol": symbol,
                "project": project,
                "impact_radius": count,
                "depth": depth,
                "surfaces_used": surfaces_used,
                "surfaces_degraded": surfaces_degraded,
                "total_available": count,
                "next_call_hint": "impact symbol=<symbol> for precise affected components",
            }
        }))
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
    /// MIL-AXO-017 slice 5 (REQ-AXO-299) — query `ist.callers_of`
    /// REQ-AXO-91512 — RAM-first counterpart of
    /// `impact_callers_via_ist_edge`. Performs the reverse traversal
    /// in the in-memory IST snapshot (PIL-AXO-9002), classifies each
    /// caller's direct edge to the target via
    /// `IstGraphView::direct_edge_relation`, and materialises the
    /// 5-column row shape downstream parsers expect (`[caller_id,
    /// edge_type, origin, name, kind]`). The origin column is set to
    /// the project_code (Symbol-level granularity ; per-chunk file
    /// path materialisation stays a PG-only feature for v1).
    fn build_impact_rows_from_ram(
        &self,
        view: &crate::ist_snapshot::IstGraphView,
        project: &str,
        target_id: &str,
        depth: u64,
    ) -> String {
        let depth_u32 = depth.clamp(1, 10) as u32;
        let callers = view
            .reverse_at_radius(project, target_id, depth_u32, 10_000, &[])
            .unwrap_or_default();
        if callers.is_empty() {
            return "[]".to_string();
        }
        // Per-caller direct-edge classification (CALLS / CALLS_NIF).
        // Indirect callers (distance > 1) have no direct edge to the
        // target ; we mark them `calls` to preserve the legacy
        // confidence-label arithmetic (`direct_edges + nif_edges > 0
        // ⇒ high confidence`), since the RAM snapshot guarantees a
        // real graph path (no text-matching heuristic).
        let mut edge_type_by_caller: std::collections::HashMap<String, &'static str> =
            std::collections::HashMap::new();
        for caller in &callers {
            // REQ-AXO-91505 — surface the new IMPLEMENTS / IMPORTS / USES
            // edge kinds emitted by the parsers (Rust traits, Elixir
            // protocols, every-language import/use lines). Falls back to
            // "calls" so the legacy confidence-label arithmetic (direct +
            // nif > 0 ⇒ high confidence) still works for unknown edges.
            let label = match view.direct_edge_relation(project, caller, target_id) {
                Some(RelationType::CallsNif) => "calls_nif",
                Some(RelationType::Calls) => "calls",
                Some(RelationType::Implements) => "implements",
                Some(RelationType::Imports) => "imports",
                Some(RelationType::Uses) => "uses",
                Some(_) | None => "calls",
            };
            edge_type_by_caller.insert(caller.clone(), label);
        }
        // Single batch SQL to materialise name + kind + project.
        let escaped: Vec<String> = callers
            .iter()
            .map(|id| format!("'{}'", id.replace('\'', "''")))
            .collect();
        let sql = format!(
            "SELECT id, name, kind, COALESCE(project_code, 'unknown') \
             FROM ist.Symbol WHERE id IN ({})",
            escaped.join(", ")
        );
        let raw = self
            .graph_store
            .query_json(&sql)
            .unwrap_or_else(|_| "[]".to_string());
        let lookup_rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut meta_by_id: std::collections::HashMap<String, (String, String, String)> =
            std::collections::HashMap::new();
        for row in &lookup_rows {
            let id = row.first().and_then(Value::as_str).unwrap_or("").to_string();
            let name = row.get(1).and_then(Value::as_str).unwrap_or("-").to_string();
            let kind = row.get(2).and_then(Value::as_str).unwrap_or("-").to_string();
            let proj = row.get(3).and_then(Value::as_str).unwrap_or("unknown").to_string();
            if !id.is_empty() {
                meta_by_id.insert(id, (name, kind, proj));
            }
        }
        // Build the 5-column row format the downstream parser expects.
        let rows: Vec<Value> = callers
            .iter()
            .map(|caller_id| {
                let edge_type = edge_type_by_caller
                    .get(caller_id)
                    .copied()
                    .unwrap_or("calls");
                let (name, kind, origin) = meta_by_id
                    .get(caller_id)
                    .cloned()
                    .unwrap_or_else(|| ("-".to_string(), "-".to_string(), project.to_string()));
                json!([caller_id, edge_type, origin, name, kind])
            })
            .collect();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
    }

    /// PG fallback (REQ-AXO-296 / REQ-AXO-901869 A3) for `axon_impact`'s
    /// reverse-traversal callers. Used ONLY when the canonical RAM
    /// `IstGraphView` snapshot is cold (brain_only boot, RAM disabled,
    /// tests). The warm/prod path is `build_impact_rows_from_ram`
    /// (PIL-AXO-9002 — the in-memory graph is canonical; PG is the
    /// degraded fallback, not the source of truth). Wraps
    /// `ist.callers_of` (WITH RECURSIVE on `ist.Edge`) and joins
    /// `ist.Symbol` + `ist.Chunk` to surface the 5-column row shape
    /// `axon_impact` parses: (caller_id, edge_type, origin, name, kind).
    /// Returns `None` on empty / error so the caller degrades to
    /// `axon_impact_without_calls`.
    fn impact_callers_via_ist_edge(
        &self,
        target_id: &str,
        project: Option<&str>,
        depth: u64,
    ) -> Option<String> {
        let depth_clamped = depth.clamp(1, 10) as i32;
        let project_code_param = project.unwrap_or("");
        // REQ-AXO-901869 A3 root cause: the prior SQL used `\`
        // line-continuations, which strip the next line's leading
        // whitespace and glued `relation_type` to `WHEN` →
        // `relation_typeWHEN` → "syntax error at or near THEN" →
        // every cold-cache impact silently returned `[]`. We now use a
        // plain multi-line literal (newlines kept) so no two SQL tokens
        // can ever be welded together. Named params (`$target`/`$proj`)
        // are inlined as escaped literals by `expand_named_params`.
        let sql = format!(
            "WITH callers AS (
                 SELECT source_id, distance, relation_type
                 FROM ist.callers_of($target, {depth_clamped}::INT, $proj)
             ),
             enriched AS (
                 SELECT c.source_id AS caller_id,
                        CASE c.relation_type
                            WHEN 'CALLS' THEN 'calls'
                            WHEN 'CALLS_NIF' THEN 'calls_nif'
                            ELSE lower(c.relation_type)
                        END AS edge_type,
                        COALESCE(s.name, '-') AS name,
                        COALESCE(s.kind, '-') AS kind,
                        COALESCE(MIN(ch.file_path), '-') AS origin
                 FROM callers c
                 LEFT JOIN ist.Symbol s ON s.id = c.source_id
                 LEFT JOIN ist.Chunk ch ON ch.source_id = c.source_id AND ch.source_type = 'symbol'
                 GROUP BY c.source_id, c.relation_type, s.name, s.kind
             )
             SELECT caller_id, edge_type, origin, name, kind FROM enriched"
        );
        let params = serde_json::json!({ "target": target_id, "proj": project_code_param });
        let raw = match self.graph_store.query_json_param(&sql, &params) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("impact_callers_via_ist_edge: SQL query failed: {}", e);
                return None;
            }
        };
        if raw.trim() == "[]" {
            return None;
        }
        Some(raw)
    }

    // MIL-AXO-017 slice 6B: AGE helper impact_callers_via_age removed ; ist.callers_of is canonical.
}
