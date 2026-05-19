// REQ-AXO-91485 (MIL-AXO-019 slice 1) — in-memory IST snapshot.
//
// CSR forward + reverse adjacency for IST edges (CONTAINS / CALLS / CALLS_NIF),
// loaded once per project from public.symbol + public.edge and held under an
// ArcSwap cache so MCP tools can traverse the graph without per-call SQL.
// Sync to live data (LISTEN/NOTIFY + incremental patches) lives in
// REQ-AXO-91487 ; this module ships only the cold-load + lookup path.

pub mod algorithms;
pub mod cache;
pub mod code_smells;
pub mod loader;
pub mod notify_listener;
pub mod snapshot;
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
/// and by slice 3 LISTEN/NOTIFY when an indexed_file is deleted).
pub fn evict_process_snapshot(project_code: &str) {
    process_cache().evict(project_code);
}
