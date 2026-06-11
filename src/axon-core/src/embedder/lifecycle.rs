//! Embedder lifecycle state (REQ-AXO-90009 Slice 1A, DEC-AXO-086).
//!
//! `EmbedderRuntimeState` is a process-wide set of chunk_ids known to need
//! a fresh embedding. Pipeline A3 marks chunks `pending` pre-commit when
//! it writes new `ist.Chunk` rows, and pipeline B3 marks them
//! `embedded` post-commit after the matching `ist.ChunkEmbedding`
//! row lands. `retrieve_context` consults `pending_subset` to expose a
//! cheap freshness gate without an extra DB round-trip.
//!
//! Slice 1A (this module) ships the in-memory state surface alone — the
//! A3/B3 mark calls, the PG `LISTEN chunk_pending_embed` task, the
//! reconcile loop, and the `EmbedderLifecycle` 2-state sleep/wake
//! machine all build on this primitive in Slice 1B → Slice 3.
//!
//! Concurrency contract :
//! - All mutations and reads go through a `parking_lot::RwLock` so MCP
//!   reads (`retrieve_context` freshness check) don't block A3 writers
//!   under usual ratios.
//! - The set is intentionally a flat `HashSet<String>` of chunk_ids.
//!   Project-code scoping happens upstream — chunk_ids are globally
//!   unique already (`{project_code}::{symbol_id}::{chunk_idx}`).
//!
//! Boot hydration :
//! - `hydrate_from_db_rows` rebuilds the set from a caller-supplied row
//!   iterator (chunk_ids that have NO matching ChunkEmbedding row, OR a
//!   stale `source_hash`). Keeping the DB query out of this module lets
//!   it unit-test without a live PG ; the orchestrator wires the actual
//!   `LEFT JOIN ChunkEmbedding` query.
//!
//! Invariants :
//! - `mark_pending` is idempotent (no-op on already-pending).
//! - `mark_embedded` is idempotent (no-op when chunk isn't pending).
//! - `pending_subset` returns the intersection of caller candidates and
//!   the pending set ; never a superset, never a copy of the whole set.

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

#[derive(Default)]
pub struct EmbedderRuntimeState {
    pending: RwLock<HashSet<String>>,
}

/// Process-level singleton matching the pattern used by
/// [`crate::ist_snapshot::process_cache`] : a lazy `OnceLock` so any
/// call-site (A3 pre-commit, B3 post-commit, MCP `retrieve_context`,
/// future LISTEN supervisor) can share the same state without plumbing
/// it through every constructor. Clones of the `Arc` are cheap.
pub fn process_state() -> &'static Arc<EmbedderRuntimeState> {
    static STATE: OnceLock<Arc<EmbedderRuntimeState>> = OnceLock::new();
    STATE.get_or_init(|| Arc::new(EmbedderRuntimeState::new()))
}

impl EmbedderRuntimeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the pending set wholesale with the DB-authoritative truth.
    ///
    /// Used at boot AND by the periodic reconcile loop (REQ-AXO-901931).
    /// The argument is the result of the canonical orphan query
    /// (`SELECT chunk_id FROM Chunk WHERE NOT EXISTS (matching
    /// ChunkEmbedding)`) — i.e. the complete set of chunks that genuinely
    /// still need an embedding. Replacing wholesale (rather than the
    /// historical additive-only union) is what makes the pending set
    /// **self-healing**: any chunk_id marked `pending` but never
    /// `mark_embedded` (dedup-skip on the B lane, unchanged re-chunk,
    /// dropped handoff, …) is purged on the next reconcile tick instead
    /// of poisoning `EmbedderLifecycle::should_sleep` forever and pinning
    /// VRAM at idle.
    ///
    /// Safety of wholesale replace: the in-memory set drives ONLY
    /// `should_sleep` and `retrieve_context` freshness — NOT the embed
    /// pipeline (demand_pull_b reads pending chunks from PG directly). A
    /// chunk marked pending by A3 pre-commit but not yet committed when a
    /// reconcile fires is transiently absent from the freshness envelope
    /// for at most one reconcile interval; it is still embedded by the B
    /// lane and reappears in the next orphan query. No embedding is lost.
    pub fn hydrate_from_db_rows<I>(&self, rows: I)
    where
        I: IntoIterator<Item = String>,
    {
        let new_set: HashSet<String> = rows.into_iter().collect();
        *self.pending.write() = new_set;
    }

    /// Idempotent. Called by A3 pre-commit when a new or content-changed
    /// `ist.Chunk` row is about to be written.
    pub fn mark_pending(&self, chunk_id: impl Into<String>) {
        self.pending.write().insert(chunk_id.into());
    }

    /// Idempotent. Called by B3 post-commit after `ist.ChunkEmbedding`
    /// INSERT succeeds. A chunk_id absent from the set is a no-op (the
    /// reconcile loop or the LISTEN task may have already cleared it).
    pub fn mark_embedded(&self, chunk_id: &str) {
        self.pending.write().remove(chunk_id);
    }

    /// Return `true` when no chunks are pending. Used by Slice 3's
    /// `EmbedderLifecycle` to decide whether the GPU session can be
    /// dropped on `T_idle` expiry.
    pub fn is_empty(&self) -> bool {
        self.pending.read().is_empty()
    }

    /// Snapshot of the pending count. Heartbeat / `embedding_status` use
    /// this for the operator-visible backlog metric (Slice 2).
    pub fn pending_count(&self) -> usize {
        self.pending.read().len()
    }

    /// Intersection of `candidates` and the pending set. Designed for
    /// `retrieve_context` freshness : the caller passes the chunk_ids
    /// it would return, gets back the subset still waiting on an
    /// embedding update. Result preserves the caller's input order so
    /// callers can render alongside their original ranking.
    #[allow(dead_code)]
    pub fn pending_subset(&self, candidates: &[String]) -> Vec<String> {
        let pending = self.pending.read();
        candidates
            .iter()
            .filter(|cid| pending.contains(cid.as_str()))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_starts_empty() {
        let state = EmbedderRuntimeState::new();
        assert!(state.is_empty());
        assert_eq!(state.pending_count(), 0);
    }

    #[test]
    fn mark_pending_then_mark_embedded_roundtrip() {
        let state = EmbedderRuntimeState::new();
        state.mark_pending("chunk-a");
        assert!(!state.is_empty());
        assert_eq!(state.pending_count(), 1);
        state.mark_embedded("chunk-a");
        assert!(state.is_empty());
    }

    #[test]
    fn mark_pending_is_idempotent() {
        let state = EmbedderRuntimeState::new();
        state.mark_pending("c");
        state.mark_pending("c");
        state.mark_pending("c");
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn mark_embedded_on_absent_chunk_is_a_no_op() {
        let state = EmbedderRuntimeState::new();
        state.mark_embedded("never-pending");
        assert_eq!(state.pending_count(), 0);
    }

    #[test]
    fn hydrate_from_db_rows_replaces_set_wholesale() {
        let state = EmbedderRuntimeState::new();
        state.mark_pending("legacy");
        state.hydrate_from_db_rows(["fresh-1".to_string(), "fresh-2".to_string()]);
        assert_eq!(state.pending_count(), 2);
        // Legacy entry is dropped — hydrate is canonical wholesale.
        assert!(state.pending_subset(&["legacy".into()]).is_empty());
        assert_eq!(
            state.pending_subset(&["fresh-1".into(), "missing".into()]),
            vec!["fresh-1".to_string()]
        );
    }

    #[test]
    fn reconcile_purges_phantom_pending_against_empty_db_truth() {
        // REQ-AXO-901931 regression — 200 chunks marked pending but never
        // mark_embedded (dedup-skip / unchanged re-chunk) poison the set.
        // DB coverage is 100% (orphan query returns empty) → a wholesale
        // reconcile MUST drain them so should_sleep can fire and VRAM is
        // released. The historical additive-only reconcile left them
        // forever, pinning the GPU session at idle.
        let state = EmbedderRuntimeState::new();
        for i in 0..200 {
            state.mark_pending(format!("phantom-{i}"));
        }
        assert_eq!(state.pending_count(), 200);
        assert!(!state.is_empty());
        // DB says nothing is orphaned → wholesale replace with empty truth.
        state.hydrate_from_db_rows(Vec::<String>::new());
        assert_eq!(state.pending_count(), 0);
        assert!(state.is_empty(), "phantom pending must be purged so should_sleep can fire");
    }

    #[test]
    fn reconcile_keeps_genuine_db_pending_and_drops_only_phantoms() {
        // Mixed case: some entries are genuine DB orphans (must survive),
        // others are leaked phantoms (must be dropped).
        let state = EmbedderRuntimeState::new();
        state.mark_pending("phantom-leaked");
        state.mark_pending("genuine-1");
        // DB truth: only genuine-1 and a freshly-discovered genuine-2.
        state.hydrate_from_db_rows(["genuine-1".to_string(), "genuine-2".to_string()]);
        assert_eq!(state.pending_count(), 2);
        assert!(state.pending_subset(&["phantom-leaked".into()]).is_empty());
        assert_eq!(
            state.pending_subset(&["genuine-1".into(), "genuine-2".into()]).len(),
            2
        );
    }

    #[test]
    fn pending_subset_preserves_caller_input_order() {
        let state = EmbedderRuntimeState::new();
        state.mark_pending("b");
        state.mark_pending("a");
        state.mark_pending("c");
        let result = state.pending_subset(&[
            "x".into(),
            "a".into(),
            "y".into(),
            "b".into(),
            "c".into(),
        ]);
        assert_eq!(result, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn concurrent_mark_pending_and_pending_count_are_safe() {
        use std::sync::Arc;
        use std::thread;
        let state = Arc::new(EmbedderRuntimeState::new());
        let mut handles = Vec::new();
        for i in 0..16 {
            let s = state.clone();
            handles.push(thread::spawn(move || {
                for j in 0..100 {
                    s.mark_pending(format!("t{i}-c{j}"));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(state.pending_count(), 16 * 100);
    }
}
