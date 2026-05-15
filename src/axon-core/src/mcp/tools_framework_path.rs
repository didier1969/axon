use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;

/// REQ-AXO-299 / MIL-AXO-017 slice 5 : build the SQL that wraps
/// `public.path` and LEFT-JOINs Symbol to materialize names alongside
/// hops. Pure formatter — extracted so the SQL-escape contract is unit
/// testable without a live PG backend.
fn build_path_sql(source_id: &str, sink_id: &str, depth: u64, project: &str) -> String {
    format!(
        "SELECT p.hop, p.node_id, COALESCE(s.name, p.node_id) AS name, \
                COALESCE(p.relation_type, 'anchor') AS relation_type \
         FROM public.path('{src}', '{snk}', {depth}, '{proj}') p \
         LEFT JOIN public.Symbol s ON s.id = p.node_id \
         ORDER BY p.hop",
        src = source_id.replace('\'', "''"),
        snk = sink_id.replace('\'', "''"),
        depth = depth,
        proj = project.replace('\'', "''"),
    )
}

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

        // REQ-AXO-299 / MIL-AXO-017 slice 5 : thin wrapper on public.path SQL
        // function (db/ddl/04_graph_functions.sql). The fn returns one row per
        // hop on the shortest path discovered (cycle-safe WITH RECURSIVE on
        // public.Edge). We JOIN with public.Symbol to materialize names.
        let sql = build_path_sql(&source_id, &sink_id, depth, project.unwrap_or_default());
        let raw = self
            .graph_store
            .query_json(&sql)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();

        let resolved_path: Option<(Vec<String>, Vec<String>)> = if rows.is_empty() {
            None
        } else {
            let mut path_names: Vec<String> = Vec::with_capacity(rows.len());
            let mut edge_kinds: Vec<String> = Vec::with_capacity(rows.len());
            for row in &rows {
                let name = row
                    .get(2)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let rel = row
                    .get(3)
                    .and_then(|v| v.as_str())
                    .unwrap_or("anchor")
                    .to_string();
                path_names.push(name);
                edge_kinds.push(rel);
            }
            Some((path_names, edge_kinds))
        };

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
                "provenance": "public.path SQL function (WITH RECURSIVE on public.Edge)",
                "evidence_sources": ["public.Edge"],
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
}

#[cfg(test)]
mod tests {
    use super::build_path_sql;

    #[test]
    fn build_path_sql_wraps_public_path_with_symbol_join() {
        let sql = build_path_sql("foo", "bar", 5, "AXO");
        assert!(
            sql.contains("FROM public.path('foo', 'bar', 5, 'AXO')"),
            "must call public.path SQL fn with positional args: {sql}"
        );
        assert!(
            sql.contains("LEFT JOIN public.Symbol s ON s.id = p.node_id"),
            "must JOIN public.Symbol to materialize names: {sql}"
        );
        assert!(
            sql.contains("ORDER BY p.hop"),
            "must order rows by hop: {sql}"
        );
    }

    #[test]
    fn build_path_sql_escapes_single_quotes_and_unscoped_when_project_empty() {
        let sql = build_path_sql("o'brien", "ba'r", 3, "");
        assert!(
            sql.contains("public.path('o''brien', 'ba''r', 3, '')"),
            "must double single quotes for SQL safety and pass '' for unscoped: {sql}"
        );
    }
}
