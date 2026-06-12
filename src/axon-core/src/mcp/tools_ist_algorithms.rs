// REQ-AXO-91488 (MIL-AXO-019 slice 4) — MCP tools for advanced IST algos.
//
// Three tools expose petgraph-backed algorithms over the in-memory CSR
// snapshot. All three reuse the process IstSnapshotCache (slice 1) and
// dispatch on it. Cache miss / disabled → structured error with hint to
// run `ist_snapshot_warm` first.

use serde_json::{json, Value};

use crate::ist_snapshot::algorithms::{pagerank_top, shortest_path, structural_sccs};
use crate::ist_snapshot::{process_view, IstSnapshotCache};
use crate::mcp::McpServer;

impl McpServer {
    pub(crate) fn axon_ist_centrality_pagerank(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "ist_centrality_pagerank") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let top = args
            .get("top")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(20);
        let damping = args
            .get("damping")
            .and_then(|v| v.as_f64())
            .map(|d| d as f32)
            .unwrap_or(0.85);
        let iterations = args
            .get("iterations")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(50);

        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("ist_centrality_pagerank", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => {
                return Some(ist_cache_miss_error("ist_centrality_pagerank", &project));
            }
        };

        let pairs = pagerank_top(&snapshot, damping, iterations, top);
        let rows: Vec<Value> = pairs
            .iter()
            .enumerate()
            .map(|(rank, (id, score))| {
                json!({
                    "rank": rank + 1,
                    "id": id,
                    "score": score,
                })
            })
            .collect();
        let summary = if pairs.is_empty() {
            format!("ist_centrality_pagerank {} : empty snapshot", project)
        } else {
            format!(
                "ist_centrality_pagerank {} top {} (damping={}, iter={}) — top: {}",
                project, top, damping, iterations, pairs[0].0
            )
        };
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": "ok",
                "project_code": project,
                "node_count": snapshot.node_count(),
                "edge_count": snapshot.edge_count(),
                "top_n": top,
                "damping": damping,
                "iterations": iterations,
                "results": rows
            }
        }))
    }

    pub(crate) fn axon_ist_structural_sccs(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "ist_structural_sccs") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("ist_structural_sccs", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => {
                return Some(ist_cache_miss_error("ist_structural_sccs", &project));
            }
        };

        let sccs = structural_sccs(&snapshot);
        let payload: Vec<Value> = sccs
            .iter()
            .map(|c| {
                json!({
                    "size": c.len(),
                    "nodes": c
                })
            })
            .collect();
        let summary = if sccs.is_empty() {
            format!(
                "ist_structural_sccs {} : 0 SCC>1 detected ({} nodes, {} edges)",
                project,
                snapshot.node_count(),
                snapshot.edge_count()
            )
        } else {
            format!(
                "ist_structural_sccs {} : {} SCC>1 (largest size = {})",
                project,
                sccs.len(),
                sccs[0].len()
            )
        };
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": if sccs.is_empty() { "ok" } else { "cycles_detected" },
                "project_code": project,
                "node_count": snapshot.node_count(),
                "edge_count": snapshot.edge_count(),
                "scc_count": sccs.len(),
                "sccs": payload
            }
        }))
    }

    pub(crate) fn axon_ist_shortest_path(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "ist_shortest_path") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let from = args
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if from.is_empty() || to.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "ist_shortest_path requires `from` and `to` canonical ids." }],
                "isError": true,
                "data": {
                    "status": "missing_endpoints",
                    "parameter_repair": {
                        "invalid_field": if from.is_empty() { "from" } else { "to" },
                        "tool": "ist_shortest_path",
                        "follow_up_tools": ["query", "inspect"]
                    }
                }
            }));
        }
        let max_radius = args
            .get("max_radius")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(20);

        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("ist_shortest_path", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => return Some(ist_cache_miss_error("ist_shortest_path", &project)),
        };

        let path_opt = shortest_path(&snapshot, &from, &to, max_radius, &[]);
        match path_opt {
            None => Some(json!({
                "content": [{ "type": "text", "text": format!("ist_shortest_path {} : no path from {} to {} within radius {}", project, from, to, max_radius) }],
                "data": {
                    "status": "no_path",
                    "project_code": project,
                    "from": from,
                    "to": to,
                    "max_radius": max_radius,
                    "path": Value::Null
                }
            })),
            Some(path) => {
                let hops = path.len().saturating_sub(1);
                Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("ist_shortest_path {} : {} → {} via {} hop(s)", project, from, to, hops)
                    }],
                    "data": {
                        "status": "ok",
                        "project_code": project,
                        "from": from,
                        "to": to,
                        "max_radius": max_radius,
                        "hops": hops,
                        "path": path
                    }
                }))
            }
        }
    }

    fn ist_resolve_project(&self, args: &Value, tool: &str) -> Result<String, Value> {
        let raw = args.get("project_code").and_then(|v| v.as_str());
        match raw {
            Some(code) => self
                .resolve_project_code(code)
                .map_err(|_| self.wrong_project_scope_response(code, tool)),
            None => Err(json!({
                "content": [{ "type": "text", "text": format!("{} requires project_code", tool) }],
                "isError": true,
                "data": {
                    "status": "missing_project_code",
                    "parameter_repair": {
                        "invalid_field": "project_code",
                        "tool": tool,
                        "follow_up_tools": ["project_registry_lookup", "help"]
                    }
                }
            })),
        }
    }
}

fn ist_cache_miss_error(tool: &str, project: &str) -> Value {
    // REQ-AXO-901952 — RAM is unconditional (no opt-out) ; `is_enabled()` is a
    // status reporter that is always true. A cache miss means the snapshot is
    // cold, not disabled : the only remedy is to warm it.
    let enabled = IstSnapshotCache::is_enabled();
    let hint = format!(
        "call `ist_snapshot_warm project_code={}` first ; then retry",
        project
    );
    json!({
        "content": [{
            "type": "text",
            "text": format!(
                "{} : IST RAM snapshot not warm for {}. {}",
                tool, project, hint
            )
        }],
        "isError": true,
        "data": {
            "status": "ist_cache_miss",
            "project_code": project,
            "ram_enabled": enabled,
            "parameter_repair": {
                "invalid_field": "ist_cache_snapshot",
                "tool": tool,
                "follow_up_tools": ["ist_snapshot_warm", "status"],
                "hint": hint
            }
        }
    })
}
