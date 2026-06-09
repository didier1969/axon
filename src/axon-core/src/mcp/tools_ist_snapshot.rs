// REQ-AXO-91486 — MCP tool to warm the IstSnapshot cache for a project.
//
// Without LISTEN/NOTIFY sync (slice 3 ; REQ-AXO-91487), the in-memory
// snapshot is not auto-populated on brain start. This tool lets the
// operator (or a startup script) warm the cache explicitly before
// hitting the migrated call-sites, ensuring the RAM fast-path fires.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::ist_snapshot::{load_snapshot, loader::JsonSqlStore, publish_process_snapshot};
use crate::mcp::McpServer;

struct GraphStoreSqlAdapter<'a> {
    inner: &'a crate::graph::GraphStore,
}

impl<'a> JsonSqlStore for GraphStoreSqlAdapter<'a> {
    fn query_json(&self, sql: &str) -> Result<String, String> {
        self.inner.query_json(sql).map_err(|e| e.to_string())
    }
}

impl McpServer {
    /// REQ-AXO-901922 — lazy-warm the process IST snapshot for `project_code`
    /// so RAM-first structural tools (path / bidi_trace / impact) never fall
    /// to a hardcoded-empty result just because the cache was never explicitly
    /// warmed. Brain start does NOT auto-populate the snapshot (see module
    /// header: REQ-AXO-91487 LISTEN/NOTIFY sync unshipped), so without this
    /// every fresh session saw empty traversals until an operator called
    /// `ist_snapshot_warm` by hand. Returns the TRUE warmth after the attempt
    /// (honours AXON_IST_RAM_ENABLED via `is_warm`): one-time O(snapshot) load,
    /// subsequent calls hit the cache. On load error the caller transparently
    /// falls back to its PG path.
    pub(crate) fn ensure_ram_snapshot_warm(&self, project_code: &str) -> bool {
        if project_code.is_empty() {
            return false;
        }
        if crate::ist_snapshot::process_view().is_warm(project_code) {
            return true;
        }
        let adapter = GraphStoreSqlAdapter {
            inner: &self.graph_store,
        };
        if let Ok((graph, _stats)) = load_snapshot(&adapter, project_code) {
            publish_process_snapshot(project_code.to_string(), Arc::new(graph));
        }
        crate::ist_snapshot::process_view().is_warm(project_code)
    }

    pub(crate) fn axon_ist_snapshot_warm(&self, args: &Value) -> Option<Value> {
        let project_arg = args.get("project_code").and_then(|v| v.as_str());
        let resolved = match project_arg {
            Some(code) => match self.resolve_project_code_value(code) {
                Some(c) => c,
                None => {
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!("ist_snapshot_warm: unknown project_code '{}'", code)
                        }],
                        "isError": true,
                        "data": {
                            "status": "wrong_project_scope",
                            "parameter_repair": {
                                "invalid_field": "project_code",
                                "tool": "ist_snapshot_warm",
                                "follow_up_tools": ["project_registry_lookup"],
                                "hint": "supply a canonical 3-letter project code (e.g. AXO)"
                            }
                        }
                    }))
                }
            },
            None => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": "ist_snapshot_warm requires project_code (canonical 3-letter, e.g. AXO)."
                    }],
                    "isError": true,
                    "data": {
                        "status": "missing_project_code",
                        "parameter_repair": {
                            "invalid_field": "project_code",
                            "tool": "ist_snapshot_warm",
                            "follow_up_tools": ["project_registry_lookup", "help"]
                        }
                    }
                }))
            }
        };

        let adapter = GraphStoreSqlAdapter {
            inner: &self.graph_store,
        };
        let (graph, stats) = match load_snapshot(&adapter, &resolved) {
            Ok(g) => g,
            Err(e) => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("ist_snapshot_warm load failed: {}", e)
                    }],
                    "isError": true,
                    "data": {
                        "status": "load_failed",
                        "diagnostic_excerpt": e.chars().take(240).collect::<String>(),
                        "parameter_repair": {
                            "invalid_field": "ist_snapshot_load",
                            "tool": "ist_snapshot_warm",
                            "follow_up_tools": ["status", "diagnose_indexing"],
                            "hint": "check `status` / `diagnose_indexing` for upstream IST state"
                        }
                    }
                }))
            }
        };
        publish_process_snapshot(resolved.clone(), Arc::new(graph));

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "ist_snapshot_warm ok for {} : {} nodes, {} edges, {:.1} MB, {} ms.",
                    resolved,
                    stats.nodes_loaded,
                    stats.edges_loaded,
                    (stats.approximate_bytes as f64) / (1024.0 * 1024.0),
                    stats.load_ms
                )
            }],
            "data": {
                "status": "ok",
                "project_code": resolved,
                "nodes_loaded": stats.nodes_loaded,
                "edges_loaded": stats.edges_loaded,
                "approximate_bytes": stats.approximate_bytes,
                "load_ms": stats.load_ms,
                "ram_enabled": crate::ist_snapshot::IstSnapshotCache::is_enabled(),
                "operator_guidance": {
                    "problem_class": "none",
                    "follow_up_tools": ["status", "impact", "retrieve_context"],
                    "next_action": "RAM fast-path now active for this project ; impact / collect_structural_neighbors / get_circular_dependency_count_fast will dispatch to CSR until eviction"
                }
            }
        }))
    }

    fn resolve_project_code_value(&self, code: &str) -> Option<String> {
        self.resolve_project_code(code).ok()
    }
}
