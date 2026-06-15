// REQ-AXO-91485 / DEC-AXO-097 — IstSnapshotCache.
//
// One ArcSwap per process holds the per-project snapshots. Readers grab the
// current Arc<HashMap<project_code, Arc<IstGraph>>> lock-free ; writers
// publish a new map atomically when a load lands. REQ-AXO-901952 made the
// RAM snapshot the SINGLE source for structural graph queries — the former
// `AXON_IST_RAM_ENABLED` client opt-out toggle is removed (RAM unconditional).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;

use crate::ist_snapshot::snapshot::IstGraph;

/// REQ-AXO-902005 — per-project rebuild coordination, kept in a side map so the
/// hot snapshot map value stays `Arc<IstGraph>` (zero churn on the read path /
/// view methods). `in_flight`/`dirty` drive single-flight coalescing: while a
/// rebuild runs, a fresh `ist_mutated` sets `dirty` instead of spawning a second
/// loader; the running rebuild re-runs once on finish.
#[derive(Default, Clone, Copy)]
struct ProjectState {
    in_flight: bool,
    dirty: bool,
}

/// Atomic per-project snapshot cache. Cloning the cache handle is cheap (one
/// `Arc` clone) ; the snapshots themselves never move once published.
pub struct IstSnapshotCache {
    inner: Arc<ArcSwap<HashMap<String, Arc<IstGraph>>>>,
    /// REQ-AXO-902005 — rebuild single-flight + freshness, keyed by project.
    state: Arc<Mutex<HashMap<String, ProjectState>>>,
}

impl Default for IstSnapshotCache {
    fn default() -> Self {
        Self::new()
    }
}

impl IstSnapshotCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(HashMap::new()))),
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            state: Arc::clone(&self.state),
        }
    }

    /// REQ-AXO-901952 — the IST RAM snapshot is the SINGLE source for
    /// structural graph queries (operator directive session 77, repeated 5×):
    /// no PG fallback, one query method. The former client opt-out toggle
    /// `AXON_IST_RAM_ENABLED` is removed — RAM is unconditional. Retained as a
    /// status reporter (always `true`) for the `ram_enabled` field surfaced by
    /// the ist_snapshot tools. Supersedes DEC-AXO-097 (IST RAM disable path).
    pub fn is_enabled() -> bool {
        true
    }

    pub fn get(&self, project_code: &str) -> Option<Arc<IstGraph>> {
        self.inner.load().get(project_code).cloned()
    }

    pub fn publish(&self, project_code: String, snapshot: Arc<IstGraph>) {
        let current = self.inner.load();
        let mut next: HashMap<String, Arc<IstGraph>> = (**current).clone();
        next.insert(project_code, snapshot);
        self.inner.store(Arc::new(next));
    }

    pub fn evict(&self, project_code: &str) {
        let current = self.inner.load();
        if !current.contains_key(project_code) {
            return;
        }
        let mut next: HashMap<String, Arc<IstGraph>> = (**current).clone();
        next.remove(project_code);
        self.inner.store(Arc::new(next));
    }

    pub fn project_codes(&self) -> Vec<String> {
        self.inner.load().keys().cloned().collect()
    }

    /// REQ-AXO-902005 — single-flight gate. Returns `true` when the caller wins
    /// the right to rebuild `project` (no rebuild was in flight). Returns
    /// `false` when a rebuild is already running — in that case the request is
    /// recorded as `dirty` so the in-flight rebuild re-runs once on completion,
    /// guaranteeing the snapshot reflects the latest mutation without spawning a
    /// second concurrent loader (no thundering herd).
    pub fn begin_rebuild(&self, project: &str) -> bool {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = guard.entry(project.to_string()).or_default();
        if st.in_flight {
            st.dirty = true;
            false
        } else {
            st.in_flight = true;
            st.dirty = false;
            true
        }
    }

    /// REQ-AXO-902005 — close out a rebuild. Returns `true` when a mutation
    /// landed during the rebuild (`dirty`): the caller must re-run the load to
    /// pick it up; `in_flight` is kept set so no other caller interleaves.
    /// Returns `false` when clean: `in_flight` is cleared.
    pub fn finish_rebuild(&self, project: &str) -> bool {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = guard.entry(project.to_string()).or_default();
        if st.dirty {
            st.dirty = false;
            true
        } else {
            st.in_flight = false;
            false
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ist_snapshot::snapshot::IstGraph;

    fn empty_snapshot() -> Arc<IstGraph> {
        Arc::new(IstGraph::build(vec![], vec![]))
    }

    #[test]
    fn cache_starts_empty() {
        let cache = IstSnapshotCache::new();
        assert!(cache.get("AXO").is_none());
        assert!(cache.project_codes().is_empty());
    }

    #[test]
    fn publish_then_get_returns_snapshot() {
        let cache = IstSnapshotCache::new();
        cache.publish("AXO".to_string(), empty_snapshot());
        assert!(cache.get("AXO").is_some());
        assert_eq!(cache.project_codes(), vec!["AXO".to_string()]);
    }

    #[test]
    fn publish_replaces_existing_project() {
        let cache = IstSnapshotCache::new();
        let first = empty_snapshot();
        let second = empty_snapshot();
        cache.publish("AXO".to_string(), Arc::clone(&first));
        cache.publish("AXO".to_string(), Arc::clone(&second));
        let got = cache.get("AXO").unwrap();
        assert!(Arc::ptr_eq(&got, &second));
        assert!(!Arc::ptr_eq(&got, &first));
    }

    #[test]
    fn evict_removes_project_without_affecting_others() {
        let cache = IstSnapshotCache::new();
        cache.publish("AXO".to_string(), empty_snapshot());
        cache.publish("OPT".to_string(), empty_snapshot());
        cache.evict("AXO");
        assert!(cache.get("AXO").is_none());
        assert!(cache.get("OPT").is_some());
    }

    #[test]
    fn handle_shares_same_arcswap() {
        let cache = IstSnapshotCache::new();
        let handle = cache.handle();
        cache.publish("AXO".to_string(), empty_snapshot());
        assert!(handle.get("AXO").is_some());
    }

    // REQ-AXO-902005 — single-flight coordinator.

    #[test]
    fn first_begin_rebuild_wins_second_marks_dirty() {
        let cache = IstSnapshotCache::new();
        assert!(cache.begin_rebuild("AXO"), "first caller wins the rebuild slot");
        assert!(
            !cache.begin_rebuild("AXO"),
            "second caller loses (rebuild already in flight)"
        );
        // The lost caller recorded dirty → finish must request a re-run.
        assert!(cache.finish_rebuild("AXO"), "dirty after concurrent request → re-run");
        // No further mutation → finish clears in_flight, next begin wins again.
        assert!(!cache.finish_rebuild("AXO"), "clean finish clears in_flight");
        assert!(cache.begin_rebuild("AXO"), "slot freed after clean finish");
    }

    #[test]
    fn rebuild_state_is_per_project() {
        let cache = IstSnapshotCache::new();
        assert!(cache.begin_rebuild("AXO"));
        // A different project is independent — it can start its own rebuild.
        assert!(cache.begin_rebuild("OPT"));
        assert!(!cache.finish_rebuild("AXO"));
        assert!(!cache.finish_rebuild("OPT"));
    }
}
