//! REQ-AXO-219 — evidence-packet builder methods extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Methods on `McpServer`;
//! behavior-preserving move, `self.…` call sites unchanged. They assemble the
//! direct-evidence / why-these / missing-evidence sections of the
//! `retrieve_context` packet.

use super::super::McpServer;
use super::retrieval_model::EntryCandidate;
use serde_json::{json, Value};

impl McpServer {
    pub(super) fn build_direct_evidence(&self, entry_candidates: &[EntryCandidate]) -> Vec<Value> {
        entry_candidates
            .iter()
            .map(|candidate| {
                let evidence_class = if candidate.kind == "file" {
                    "canonical_file"
                } else if candidate.kind == "repo_literal" {
                    "repo_literal_file"
                } else {
                    "canonical_symbol"
                };
                json!({
                    "symbol_id": candidate.id,
                    "name": candidate.name,
                    "kind": candidate.kind,
                    "project_code": candidate.project_code,
                    "uri": candidate.uri,
                    "evidence_class": evidence_class,
                    "score": candidate.score,
                    "ranking_reasons": candidate.reasons,
                })
            })
            .collect()
    }
}
