use std::collections::HashMap;

use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;
use crate::ist_snapshot::process_view;
use crate::ist_snapshot::RelationType;

/// REQ-AXO-91510 — single-shot name materialization for a path's node ids.
/// The BFS itself runs in RAM via IstGraphView ; this helper does ONE
/// round-trip on ist.Symbol to render human-friendly names. Returns a
/// map id → name ; ids without a hit are absent (caller falls back to id).
fn build_name_lookup_sql(ids: &[String]) -> String {
    let escaped: Vec<String> = ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect();
    format!(
        "SELECT id, name FROM ist.Symbol WHERE id IN ({})",
        escaped.join(", ")
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
        // REQ-AXO-901922 — auto-resolve project_code (AXON_PROJECT_ROOT/cwd →
        // registry, like inspect REQ-AXO-089) so the RAM snapshot is consulted
        // even when the caller omits `project`. Without this, `project` stayed
        // None → `ram_attempted` false → unscoped PG fallback returned empty.
        // `auto_project` must outlive `project` (borrowed via as_deref).
        let explicit_project = args.get("project").and_then(|value| value.as_str());
        let auto_project = if explicit_project.is_none() {
            self.auto_resolve_project_code_str()
        } else {
            None
        };
        let project = explicit_project.or(auto_project.as_deref());
        let depth = args
            .get("depth")
            .and_then(|value| value.as_u64())
            .unwrap_or(6)
            .clamp(1, 12);
        // REQ-AXO-902019 — how many node-disjoint routes to enumerate. >1 routes
        // is the multiplicity/redundancy signal the caller asks for ("is there
        // MORE than one path?"). Default 3, capped at 5 to bound the BFS cost.
        let max_paths = args
            .get("max_paths")
            .and_then(|value| value.as_u64())
            .unwrap_or(3)
            .clamp(1, 5) as usize;
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

        // REQ-AXO-91510 — RAM-first via IstGraphView (PIL-AXO-9002).
        // `feedback_trimodal_use_ram_graph_not_pg` mandates the in-memory
        // CSR snapshot for structural/graph tools ; PG `ist.path` is
        // only the degraded fallback when the cache is cold or the query
        // is project-unscoped (cache is per-project).
        // REQ-AXO-901922 — lazy-warm the RAM snapshot (brain start does not
        // auto-populate it) so the BFS runs in RAM instead of falling to the
        // PG `ist.path` fallback. `view` reads the cache live per call, so
        // warming before traversal is sufficient.
        // REQ-AXO-901952 — RAM is the SINGLE source for path traversal.
        // Requires a project-scoped, warmed snapshot ; cold cache or an
        // unscoped (project=None) query → loud degraded error, never a PG
        // fallback and never a silent "no path".
        let ram_attempted = project
            .map(|p| self.ensure_ram_snapshot_warm(p))
            .unwrap_or(false);
        if !ram_attempted {
            let why = if project.is_none() {
                "path requires an explicit `project` scope : the RAM IST snapshot is per-project (REQ-AXO-901952, no PG fallback)"
            } else {
                "IST RAM snapshot is cold for this project and could not be warmed ; call `ist_snapshot_warm` then retry (REQ-AXO-901952, no PG fallback)"
            };
            return Some(Self::path_ram_unavailable_error(source, sink, depth, why));
        }
        let view = process_view();
        // REQ-AXO-902019 — enumerate up to `max_paths` node-disjoint routes. The
        // first is the shortest path (backward-compatible `path`/`edge_kinds`);
        // the rest are independent alternates surfaced as `detours[]`, and their
        // count drives the `multiplicity` verdict.
        let ram_routes = view
            .disjoint_paths(
                project.unwrap_or_default(),
                &source_id,
                &sink_id,
                depth as u32,
                &[],
                max_paths,
            )
            .unwrap_or_default();

        let surfaces_used: Vec<&'static str> = vec!["graph_ram"];
        let surfaces_degraded: Vec<&'static str> = Vec::new();

        // One batch SELECT on ist.Symbol materializes display names across the
        // ids of EVERY route, so the per-route mapping is a cheap HashMap lookup.
        let all_ids: Vec<String> = ram_routes
            .iter()
            .flat_map(|(ids, _)| ids.iter().cloned())
            .collect();
        let name_by_id: HashMap<String, String> = if all_ids.is_empty() {
            HashMap::new()
        } else {
            let lookup_sql = build_name_lookup_sql(&all_ids);
            let raw = self
                .graph_store
                .query_json(&lookup_sql)
                .unwrap_or_else(|_| "[]".to_string());
            let lookup_rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
            lookup_rows
                .iter()
                .filter_map(|row| {
                    match (
                        row.first().and_then(Value::as_str),
                        row.get(1).and_then(Value::as_str),
                    ) {
                        (Some(id), Some(name)) => Some((id.to_string(), name.to_string())),
                        _ => None,
                    }
                })
                .collect()
        };
        let map_route = |ids: &[String], rels: &[RelationType]| -> (Vec<String>, Vec<String>) {
            let kinds: Vec<String> = rels
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    if i == 0 {
                        "anchor".to_string()
                    } else {
                        r.as_db().to_lowercase()
                    }
                })
                .collect();
            let displayed: Vec<String> = ids
                .iter()
                .map(|id| name_by_id.get(id).cloned().unwrap_or_else(|| id.clone()))
                .collect();
            (displayed, kinds)
        };
        let resolved_path: Option<(Vec<String>, Vec<String>)> = ram_routes
            .first()
            .map(|(ids, rels)| map_route(ids, rels));
        // Independent alternate routes (node-disjoint on intermediates).
        let detours: Vec<Value> = ram_routes
            .iter()
            .skip(1)
            .map(|(ids, rels)| {
                let (names, kinds) = map_route(ids, rels);
                json!({ "path": names, "edge_kinds": kinds })
            })
            .collect();
        let route_multiplicity = ram_routes.len();

        let Some((path, edges)) = resolved_path else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("No path found between '{}' and '{}' within depth {}", source, sink, depth) }],
                "isError": true,
                "data": {
                    "surfaces_used": surfaces_used,
                    "surfaces_degraded": surfaces_degraded,
                    "total_available": 0,
                    "next_call_hint": format!("inspect symbol={source}"),
                    "pagination": {
                        "offset": 0,
                        "limit": depth,
                        "next_offset": Value::Null,
                    },
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
        // REQ-AXO-902019 — surface the multiplicity verdict in the human report.
        let multiplicity_line = if route_multiplicity > 1 {
            format!(
                "**Routes:** {} node-disjoint (redundancy candidate — verify it is not a deliberate fast-path)\n",
                route_multiplicity
            )
        } else {
            "**Routes:** 1 (no independent alternate within depth)\n".to_string()
        };
        let evidence = format!(
            "**Source:** `{}`\n\
**Sink:** `{}`\n\
**Depth used:** {}\n\
**Path:** {}\n\
**Edges:** {}\n\
{}",
            source,
            sink,
            depth,
            path.join(" -> "),
            edges.join(" -> "),
            multiplicity_line
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
        // REQ-AXO-91510 — tri-modal structured envelope for `path` per
        // GUI-AXO-1003 / CPT-AXO-90007 (structural / graph primary).
        // Surface choice : `graph_ram` when the in-memory IST snapshot
        // serves the BFS (PIL-AXO-9002, feedback_trimodal_use_ram_graph_
        // not_pg) ; `graph_pg` only when the cache is cold or the query
        // is project-unscoped. Source of truth for traversal logic
        // lives in IstGraph::bfs_shortest_path (RAM) or ist.path SQL
        // (PG fallback). No vector, no FTS, no RRF.
        // Like inspect (REQ-AXO-91509), no `results[]` is added: the
        // path itself IS the result, already exposed as `data.path[]`
        // and `data.edge_kinds[]`. Adding a parallel `results[]` would
        // inflate the bench `name`-key denominator without helping
        // LLM consumers.
        let path_len = path.len() as u64;
        let provenance =
            "IstGraph::bfs_disjoint_paths (RAM CSR snapshot, PIL-AXO-9002, REQ-AXO-902019)";
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "surfaces_used": surfaces_used,
                "surfaces_degraded": surfaces_degraded,
                "total_available": route_multiplicity,
                "next_call_hint": format!("impact symbol={sink}"),
                "pagination": {
                    "offset": 0,
                    "limit": path_len,
                    "next_offset": Value::Null,
                },
                "source": source,
                "sink": sink,
                "depth": depth,
                "bounded_depth_used": depth,
                "path_found": true,
                "path_type": "bounded_call_path",
                "path": path,
                "edge_kinds": edges,
                // REQ-AXO-902019 — independent alternate routes + multiplicity
                // verdict. >1 node-disjoint route = a redundancy candidate
                // (GUI-PRO-107 L1) unless deliberate (perf fast-path).
                "detours": detours,
                "multiplicity": {
                    "route_count": route_multiplicity,
                    "has_independent_alternates": route_multiplicity > 1,
                    "interpretation": if route_multiplicity > 1 {
                        "multiple node-disjoint routes — redundancy candidate (verify it is not a deliberate fast-path) (GUI-PRO-107 L1)"
                    } else {
                        "single route — no independent alternate within depth"
                    }
                },
                "confidence": "medium",
                "provenance": provenance,
                "evidence_sources": ["ist.Edge"],
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

    /// REQ-AXO-901952 — loud degraded error when the RAM IST snapshot cannot
    /// serve `path` (cold cache or unscoped query). No PG fallback, never a
    /// silent "no path".
    fn path_ram_unavailable_error(source: &str, sink: &str, depth: u64, why: &str) -> Value {
        json!({
            "content": [{ "type": "text", "text": format!("path unavailable : {why}") }],
            "isError": true,
            "data": {
                "status": "degraded",
                "surfaces_used": [],
                "surfaces_degraded": ["graph_ram_unavailable"],
                "total_available": Value::Null,
                "next_call_hint": "ist_snapshot_warm project_code=<project>",
                "source": source,
                "sink": sink,
                "depth": depth,
                "path_found": false,
                "operator_guidance": {
                    "actionable_now": false,
                    "blocking_factors": [{
                        "factor": "ist_ram_snapshot_unavailable",
                        "severity": "high",
                        "recommended_action": why
                    }],
                    "follow_up_tools": ["ist_snapshot_warm", "status"],
                    "next_action": { "kind": "warm_ram_snapshot", "tool": "ist_snapshot_warm", "when": "now" }
                },
                "next_action": { "kind": "warm_ram_snapshot", "tool": "ist_snapshot_warm", "when": "now" },
                "parameter_repair": {
                    "invalid_field": "project",
                    "follow_up_tools": ["ist_snapshot_warm", "status"],
                    "hint": why
                }
            }
        })
    }
}
