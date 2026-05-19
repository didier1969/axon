// REQ-AXO-91486 — IstGraphView dispatcher.
//
// Façade over IstSnapshotCache. Callers invoke the same method ; the view
// returns RAM results when the cache holds a snapshot for the project AND
// AXON_IST_RAM_ENABLED is on. Otherwise the view returns None and the
// caller falls back to its existing PG path (preserving GraphProjection
// CTE behaviour per REQ-AXO-91486 invariants).
//
// `freshness_lag_ms` gating (LISTEN/NOTIFY threshold) lives in slice 3 ;
// the v1 view trusts the cache contents as fresh-enough until slice 3
// ships, so the only gates are AXON_IST_RAM_ENABLED + cache presence.

use std::sync::Arc;

use crate::ist_snapshot::cache::IstSnapshotCache;
use crate::ist_snapshot::code_smells;
use crate::ist_snapshot::snapshot::{IstGraph, RelationType};

pub struct IstGraphView {
    cache: Arc<IstSnapshotCache>,
}

impl IstGraphView {
    pub fn new(cache: Arc<IstSnapshotCache>) -> Self {
        Self { cache }
    }

    fn try_snapshot(&self, project: &str) -> Option<Arc<IstGraph>> {
        if !IstSnapshotCache::is_enabled() {
            return None;
        }
        self.cache.get(project)
    }

    /// REQ-AXO-91486 — Forward-radius reach. `None` ⇒ caller falls back to
    /// PG. `Some(vec)` ⇒ canonical ids reachable from `source_id` within
    /// `max_radius`, capped at `max_neighbors`. Pass an empty `rel_filter`
    /// to traverse every relation_type.
    pub fn forward_at_radius(
        &self,
        project: &str,
        source_id: &str,
        max_radius: u32,
        max_neighbors: usize,
        rel_filter: &[RelationType],
    ) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(snap.bfs_forward(source_id, max_radius, max_neighbors, rel_filter))
    }

    /// REQ-AXO-91486 — Reverse-radius reach (callers / ancestors).
    pub fn reverse_at_radius(
        &self,
        project: &str,
        source_id: &str,
        max_radius: u32,
        max_neighbors: usize,
        rel_filter: &[RelationType],
    ) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(snap.bfs_reverse(source_id, max_radius, max_neighbors, rel_filter))
    }

    /// REQ-AXO-91512 — Relation type of a direct edge source → target,
    /// when one exists in the snapshot. Used by `impact` to break down
    /// caller counts by edge kind (CALLS / CALLS_NIF) without touching
    /// PG. Returns `None` when no such direct edge exists or the cache
    /// is cold.
    pub fn direct_edge_relation(
        &self,
        project: &str,
        source_id: &str,
        target_id: &str,
    ) -> Option<RelationType> {
        let snap = self.try_snapshot(project)?;
        let source_idx = snap.index_of(source_id)?;
        let target_idx = snap.index_of(target_id)?;
        for (idx, rel) in snap.forward_neighbors(source_idx) {
            if idx == target_idx {
                return Some(rel);
            }
        }
        None
    }

    /// REQ-AXO-91510 — RAM shortest path source→sink. `None` ⇒ caller
    /// falls back to PG (`public.path` SQL). `Some((names, rels))` ⇒
    /// canonical names along the shortest path, with relation_type per
    /// node (placeholder `Calls` for the source slot — see snapshot.rs).
    pub fn shortest_path(
        &self,
        project: &str,
        source_id: &str,
        sink_id: &str,
        max_depth: u32,
        rel_filter: &[RelationType],
    ) -> Option<(Vec<String>, Vec<RelationType>)> {
        let snap = self.try_snapshot(project)?;
        snap.bfs_shortest_path(source_id, sink_id, max_depth, rel_filter)
    }

    /// REQ-AXO-91486 — Reciprocal CALLS cycle count (A↔B pairs). Migrates
    /// `get_circular_dependency_count_fast` from a SQL self-join to an
    /// in-memory linear scan.
    pub fn reciprocal_calls_cycle_count(&self, project: &str) -> Option<usize> {
        let snap = self.try_snapshot(project)?;
        Some(snap.reciprocal_calls_cycle_count())
    }

    pub fn is_warm(&self, project: &str) -> bool {
        IstSnapshotCache::is_enabled() && self.cache.get(project).is_some()
    }

    pub fn cache_handle(&self) -> Arc<IstSnapshotCache> {
        Arc::clone(&self.cache)
    }

    /// REQ-AXO-901595 — RAM wrapper candidates. `None` ⇒ caller falls back
    /// to `GraphStore::get_wrapper_candidates`. Result format mirrors the
    /// PG path : `"source_name -> target_name"`.
    pub fn wrapper_candidates(
        &self,
        project: &str,
        limit: usize,
    ) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::wrapper_candidates(&snap, project, limit))
    }

    /// REQ-AXO-901595 — RAM feature-envy candidates. `None` ⇒ caller falls
    /// back to `GraphStore::get_feature_envy_candidates`. Result format
    /// mirrors the PG path : `"source -> dominant_foreign_path (foreign/total)"`.
    pub fn feature_envy_candidates(
        &self,
        project: &str,
        limit: usize,
    ) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::feature_envy_candidates(&snap, project, limit))
    }

    /// REQ-AXO-901595 — RAM god-object candidates (fan-in ≥ 20). `None` ⇒
    /// caller falls back to `GraphStore::get_god_objects`. Returns the same
    /// `(name, fan_in)` pairs the PG path produces, sorted by fan_in desc
    /// then name asc.
    pub fn god_objects(
        &self,
        project: &str,
    ) -> Option<Vec<(String, usize)>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::god_objects(&snap, project))
    }

    /// REQ-AXO-901595 — RAM structural orphan_code (no callers, non-public,
    /// non-test path). Strict superset of `GraphStore::get_orphan_code_symbols`
    /// which additionally excludes symbols carrying a soll.Traceability
    /// link — that filter requires SOLL state outside the IstGraph, so
    /// callers requiring the canonical orphan_code set must keep the PG
    /// path.
    pub fn orphan_code_symbols(
        &self,
        project: &str,
        limit: usize,
    ) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::orphan_code_symbols(&snap, project, limit))
    }

    /// REQ-AXO-901596 — RAM lexical match over symbol names. Implements the
    /// same fuzzy predicate as `tools_dx.rs::symbol_search_predicate`
    /// (substring + separator-normalised + wildcard + compact). Returns
    /// `(name, kind_str, file_path)` triples.
    pub fn lexical_symbol_search(
        &self,
        project: &str,
        query_text: &str,
        limit: usize,
    ) -> Option<Vec<(String, &'static str, String)>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::lexical_symbol_search(&snap, project, query_text, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord};

    fn three_node_call_graph() -> Arc<IstGraph> {
        let nodes = vec![
            NodeRecord {
                id: "AXO::a".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
            },
            NodeRecord {
                id: "AXO::b".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
            },
            NodeRecord {
                id: "AXO::c".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
            },
        ];
        let edges = vec![
            EdgeTriple {
                source: "AXO::a".to_string(),
                target: "AXO::b".to_string(),
                rel: RelationType::Calls,
            },
            EdgeTriple {
                source: "AXO::b".to_string(),
                target: "AXO::c".to_string(),
                rel: RelationType::Calls,
            },
        ];
        Arc::new(IstGraph::build(nodes, edges))
    }

    fn warm_cache() -> Arc<IstSnapshotCache> {
        let cache = Arc::new(IstSnapshotCache::new());
        cache.publish("AXO".to_string(), three_node_call_graph());
        cache
    }

    #[test]
    fn forward_returns_none_when_disabled_env() {
        std::env::remove_var("AXON_IST_RAM_ENABLED");
        let view = IstGraphView::new(warm_cache());
        assert!(view.forward_at_radius("AXO", "AXO::a", 1, 10, &[]).is_none());
    }

    #[test]
    fn forward_returns_results_when_enabled_and_warm() {
        std::env::set_var("AXON_IST_RAM_ENABLED", "1");
        let view = IstGraphView::new(warm_cache());
        let r = view
            .forward_at_radius("AXO", "AXO::a", 1, 10, &[RelationType::Calls])
            .expect("warm cache should return Some");
        assert_eq!(r, vec!["AXO::b".to_string()]);
        std::env::remove_var("AXON_IST_RAM_ENABLED");
    }

    #[test]
    fn reverse_collects_callers() {
        std::env::set_var("AXON_IST_RAM_ENABLED", "1");
        let view = IstGraphView::new(warm_cache());
        let r = view
            .reverse_at_radius("AXO", "AXO::c", 2, 10, &[RelationType::Calls])
            .expect("warm cache should return Some");
        let set: std::collections::HashSet<&str> = r.iter().map(String::as_str).collect();
        assert!(set.contains("AXO::a"));
        assert!(set.contains("AXO::b"));
        std::env::remove_var("AXON_IST_RAM_ENABLED");
    }

    #[test]
    fn reciprocal_count_zero_for_dag() {
        std::env::set_var("AXON_IST_RAM_ENABLED", "1");
        let view = IstGraphView::new(warm_cache());
        assert_eq!(view.reciprocal_calls_cycle_count("AXO"), Some(0));
        std::env::remove_var("AXON_IST_RAM_ENABLED");
    }

    #[test]
    fn is_warm_returns_false_when_cache_empty() {
        std::env::set_var("AXON_IST_RAM_ENABLED", "1");
        let view = IstGraphView::new(Arc::new(IstSnapshotCache::new()));
        assert!(!view.is_warm("AXO"));
        std::env::remove_var("AXON_IST_RAM_ENABLED");
    }
}
