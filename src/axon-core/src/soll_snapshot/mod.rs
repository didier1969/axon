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

use std::sync::atomic::{AtomicU64, Ordering};

// REQ-AXO-902039 element 3 — fused-retrieval-lane RAM coverage, process-global.
//
// Distinct from `SollSnapshotCache::read_stats()`, which counts whole-snapshot
// cache warmth across EVERY tool that calls `snapshot()` (admin reporting tools
// included). DEC-AXO-901646 flagged that conflation as a "faux signal": the
// headline coverage figure must measure the WHY/retrieve_context fusion lane
// specifically — the symbol→governing-intent structural reads — RAM-served vs
// PG-fallback. These two counters are incremented only on that lane (see
// `tools_context.rs::{resolve_scoped_symbol_id_canonical, has_direct_soll_
// traceability, collect_soll_entities}`), so the ratio reflects how much of the
// fusion substrate is actually served from RAM (PIL-AXO-9002 invariant: RAM
// mirror primary, PG fallback explicit and measured, never silent).
static FUSION_RAM_READS: AtomicU64 = AtomicU64::new(0);
static FUSION_PG_READS: AtomicU64 = AtomicU64::new(0);

/// Record one fused-retrieval-lane SOLL/IST structural read: `ram=true` when it
/// was served from a RAM snapshot, `ram=false` when it fell back to a PG SELECT
/// (project unscoped, snapshot cold, or a column not mirrored in RAM).
pub fn record_fusion_read(ram: bool) {
    if ram {
        FUSION_RAM_READS.fetch_add(1, Ordering::Relaxed);
    } else {
        FUSION_PG_READS.fetch_add(1, Ordering::Relaxed);
    }
}

/// `(ram_reads, pg_reads)` on the fused retrieval lane since process start.
pub fn fusion_read_stats() -> (u64, u64) {
    (
        FUSION_RAM_READS.load(Ordering::Relaxed),
        FUSION_PG_READS.load(Ordering::Relaxed),
    )
}
