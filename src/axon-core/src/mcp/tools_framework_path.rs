use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;

impl McpServer {
    pub(super) fn axon_path_impl(&self, args: &Value) -> Option<Value> {
        let source = args.get("source")?.as_str()?.trim();
        if source.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "path requires a non-empty `source`" }],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "parameter_repair": {
                        "invalid_field": "source",
                        "follow_up_tools": ["help", "query"],
                        "hint": "supply a non-empty `source` symbol; use `query` to discover symbol names"
                    }
                }
            }));
        }
        let sink = args
            .get("sink")
            .and_then(|value| value.as_str())
            .map(str::trim);
        let project = args.get("project").and_then(|value| value.as_str());
        let depth = args
            .get("depth")
            .and_then(|value| value.as_u64())
            .unwrap_or(6)
            .clamp(1, 12);
        let mode = args.get("mode").and_then(|value| value.as_str());

        if sink.is_none() {
            return self.axon_bidi_trace(&json!({
                "symbol": source,
                "project": project,
                "depth": depth,
                "mode": mode.unwrap_or("brief")
            }));
        }

        let sink = sink.unwrap_or_default();
        let Some(source_id) = self.resolve_scoped_symbol_id_canonical(source, project) else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("path source '{}' not found in current scope", source) }],
                "isError": true,
                "data": {
                    "status": "input_not_found",
                    "parameter_repair": {
                        "invalid_field": "source",
                        "supplied_value": source,
                        "follow_up_tools": ["query", "inspect"],
                        "hint": format!("symbol `{}` does not resolve in scope; widen via `query` or pass a canonical symbol id", source)
                    }
                }
            }));
        };
        let Some(sink_id) = self.resolve_scoped_symbol_id_canonical(sink, project) else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("path sink '{}' not found in current scope", sink) }],
                "isError": true,
                "data": {
                    "status": "input_not_found",
                    "parameter_repair": {
                        "invalid_field": "sink",
                        "supplied_value": sink,
                        "follow_up_tools": ["query", "inspect"],
                        "hint": format!("symbol `{}` does not resolve in scope; widen via `query` or pass a canonical symbol id", sink)
                    }
                }
            }));
        };

        // MIL-AXO-015 B.3: under PG with AXON_AGE_READ=true, dump
        // adjacency from the AGE graph (populated by B.2 dual-write).
        // Vertex enrichment (commit 0f3828b) put name/project_code
        // on each Symbol so the same RETURN shape as the SQL path is
        // achievable without a JOIN. Falls back to SQL on empty /
        // error so existing PG installs without dual-write still work.
        let raw = if self.graph_store.is_postgres_backend()
            && crate::postgres::age::age_read_enabled()
        {
            self.path_edges_via_age(project).unwrap_or_else(|| {
                self.path_edges_via_sql(project)
                    .unwrap_or_else(|| "[]".to_string())
            })
        } else {
            self.path_edges_via_sql(project)
                .unwrap_or_else(|| "[]".to_string())
        };
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut adjacency: std::collections::HashMap<String, Vec<(String, String, String)>> =
            std::collections::HashMap::new();
        let mut source_name = source.to_string();
        for row in rows {
            if row.len() < 5 {
                continue;
            }
            if row[0] == source_id {
                source_name = row[1].clone();
            }
            adjacency.entry(row[0].clone()).or_default().push((
                row[2].clone(),
                row[3].clone(),
                row[4].clone(),
            ));
        }

        let mut queue = std::collections::VecDeque::new();
        queue.push_back((
            source_id.clone(),
            vec![source_id.clone()],
            vec![source_name],
            vec!["anchor".to_string()],
            0_u64,
        ));

        let mut resolved_path: Option<(Vec<String>, Vec<String>)> = None;
        while let Some((node_id, path_ids, path_names, edge_kinds, current_depth)) =
            queue.pop_front()
        {
            if node_id == sink_id {
                resolved_path = Some((path_names, edge_kinds));
                break;
            }
            if current_depth >= depth {
                continue;
            }
            if let Some(neighbors) = adjacency.get(&node_id) {
                for (target_id, target_name, edge_type) in neighbors {
                    if path_ids.iter().any(|seen| seen == target_id) {
                        continue;
                    }
                    let mut next_ids = path_ids.clone();
                    next_ids.push(target_id.clone());
                    let mut next_names = path_names.clone();
                    next_names.push(target_name.clone());
                    let mut next_edges = edge_kinds.clone();
                    next_edges.push(edge_type.clone());
                    queue.push_back((
                        target_id.clone(),
                        next_ids,
                        next_names,
                        next_edges,
                        current_depth + 1,
                    ));
                }
            }
        }

        let Some((path, edges)) = resolved_path else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("No path found between '{}' and '{}' within depth {}", source, sink, depth) }],
                "isError": true,
                "data": {
                    "source": source,
                    "sink": sink,
                    "depth": depth,
                    "path_found": false,
                    "path_type": "bounded_call_path",
                    "detours": [],
                    "bounded_depth_used": depth,
                    "operator_guidance": {
                        "actionable_now": false,
                        "blocking_factors": [{
                            "factor": "no_path_found_within_depth",
                            "severity": "medium",
                            "recommended_action": "increase depth or inspect the endpoints individually before assuming there is no reachable path"
                        }],
                        "remediation_actions": [
                            "increase depth or inspect the endpoints individually before assuming there is no reachable path"
                        ],
                        "follow_up_tools": ["inspect", "impact"],
                        "next_action": {
                            "kind": "inspect_endpoints_or_increase_depth",
                            "tool": "inspect",
                            "when": "now"
                        }
                    },
                    "next_action": {
                        "kind": "inspect_endpoints_or_increase_depth",
                        "tool": "inspect",
                        "when": "now"
                    },
                    "canonical_sources": Self::canonical_sources_snapshot(),
                    "parameter_repair": {
                        "invalid_field": "depth",
                        "supplied_value": depth,
                        "max_depth": 12,
                        "follow_up_tools": ["inspect", "impact"],
                        "hint": format!("no path within depth {}; retry with a larger depth (max 12), or call `inspect` on each endpoint to verify they live in the same call graph", depth)
                    }
                }
            }));
        };
        let evidence = format!(
            "**Source:** `{}`\n\
**Sink:** `{}`\n\
**Depth used:** {}\n\
**Path:** {}\n\
**Edges:** {}\n",
            source,
            sink,
            depth,
            path.join(" -> "),
            edges.join(" -> ")
        );
        let report = format!(
            "## 🧭 Axon Path\n\n{}",
            format_standard_contract(
                "ok",
                "bounded path computed",
                &project
                    .map(|value| format!("project:{}", value))
                    .unwrap_or_else(|| "workspace:*".to_string()),
                &evidence_by_mode(&evidence, mode),
                &[
                    "run `impact` to expand blast radius",
                    "run `why` to join rationale"
                ],
                "medium",
            )
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "source": source,
                "sink": sink,
                "depth": depth,
                "bounded_depth_used": depth,
                "path_found": true,
                "path_type": "bounded_call_path",
                "path": path,
                "edge_kinds": edges,
                "detours": [],
                "confidence": "medium",
                "provenance": "extracted_recursive_calls",
                "evidence_sources": ["CALLS", "CALLS_NIF", "CONTAINS"],
                "safe_to_act": false,
                "needs_human_confirmation": true,
                "operator_guidance": {
                    "actionable_now": true,
                    "blocking_factors": [],
                    "remediation_actions": [],
                    "follow_up_tools": ["impact", "why"],
                    "next_action": {
                        "kind": "expand_blast_radius_from_path",
                        "tool": "impact",
                        "when": "now"
                    }
                },
                "next_action": {
                    "kind": "expand_blast_radius_from_path",
                    "tool": "impact",
                    "when": "now"
                },
                "canonical_sources": Self::canonical_sources_snapshot()
            }
        }))
    }

    /// MIL-AXO-015 B.3: dump CALLS / CALLS_NIF adjacency from the SQL
    /// relation tables. Returns the same JSON shape consumed by the
    /// in-memory BFS in `axon_path_impl`: rows of `[src.id, src.name,
    /// dst.id, dst.name, edge_type]`. Returns `None` on query error
    /// so the caller can fall back to AGE / empty / etc.
    fn path_edges_via_sql(&self, project: Option<&str>) -> Option<String> {
        let edge_query = if let Some(project) = project {
            format!(
                "WITH all_edges AS (
                    SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                    UNION ALL
                    SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
                )
                SELECT src.id, src.name, dst.id, dst.name, e.edge_type
                FROM all_edges e
                JOIN Symbol src ON src.id = e.source_id
                JOIN Symbol dst ON dst.id = e.target_id
                WHERE src.project_code = '{project}'
                  AND dst.project_code = '{project}'",
                project = project.replace('\'', "''")
            )
        } else {
            "WITH all_edges AS (
                SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                UNION ALL
                SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
            )
            SELECT src.id, src.name, dst.id, dst.name, e.edge_type
            FROM all_edges e
            JOIN Symbol src ON src.id = e.source_id
            JOIN Symbol dst ON dst.id = e.target_id"
                .to_string()
        };
        self.graph_store.query_json(&edge_query).ok()
    }

    /// MIL-AXO-015 B.3: dump the same adjacency from the AGE graph
    /// via Cypher MATCH. Relies on B.2 vertex enrichment (commit
    /// 0f3828b) so `s.name` / `s.project_code` are searchable. Returns
    /// `None` on:
    /// - identifier validation failure (invalid project literal)
    /// - cypher_query SQL build failure
    /// - graph_store.query_json error (AGE empty, schema missing, …)
    /// - empty result (caller falls back to SQL — safer than serving
    ///   an empty path response when the AGE graph is unpopulated).
    fn path_edges_via_age(&self, project: Option<&str>) -> Option<String> {
        // AGE doesn't bind params; inline the project filter after
        // single-quote escaping. validate_identifier rejects values
        // that contain `$$` / `\n` / `;` / quotes.
        let where_clause = if let Some(project_code) = project {
            if crate::postgres::age::validate_identifier(project_code, "project_code").is_err() {
                log::warn!(
                    "path_edges_via_age: project_code '{}' fails AGE identifier validation; falling back",
                    project_code
                );
                return None;
            }
            format!(
                "WHERE src.project_code = \"{project_code}\" AND dst.project_code = \"{project_code}\""
            )
        } else {
            String::new()
        };
        // AGE doesn't support edge-label alternation `[r:CALLS|CALLS_NIF]`
        // (verified via syntax error against pg17/age 1.6.0). Two
        // MATCH clauses joined by UNION cover the same shape.
        let cypher = format!(
            "MATCH (src:Symbol)-[r:CALLS]->(dst:Symbol) {where_clause} \
             RETURN src.id, src.name, dst.id, dst.name, 'CALLS' AS edge_type \
             UNION \
             MATCH (src:Symbol)-[r:CALLS_NIF]->(dst:Symbol) {where_clause} \
             RETURN src.id, src.name, dst.id, dst.name, 'CALLS_NIF' AS edge_type"
        );
        let sql = match crate::postgres::age::cypher_query(
            "axon_graph",
            &cypher,
            &["src_id", "src_name", "dst_id", "dst_name", "edge_type"],
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("path_edges_via_age: cypher_query build failed: {}", e);
                return None;
            }
        };
        let raw = match self.graph_store.query_json(&sql) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("path_edges_via_age: AGE query failed: {}", e);
                return None;
            }
        };
        // Empty result -> fall back to SQL. AGE may not have been
        // populated yet (dual-write opt-in or fresh deployment).
        if raw.trim() == "[]" {
            return None;
        }
        Some(raw)
    }
}
