// REQ-AXO-91485 (MIL-AXO-019 slice 1) — in-memory IST snapshot.
//
// CSR forward + reverse adjacency for IST edges (CONTAINS / CALLS / CALLS_NIF),
// loaded once per project from ist.symbol + ist.edge and held under an
// ArcSwap cache so MCP tools can traverse the graph without per-call SQL.
// Sync to live data (LISTEN/NOTIFY + incremental patches) lives in
// REQ-AXO-91487 ; this module ships only the cold-load + lookup path.

pub mod algorithms;
pub mod cache;
pub mod code_smells;
pub mod loader;
pub mod notify_listener;
pub mod snapshot;
pub mod structural_invariants;
pub mod view;

pub use cache::IstSnapshotCache;
pub use loader::{load_snapshot, LoadStats};
pub use snapshot::{IstGraph, NodeFlags, NodeKind, RelationType};
pub use view::IstGraphView;

use std::sync::{Arc, OnceLock};

/// REQ-AXO-91486 — process-level cache so any call-site can share the same
/// IstGraph snapshots without plumbing it through McpServer / GraphStore
/// constructors. Lazy-initialised on first access ; cheap (an empty
/// `ArcSwap`) so the cost is paid only when the call-site asks for it.
fn process_cache() -> &'static Arc<IstSnapshotCache> {
    static CACHE: OnceLock<Arc<IstSnapshotCache>> = OnceLock::new();
    CACHE.get_or_init(|| Arc::new(IstSnapshotCache::new()))
}

/// REQ-AXO-91486 — caller-facing handle. Clones are cheap. Use this from
/// any module that needs RAM-first / PG-fallback dispatch on IST queries.
pub fn process_view() -> IstGraphView {
    IstGraphView::new(Arc::clone(process_cache()))
}

/// REQ-AXO-91486 — populate (or refresh) the process cache for a project.
/// Idempotent ; replaces the existing snapshot atomically via ArcSwap.
pub fn publish_process_snapshot(project_code: String, snapshot: Arc<IstGraph>) {
    process_cache().publish(project_code, snapshot);
}

/// REQ-AXO-91486 — evict a project from the process cache (used by tests
/// and for genuine project removal). NB: the `ist_mutated` listener no longer
/// evicts on mutation — see `refresh_process_snapshot` (serve-stale).
pub fn evict_process_snapshot(project_code: &str) {
    process_cache().evict(project_code);
}

/// REQ-AXO-902005 — serve-stale refresh on `ist_mutated`. Instead of evicting
/// (which forced the next reader to pay a synchronous full cold-load on the
/// hot path, or surfaced a degraded cold cache), this KEEPS serving the
/// current snapshot and rebuilds asynchronously: on success the fresh CSR graph
/// is swapped in atomically (ArcSwap); on failure the stale snapshot is
/// retained (never a regression to cold). Single-flight + dirty-bit coalescing
/// via the cache coordinator: concurrent mutations during a rebuild trigger
/// exactly one re-run, never a thundering herd. Readers never block — at worst
/// they see slightly-stale data, which the IST freshness contract already
/// tolerates (CPT-AXO-029). `store` is a cheap `JsonSqlStore` handle (Arc over
/// the GraphStore adapter) so the loader stays decoupled from `GraphStore`.
pub fn refresh_process_snapshot(
    project_code: String,
    store: Arc<dyn loader::JsonSqlStore + Send + Sync>,
) {
    refresh_snapshot_into(Arc::clone(process_cache()), project_code, store);
}

/// REQ-AXO-902005 — cache-explicit core of `refresh_process_snapshot`, so the
/// serve-stale + single-flight behaviour is integration-testable against a
/// local cache + fake store without touching the process-global cache.
fn refresh_snapshot_into(
    cache: Arc<IstSnapshotCache>,
    project_code: String,
    store: Arc<dyn loader::JsonSqlStore + Send + Sync>,
) {
    // Lose the race? The in-flight rebuild was marked dirty and will re-run.
    if !cache.begin_rebuild(&project_code) {
        return;
    }
    tokio::spawn(async move {
        loop {
            let load_store = Arc::clone(&store);
            let load_project = project_code.clone();
            // Blocking SQL load off the async runtime; the stale snapshot keeps
            // serving readers throughout.
            let loaded = tokio::task::spawn_blocking(move || {
                load_snapshot(load_store.as_ref(), &load_project)
            })
            .await;
            match loaded {
                Ok(Ok((graph, stats))) => {
                    // Atomic swap — never a transient None for readers.
                    cache.publish(project_code.clone(), Arc::new(graph));
                    tracing::info!(
                        project = %project_code,
                        nodes = stats.nodes_loaded,
                        edges = stats.edges_loaded,
                        "REQ-AXO-902005: IST snapshot refreshed async (serve-stale, no read-path block)"
                    );
                }
                Ok(Err(err)) => tracing::warn!(
                    project = %project_code,
                    error = %err,
                    "REQ-AXO-902005: async IST refresh failed; retaining stale snapshot"
                ),
                Err(join_err) => tracing::warn!(
                    project = %project_code,
                    error = %join_err,
                    "REQ-AXO-902005: async IST refresh task panicked; retaining stale snapshot"
                ),
            }
            // Re-run iff a mutation landed mid-rebuild; else clear in_flight.
            if !cache.finish_rebuild(&project_code) {
                break;
            }
        }
    });
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use crate::ist_snapshot::snapshot::{EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord};

    /// Fake `JsonSqlStore` returning a fixed 2-node / 1-edge AXO graph,
    /// matching `loader::load_snapshot`'s NODE_SQL / EDGE_SQL row shapes.
    struct FakeStore;
    impl loader::JsonSqlStore for FakeStore {
        fn query_json(&self, sql: &str) -> Result<String, String> {
            if sql.contains("ist.symbol") {
                // id, kind, project_code, tested, is_public, is_nif, is_unsafe, name
                Ok(r#"[["AXO::x","function","AXO","false","true","false","false","x"],
                       ["AXO::y","function","AXO","false","true","false","false","y"]]"#
                    .to_string())
            } else {
                // source_id, target_id, relation_type
                Ok(r#"[["AXO::x","AXO::y","CALLS"]]"#.to_string())
            }
        }
    }

    fn one_node_graph() -> Arc<IstGraph> {
        Arc::new(IstGraph::build(
            vec![NodeRecord {
                id: "AXO::stale".to_string(),
                name: "stale".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
            }],
            vec![] as Vec<EdgeTriple>,
        ))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refresh_serves_stale_then_swaps_without_ever_going_cold() {
        let cache = Arc::new(IstSnapshotCache::new());
        // Warm with the stale 1-node graph.
        cache.publish("AXO".to_string(), one_node_graph());
        assert_eq!(cache.get("AXO").unwrap().node_count(), 1);

        refresh_snapshot_into(Arc::clone(&cache), "AXO".to_string(), Arc::new(FakeStore));

        // Poll until the async rebuild swaps in the fresh 2-node graph. The cache
        // must NEVER be cold (None) at any observation — the serve-stale invariant.
        let mut swapped = false;
        for _ in 0..200 {
            let snap = cache.get("AXO");
            assert!(snap.is_some(), "REQ-AXO-902005: cache must never go cold during refresh");
            if snap.unwrap().node_count() == 2 {
                swapped = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(swapped, "async refresh should swap in the fresh snapshot");
        // in_flight cleared after a clean finish → a new refresh can start.
        assert!(cache.begin_rebuild("AXO"), "rebuild slot freed after refresh");
    }
}
