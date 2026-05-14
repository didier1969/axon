//! In-memory SOLL graph snapshot (DEC-AXO-091 / REQ-AXO-322).
//!
//! Mirrors the SOLL graph (nodes, edges, traceability) in RAM so that
//! hot read tools (`soll_work_plan`, `soll_verify_requirements`,
//! `soll_completeness_snapshot`, ...) avoid the per-call SQL roundtrip
//! that dominated their latency.
//!
//! PostgreSQL remains the canonical writer and audit log. The snapshot
//! is derived state, refreshed via `SollSnapshotCache::invalidate` after
//! any MCP-side mutation (hooked from the dispatch layer in
//! `mcp/dispatch.rs` via `attach_derived_docs_refresh_metadata`).
//!
//! v1 invalidation is best-effort: cross-process mutations (a second
//! brain instance writing to the same PG) are not seen until the local
//! snapshot is reloaded for another reason. Acceptable since
//! production runs a single live brain per project. v2 will add a
//! `pg_notify('soll_mutated', ...)` listener (planned as a separate
//! slice once revision-based invalidation proves itself in production).

pub mod cache;
pub mod loader;
pub mod snapshot;

pub use cache::SollSnapshotCache;
pub use snapshot::{SnapshotEdge, SnapshotNode, SnapshotTraceability, SollSnapshot};
