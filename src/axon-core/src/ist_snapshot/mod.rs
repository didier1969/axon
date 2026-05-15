// REQ-AXO-91485 (MIL-AXO-019 slice 1) — in-memory IST snapshot.
//
// CSR forward + reverse adjacency for IST edges (CONTAINS / CALLS / CALLS_NIF),
// loaded once per project from public.symbol + public.edge and held under an
// ArcSwap cache so MCP tools can traverse the graph without per-call SQL.
// Sync to live data (LISTEN/NOTIFY + incremental patches) lives in
// REQ-AXO-91487 ; this module ships only the cold-load + lookup path.

pub mod cache;
pub mod loader;
pub mod snapshot;

pub use cache::IstSnapshotCache;
pub use loader::{load_snapshot, LoadStats};
pub use snapshot::{IstGraph, NodeFlags, NodeKind, RelationType};
