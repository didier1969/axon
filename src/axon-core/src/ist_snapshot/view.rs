// REQ-AXO-91486 — IstGraphView dispatcher.
//
// Façade over IstSnapshotCache. Callers invoke the same method ; the view
// returns RAM results when the cache holds a snapshot for the project.
// REQ-AXO-901952 removed the `AXON_IST_RAM_ENABLED` opt-out, so the only
// gate is cache presence — a cold cache returns None and the caller must
// surface a loud degraded error (never a silent 0 / PG fallback).
//
// `freshness_lag_ms` gating (LISTEN/NOTIFY threshold) lives in slice 3 ;
// the v1 view trusts the cache contents as fresh-enough until slice 3 ships.

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
        // REQ-AXO-901952 — RAM is unconditional ; the only gate is cache
        // presence. A cold cache returns None → caller surfaces a loud
        // degraded error.
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
    /// falls back to PG (`ist.path` SQL). `Some((names, rels))` ⇒
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

    /// REQ-AXO-902019 — up to `max_paths` node-disjoint routes source→sink. The
    /// first is the shortest path; the rest are independent alternates whose
    /// count is the redundancy/multiplicity signal. `None` ⇒ cold cache.
    pub fn disjoint_paths(
        &self,
        project: &str,
        source_id: &str,
        sink_id: &str,
        max_depth: u32,
        rel_filter: &[RelationType],
        max_paths: usize,
    ) -> Option<Vec<(Vec<String>, Vec<RelationType>)>> {
        let snap = self.try_snapshot(project)?;
        Some(snap.bfs_disjoint_paths(source_id, sink_id, max_depth, rel_filter, max_paths))
    }

    /// REQ-AXO-91486 — Reciprocal CALLS cycle count (A↔B pairs). Migrates
    /// `get_circular_dependency_count_fast` from a SQL self-join to an
    /// in-memory linear scan.
    pub fn reciprocal_calls_cycle_count(&self, project: &str) -> Option<usize> {
        let snap = self.try_snapshot(project)?;
        Some(snap.reciprocal_calls_cycle_count())
    }

    /// REQ-AXO-901952 — Tarjan strongly-connected components (size > 1 = a
    /// circular-dependency cluster), ordered by descending size. Migrates the
    /// `get_circular_dependencies` cycle LISTING off the PG `WITH RECURSIVE`
    /// path-enumeration onto the RAM snapshot. `None` ⇒ cold cache (caller
    /// surfaces empty, never a PG fallback — the count stays the RAM heartbeat).
    pub fn structural_sccs(&self, project: &str) -> Option<Vec<Vec<String>>> {
        let snap = self.try_snapshot(project)?;
        Some(crate::ist_snapshot::algorithms::structural_sccs(&snap))
    }

    /// REQ-AXO-901952 (gap B) — coverage `tested` flag from RAM NodeFlags
    /// (the loader carries `tested::text` since REQ-AXO-91485). Migrates
    /// `change_safety` off the `SELECT … FROM Symbol … tested=true` PG count.
    /// `None` ⇒ cold cache OR symbol absent from the snapshot (caller treats
    /// as not-tested, the conservative change-safety default).
    pub fn node_tested(&self, project: &str, symbol_id: &str) -> Option<bool> {
        let snap = self.try_snapshot(project)?;
        let idx = snap.index_of(symbol_id)?;
        let (_, _, flags) = snap.node_meta(idx);
        Some(flags.tested())
    }

    /// REQ-AXO-901970 — canonical ids whose short name matches `name`, owned
    /// copies. `None` ⇒ cold cache (caller surfaces empty, never PG).
    pub fn ids_for_short_name(&self, project: &str, name: &str) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(
            snap.ids_with_short_name(name)
                .into_iter()
                .map(str::to_string)
                .collect(),
        )
    }

    /// REQ-AXO-901970 — canonical lowercase kind string of a symbol id (matches
    /// the `ist.symbol.kind` column / `COALESCE(s.kind,'')`). `None` ⇒ cold
    /// cache or id absent.
    pub fn node_kind_db(&self, project: &str, id: &str) -> Option<&'static str> {
        let snap = self.try_snapshot(project)?;
        snap.node_kind(id).map(|k| k.as_db())
    }

    /// REQ-AXO-901970 — symbol ids whose containing file name matches the query
    /// (wildcard or substring). `None` ⇒ cold cache. Backs `query`'s chunk-search
    /// file-name `path_match` without a PG EXISTS(CONTAINS) subquery.
    pub fn symbols_in_matching_files(
        &self,
        project: &str,
        normalized: &str,
        wildcard: &str,
    ) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::symbols_in_matching_files(
            &snap, project, normalized, wildcard,
        ))
    }

    /// REQ-AXO-901970 — RAM anomalies/audit sub-checks. `None` ⇒ cold cache.
    pub fn detour_candidates(&self, project: &str, limit: usize) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::detour_candidates(&snap, project, limit))
    }

    pub fn abstraction_detour_candidates(
        &self,
        project: &str,
        limit: usize,
    ) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::abstraction_detour_candidates(
            &snap, project, limit,
        ))
    }

    pub fn domain_leakage(&self, project: &str, domain: &str, infra: &str) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::domain_leakage(&snap, project, domain, infra))
    }

    pub fn dead_code_count(&self, project: &str) -> Option<usize> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::dead_code_count(&snap, project))
    }

    pub fn phantom_dead_refs(&self, project: &str) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::phantom_dead_refs(&snap, project))
    }

    pub fn phantom_multi_declare(&self, project: &str) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::phantom_multi_declare(&snap, project))
    }

    /// REQ-AXO-901970 — RAM audit-score sub-checks. `None` ⇒ cold cache.
    pub fn security_audit_paths(&self, project: &str) -> Option<Vec<(String, String)>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::security_audit_paths(&snap, project))
    }

    pub fn technical_debt(&self, project: &str) -> Option<Vec<(String, String)>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::technical_debt(&snap, project))
    }

    pub fn telemetry_log_call_count(&self, project: &str) -> Option<usize> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::telemetry_log_call_count(&snap, project))
    }

    /// REQ-AXO-901970 — count edges of the given relation types in the project
    /// snapshot. `None` ⇒ cold cache (caller surfaces 0, never a PG count).
    pub fn count_edges_with_relation(
        &self,
        project: &str,
        rels: &[crate::ist_snapshot::snapshot::RelationType],
    ) -> Option<usize> {
        let snap = self.try_snapshot(project)?;
        Some(snap.count_edges_with_relation(rels))
    }

    pub fn is_warm(&self, project: &str) -> bool {
        self.cache.get(project).is_some()
    }

    pub fn cache_handle(&self) -> Arc<IstSnapshotCache> {
        Arc::clone(&self.cache)
    }

    /// REQ-AXO-901595 — RAM wrapper candidates. `None` ⇒ caller falls back
    /// to `GraphStore::get_wrapper_candidates`. Result format mirrors the
    /// PG path : `"source_name -> target_name"`.
    pub fn wrapper_candidates(&self, project: &str, limit: usize) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::wrapper_candidates(&snap, project, limit))
    }

    /// REQ-AXO-901595 — RAM feature-envy candidates. `None` ⇒ caller falls
    /// back to `GraphStore::get_feature_envy_candidates`. Result format
    /// mirrors the PG path : `"source -> dominant_foreign_path (foreign/total)"`.
    pub fn feature_envy_candidates(&self, project: &str, limit: usize) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::feature_envy_candidates(&snap, project, limit))
    }

    /// REQ-AXO-901595 — RAM god-object candidates (fan-in ≥ 20). `None` ⇒
    /// caller falls back to `GraphStore::get_god_objects`. Returns the same
    /// `(name, fan_in)` pairs the PG path produces, sorted by fan_in desc
    /// then name asc.
    pub fn god_objects(&self, project: &str) -> Option<Vec<(String, usize)>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::god_objects(&snap, project))
    }

    /// REQ-AXO-901970 — RAM unsafe-exposure call paths (public → unsafe).
    /// `None` ⇒ cold cache (caller surfaces empty, never a PG WITH RECURSIVE).
    pub fn unsafe_exposure(&self, project: &str) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::unsafe_exposure(&snap, project))
    }

    /// REQ-AXO-901970 — RAM NIF-blocking risks (deep CALLS chain from a NIF).
    /// `None` ⇒ cold cache (caller surfaces empty, never a PG WITH RECURSIVE).
    pub fn nif_blocking_risks(&self, project: &str) -> Option<Vec<String>> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::nif_blocking_risks(&snap, project))
    }

    /// REQ-AXO-901970 — RAM cross-file CALLS flows for `conception_view`.
    /// Returns `(flows[(src,src_file,dst,dst_file)], total_count)`. `None` ⇒
    /// cold cache (caller surfaces empty flows + 0, never a PG join).
    #[allow(clippy::type_complexity)]
    pub fn cross_file_call_flows(
        &self,
        project: &str,
        limit: usize,
    ) -> Option<(Vec<(String, String, String, String)>, usize)> {
        let snap = self.try_snapshot(project)?;
        Some(code_smells::cross_file_call_flows(&snap, project, limit))
    }

    /// REQ-AXO-901595 — RAM structural orphan_code (no callers, non-public,
    /// non-test path). Strict superset of `GraphStore::get_orphan_code_symbols`
    /// which additionally excludes symbols carrying a soll.Traceability
    /// link — that filter requires SOLL state outside the IstGraph, so
    /// callers requiring the canonical orphan_code set must keep the PG
    /// path.
    pub fn orphan_code_symbols(&self, project: &str, limit: usize) -> Option<Vec<String>> {
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
        Some(code_smells::lexical_symbol_search(
            &snap, project, query_text, limit,
        ))
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
                name: "a".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
                complexity: None,
            },
            NodeRecord {
                id: "AXO::b".to_string(),
                name: "b".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
                complexity: None,
            },
            NodeRecord {
                id: "AXO::c".to_string(),
                name: "c".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
                complexity: None,
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

    // REQ-AXO-901952 — the `AXON_IST_RAM_ENABLED` opt-out is removed ; RAM is
    // unconditional. The only gate is cache presence (warm → Some, cold → None).

    #[test]
    fn forward_returns_results_when_warm() {
        let view = IstGraphView::new(warm_cache());
        let r = view
            .forward_at_radius("AXO", "AXO::a", 1, 10, &[RelationType::Calls])
            .expect("warm cache should return Some");
        assert_eq!(r, vec!["AXO::b".to_string()]);
    }

    #[test]
    fn reverse_collects_callers() {
        let view = IstGraphView::new(warm_cache());
        let r = view
            .reverse_at_radius("AXO", "AXO::c", 2, 10, &[RelationType::Calls])
            .expect("warm cache should return Some");
        let set: std::collections::HashSet<&str> = r.iter().map(String::as_str).collect();
        assert!(set.contains("AXO::a"));
        assert!(set.contains("AXO::b"));
    }

    #[test]
    fn reciprocal_count_zero_for_dag() {
        let view = IstGraphView::new(warm_cache());
        assert_eq!(view.reciprocal_calls_cycle_count("AXO"), Some(0));
    }

    // REQ-AXO-901952 (gap B) — node_tested reads the `tested` NodeFlag from RAM,
    // backing change_safety's coverage signal without the PG Symbol count.
    #[test]
    fn node_tested_reads_ram_nodeflags() {
        let nodes = vec![
            NodeRecord {
                id: "AXO::covered".to_string(),
                name: "covered".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::new(true, false, false, false),
                complexity: None,
            },
            NodeRecord {
                id: "AXO::uncovered".to_string(),
                name: "uncovered".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::new(false, false, false, false),
                complexity: None,
            },
        ];
        let cache = Arc::new(IstSnapshotCache::new());
        cache.publish("AXO".to_string(), Arc::new(IstGraph::build(nodes, vec![])));
        let view = IstGraphView::new(cache);
        assert_eq!(view.node_tested("AXO", "AXO::covered"), Some(true));
        assert_eq!(view.node_tested("AXO", "AXO::uncovered"), Some(false));
        // Absent symbol → None (caller treats as not-tested).
        assert_eq!(view.node_tested("AXO", "AXO::ghost"), None);
        // Cold project → None.
        assert_eq!(view.node_tested("ZZZ", "AXO::covered"), None);
    }

    #[test]
    fn is_warm_returns_false_when_cache_empty() {
        let view = IstGraphView::new(Arc::new(IstSnapshotCache::new()));
        assert!(!view.is_warm("AXO"));
    }

    #[test]
    fn cold_cache_returns_none() {
        // REQ-AXO-901952 — cold cache (no snapshot published) → None, so the
        // caller can surface a loud degraded error instead of a silent 0.
        let view = IstGraphView::new(Arc::new(IstSnapshotCache::new()));
        assert!(view.forward_at_radius("AXO", "AXO::a", 1, 10, &[]).is_none());
    }
}
