// REQ-AXO-91485 / DEC-AXO-097 — IstSnapshotCache.
//
// One ArcSwap per process holds the per-project snapshots. Readers grab the
// current Arc<HashMap<project_code, Arc<IstGraph>>> lock-free ; writers
// publish a new map atomically when a load lands. REQ-AXO-901952 made the
// RAM snapshot the SINGLE source for structural graph queries — the former
// `AXON_IST_RAM_ENABLED` client opt-out toggle is removed (RAM unconditional).

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::ist_snapshot::snapshot::IstGraph;

/// Atomic per-project snapshot cache. Cloning the cache handle is cheap (one
/// `Arc` clone) ; the snapshots themselves never move once published.
pub struct IstSnapshotCache {
    inner: Arc<ArcSwap<HashMap<String, Arc<IstGraph>>>>,
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
        }
    }

    pub fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
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
}
