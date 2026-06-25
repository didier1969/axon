//! REQ-AXO-219 — direct-SOLL-traceability checks extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Methods on `McpServer`;
//! behavior-preserving move, `self.…` / `Self::…` call sites unchanged. RAM-first
//! (SollSnapshot) with PG fallback, per PIL-AXO-9002.

use super::super::McpServer;
use super::retrieval_model::EntryCandidate;

impl McpServer {
    pub(super) fn has_direct_soll_traceability(
        &self,
        entry_candidates: &[EntryCandidate],
        project: Option<&str>,
    ) -> bool {
        // REQ-AXO-902039 element 2 — RAM-first via SollSnapshot (PIL-AXO-9002).
        // The PG form is `count(*) FROM soll.Traceability JOIN soll.Node`; the
        // RAM equivalent scans the per-project snapshot's traceability rows for a
        // Symbol/File artifact whose governing node is present in this project's
        // snapshot (the JOIN + project_filter are implicit: the snapshot is
        // scoped to one project). Project unscoped or snapshot cold ⇒ PG fallback.
        if let Some(proj) = project {
            if let Ok(snap) = self.soll_cache().snapshot(proj) {
                crate::soll_snapshot::record_fusion_read(true);
                return Self::snapshot_has_direct_traceability(&snap, entry_candidates);
            }
        }
        crate::soll_snapshot::record_fusion_read(false);
        let symbol_names = entry_candidates
            .iter()
            .filter(|candidate| candidate.kind != "file")
            .map(|candidate| {
                format!(
                    "'{}'",
                    Self::escape_sql(&candidate.name.to_ascii_lowercase())
                )
            })
            .collect::<Vec<_>>();
        let file_paths = entry_candidates
            .iter()
            .filter(|candidate| !candidate.uri.is_empty())
            .map(|candidate| format!("'{}'", Self::escape_sql(&candidate.uri)))
            .collect::<Vec<_>>();
        let project_filter = project
            .map(|value| {
                format!(
                    " AND lower(n.project_code) IN ({})",
                    Self::project_scope_variants(Some(value))
                        .iter()
                        .map(|variant| format!(
                            "'{}'",
                            Self::escape_sql(&variant.to_ascii_lowercase())
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_default();
        let mut predicates = Vec::new();
        if !symbol_names.is_empty() {
            predicates.push(format!(
                "(t.artifact_type = 'Symbol' AND lower(t.artifact_ref) IN ({}))",
                symbol_names.join(",")
            ));
        }
        if !file_paths.is_empty() {
            predicates.push(format!(
                "(t.artifact_type = 'File' AND t.artifact_ref IN ({}))",
                file_paths.join(",")
            ));
        }
        if predicates.is_empty() {
            return false;
        }
        let query = format!(
            "SELECT count(*) FROM soll.Traceability t \
             JOIN soll.Node n ON n.id = t.soll_entity_id \
             WHERE ({predicates}){project_filter}",
            predicates = predicates.join(" OR "),
        );
        self.graph_store.query_count(&query).unwrap_or(0) > 0
    }

    /// REQ-AXO-902039 element 2 — RAM form of `has_direct_soll_traceability`.
    /// Any Symbol/File traceability row whose governing node is present in this
    /// project's snapshot (the snapshot scopes the JOIN + project_filter).
    pub(super) fn snapshot_has_direct_traceability(
        snap: &crate::soll_snapshot::SollSnapshot,
        entry_candidates: &[EntryCandidate],
    ) -> bool {
        use std::collections::HashSet;
        let symbol_names: HashSet<String> = entry_candidates
            .iter()
            .filter(|c| c.kind != "file")
            .map(|c| c.name.to_ascii_lowercase())
            .collect();
        let file_paths: HashSet<&str> = entry_candidates
            .iter()
            .filter(|c| !c.uri.is_empty())
            .map(|c| c.uri.as_str())
            .collect();
        if symbol_names.is_empty() && file_paths.is_empty() {
            return false;
        }
        snap.traceability.iter().any(|t| {
            let matches = (t.artifact_type == "Symbol"
                && symbol_names.contains(&t.artifact_ref.to_ascii_lowercase()))
                || (t.artifact_type == "File" && file_paths.contains(t.artifact_ref.as_str()));
            matches && snap.nodes.contains_key(&t.soll_entity_id)
        })
    }
}
