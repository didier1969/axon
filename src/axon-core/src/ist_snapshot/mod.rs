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
pub mod drift_history;
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
/// Is this symbol id something a reader can actually OPEN?
///
/// REQ-AXO-902440 — the corpus holds entities that must never reach a surface
/// meant for a human or an LLM, and three tools were shipping them raw:
///
/// * `impact`'s Derived Local Projection — measured 2026-08-21 on
///   `current_runtime_tuning_snapshot`: **15 of 19 rows** were unopenable
///   (`fused_L19_28_0` chunker artefacts, `Some`/`new`/`lock`/`min`/`max`
///   language primitives, a bare file path, an id with an empty tail).
///   The 4 rows the caller actually needed were buried in them.
/// * `debt_digest`'s `dry` section — TE2 reported 10 of the 15 "most central
///   duplications" were `fused_L*` (llm_feedback #183). Same entities, other
///   tool: a shared chokepoint, not two bugs.
///
/// This is a PRESENTATION filter, never a measurement filter: counts stay
/// whole, and every caller says how many rows it withheld. Silently shrinking
/// a list reads as "there was nothing else", which is the failure mode this
/// repo keeps paying for.
pub fn symbol_id_is_presentable(id: &str) -> bool {
    // Language and stdlib primitives. `Some` is not a structural neighbour, it
    // is a variant constructor; a neighbour list containing it ranks nothing.
    const PRIMITIVES: &[&str] = &[
        "Some", "None", "Ok", "Err", "new", "default", "clone", "lock", "min", "max", "len",
        "push", "get", "insert", "unwrap", "unwrap_or", "unwrap_or_else", "unwrap_or_default",
        "into_inner", "get_or_insert", "get_or_init", "to_string", "from", "into", "iter",
        "collect", "map", "filter", "next", "as_str", "is_empty", "trim", "format", "println",
    ];
    let Some((_, tail)) = id.rsplit_once("::") else {
        // No `::` at all — this is a file path or a bare token, not a symbol
        // anyone can `inspect`.
        return false;
    };
    let tail = tail.trim();
    !tail.is_empty()
        // Chunker fusion artefacts (`code_chunker.rs` emits
        // `<file>::fused_L<start>_<end>_<seq>`): an indexing internal, never a
        // symbol. They cannot be opened, inspected, or acted on.
        && !tail.starts_with("fused_")
        && !PRIMITIVES.contains(&tail)
}

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
                complexity: None,
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
