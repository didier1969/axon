use super::*;

#[derive(Clone, Debug)]
pub(super) struct RequirementCoverageEntry {
    pub(super) id: String,
    pub(super) status: String,
    pub(super) evidence_count: usize,
    pub(super) validation_count: usize,
    pub(super) has_criteria: bool,
    pub(super) broken_file_evidence_count: usize,
    pub(super) state: String,
    pub(super) missing_dimensions: Vec<String>,
    pub(super) suggested_next_actions: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct SollDocNode {
    pub(super) id: String,
    pub(super) entity_type: String,
    pub(super) title: String,
    pub(super) description: String,
    pub(super) status: String,
    pub(super) metadata: String,
}

#[derive(Clone, Debug)]
pub(super) struct SollDocEdge {
    pub(super) source_id: String,
    pub(super) target_id: String,
    pub(super) relation_type: String,
}

#[derive(Clone, Debug)]
pub(super) struct SollDocPageSpec {
    pub(super) relative_path: String,
    pub(super) title: String,
    pub(super) html: String,
    pub(super) node_ids: Vec<String>,
    pub(super) edge_keys: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct SollDerivedProjectEntry {
    pub(super) project_code: String,
    pub(super) project_name: String,
    pub(super) project_path: String,
    pub(super) node_count: usize,
    pub(super) has_docs: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SollDerivedDocsRefreshSummary {
    pub(crate) project_code: String,
    pub(crate) site_root: String,
    pub(crate) project_output_root: String,
    pub(crate) project_manifest_path: String,
    pub(crate) root_manifest_path: String,
    pub(crate) root_index_path: String,
    pub(crate) refresh_mode: String,
    pub(crate) pages_total: usize,
    pub(crate) pages_written: usize,
    pub(crate) pages_unchanged: usize,
    pub(crate) pages_deleted: usize,
    pub(crate) deleted_paths: Vec<String>,
    pub(crate) root_written: bool,
    pub(crate) stale_docs: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RequirementCoverageSummary {
    pub(super) done: usize,
    pub(super) partial: usize,
    pub(super) missing: usize,
    pub(super) entries: Vec<RequirementCoverageEntry>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SollCompletenessSnapshot {
    pub(super) project_scope: String,
    pub(super) total_nodes: usize,
    pub(super) orphan_requirements: Vec<String>,
    pub(super) validations_without_verifies: Vec<String>,
    pub(super) decisions_without_links: Vec<String>,
    pub(super) uncovered_requirements: Vec<String>,
    pub(super) duplicate_title_rows: Vec<Vec<String>>,
    pub(super) duplicate_ids: Vec<String>,
    pub(super) relation_policy_violations: Vec<String>,
    pub(super) requirement_coverage: RequirementCoverageSummary,
}

impl SollCompletenessSnapshot {
    pub(crate) fn structurally_connected(&self) -> bool {
        self.orphan_requirements.is_empty()
            && self.validations_without_verifies.is_empty()
            && self.decisions_without_links.is_empty()
            && self.relation_policy_violations.is_empty()
    }

    pub(crate) fn duplicate_free(&self) -> bool {
        self.duplicate_title_rows.is_empty()
    }

    pub(crate) fn evidence_ready(&self) -> bool {
        self.uncovered_requirements.is_empty()
    }

    pub(crate) fn concept_complete(&self) -> bool {
        self.total_nodes > 0 && self.structurally_connected() && self.duplicate_free()
    }

    pub(crate) fn implementation_complete(&self) -> bool {
        self.requirement_coverage.missing == 0
    }

    pub(crate) fn canonical_orphan_intent_ids(&self) -> BTreeSet<String> {
        self.orphan_requirements
            .iter()
            .chain(self.validations_without_verifies.iter())
            .chain(self.decisions_without_links.iter())
            .chain(self.uncovered_requirements.iter())
            .chain(self.duplicate_ids.iter())
            .cloned()
            .collect()
    }
}

pub(super) fn requirement_state_from(
    status: &str,
    criteria: &str,
    evidence_count: usize,
    broken_file_evidence_count: usize,
) -> &'static str {
    // REQ-AXO-136: terminal-status requirements are done by definition.
    // status=`completed` means the work was delivered; status=`delivered` is
    // the Decision-side equivalent that a Requirement may inherit. No
    // metadata cross-check is required for the terminal path — closing a
    // REQ via `soll_manager update status=completed` is the canonical "I'm
    // done" signal an LLM emits, and the verifier must mirror it.
    if matches!(status, "completed" | "delivered") {
        return "done";
    }
    let has_criteria = !criteria.trim().is_empty() && criteria.trim() != "[]";
    if evidence_count > 0
        && broken_file_evidence_count == 0
        && has_criteria
        && matches!(status, "current" | "accepted")
    {
        "done"
    } else if evidence_count > 0 || has_criteria || broken_file_evidence_count > 0 {
        "partial"
    } else {
        "missing"
    }
}

pub(super) fn requirement_missing_dimensions(
    status: &str,
    has_criteria: bool,
    evidence_count: usize,
    validation_count: usize,
    broken_file_evidence_count: usize,
) -> Vec<String> {
    let mut missing = Vec::new();
    // REQ-AXO-136: terminal statuses count as the strongest "status" signal,
    // not as a missing-status gap. Active statuses (current/accepted) also
    // pass; everything else flags the status dimension.
    if !matches!(status, "current" | "accepted" | "completed" | "delivered") {
        missing.push("status".to_string());
    }
    if !has_criteria {
        missing.push("criteria".to_string());
    }
    if evidence_count == 0 {
        missing.push("evidence".to_string());
    }
    if validation_count == 0 {
        missing.push("validation".to_string());
    }
    if broken_file_evidence_count > 0 {
        missing.push("broken_file_evidence".to_string());
    }
    missing
}

pub(super) fn requirement_dimension_canonical_name(dimension: &str) -> &str {
    match dimension {
        "status" => "accepted_runtime_status",
        "criteria" => "structured_acceptance_criteria",
        "evidence" => "supporting_evidence",
        "validation" => "qualifying_validation_edge",
        "broken_file_evidence" => "resolvable_file_evidence",
        _ => dimension,
    }
}

pub(super) fn requirement_dimension_descriptor(dimension: &str) -> Value {
    match dimension {
        "status" => json!({
            "legacy_key": "status",
            "canonical_key": "accepted_runtime_status",
            "label": "Accepted runtime status",
            "severity": "blocking",
            "meaning": "Requirement status should be `current` or `accepted` before it is treated as complete.",
            "next_action": "set requirement status to `current` or `accepted`"
        }),
        "criteria" => json!({
            "legacy_key": "criteria",
            "canonical_key": "structured_acceptance_criteria",
            "label": "Structured acceptance criteria",
            "severity": "blocking",
            "meaning": "Requirement metadata must include explicit acceptance criteria.",
            "next_action": "add acceptance criteria in requirement metadata"
        }),
        "evidence" => json!({
            "legacy_key": "evidence",
            "canonical_key": "supporting_evidence",
            "label": "Supporting evidence",
            "severity": "blocking",
            "meaning": "At least one traceability or proof artifact should support this requirement.",
            "next_action": "attach proof with `soll_attach_evidence`"
        }),
        "validation" => json!({
            "legacy_key": "validation",
            "canonical_key": "qualifying_validation_edge",
            "label": "Qualifying validation edge",
            "severity": "blocking",
            "meaning": "A validation node should `VERIFIES` this requirement before it is considered done.",
            "next_action": "create or link a validation node that `VERIFIES` the requirement"
        }),
        "broken_file_evidence" => json!({
            "legacy_key": "broken_file_evidence",
            "canonical_key": "resolvable_file_evidence",
            "label": "Resolvable file evidence",
            "severity": "warning",
            "meaning": "Some attached file evidence is no longer reachable on disk and weakens proof quality.",
            "next_action": "repair or replace broken file evidence paths before relying on coverage"
        }),
        _ => json!({
            "legacy_key": dimension,
            "canonical_key": dimension,
            "label": dimension,
            "severity": "warning",
            "meaning": "Additional requirement coverage dimension",
            "next_action": Value::Null
        }),
    }
}

pub(super) fn requirement_next_actions(missing_dimensions: &[String]) -> Vec<String> {
    let mut actions = Vec::new();
    for dimension in missing_dimensions {
        let action = match dimension.as_str() {
            "status" => "set requirement status to `current` or `accepted`".to_string(),
            "criteria" => "add acceptance criteria in requirement metadata".to_string(),
            "evidence" => "attach proof with `soll_attach_evidence`".to_string(),
            "validation" => {
                "create or link a validation node that `VERIFIES` the requirement".to_string()
            }
            "broken_file_evidence" => {
                "repair or replace broken file evidence paths before relying on coverage"
                    .to_string()
            }
            _ => continue,
        };
        if !actions.contains(&action) {
            actions.push(action);
        }
    }
    actions
}

pub(super) fn requirement_state_reason(state: &str, missing_dimensions: &[String]) -> String {
    if missing_dimensions.is_empty() {
        return "Requirement is complete across status, criteria, evidence, and validation coverage."
            .to_string();
    }
    let canonical = missing_dimensions
        .iter()
        .map(|dimension| requirement_dimension_canonical_name(dimension))
        .collect::<Vec<_>>()
        .join(", ");
    match state {
        "done" => format!(
            "Requirement is complete, but operator attention is still required for: {canonical}."
        ),
        "partial" => format!(
            "Requirement is partially complete because coverage is still missing for: {canonical}."
        ),
        _ => format!("Requirement is missing required coverage dimensions: {canonical}."),
    }
}

pub(super) fn normalize_traceability_entity_type(entity_type: &str) -> String {
    match entity_type.trim().to_ascii_lowercase().as_str() {
        "vision" | "vis" => "vision".to_string(),
        "pillar" | "pil" => "pillar".to_string(),
        "requirement" | "req" => "requirement".to_string(),
        "concept" | "cpt" => "concept".to_string(),
        "decision" | "dec" => "decision".to_string(),
        "milestone" | "mil" => "milestone".to_string(),
        "validation" | "val" => "validation".to_string(),
        "stakeholder" | "stk" => "stakeholder".to_string(),
        "guideline" | "gui" => "guideline".to_string(),
        other => other.to_string(),
    }
}

pub(super) fn accepted_evidence_artifact_schema(entity_type: &str) -> Vec<&'static str> {
    match normalize_traceability_entity_type(entity_type).as_str() {
        "requirement" => vec!["document", "file", "symbol", "test", "metric", "validation"],
        "decision" => vec![
            "document",
            "file",
            "symbol",
            "rationale",
            "diff",
            "validation",
        ],
        "validation" => vec!["document", "file", "symbol", "test", "metric", "diff"],
        "concept" => vec!["document", "file", "symbol", "rationale"],
        "guideline" => vec!["document", "file", "symbol", "diff"],
        "vision" | "pillar" | "milestone" | "stakeholder" => {
            vec!["document", "file", "symbol", "metric"]
        }
        _ => vec!["document", "file", "symbol"],
    }
}

pub(super) fn normalize_evidence_artifact_type(raw: &str, artifact_ref: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "document" | "doc" => {
            if artifact_ref.contains('/') || artifact_ref.ends_with(".md") {
                "File".to_string()
            } else {
                "Document".to_string()
            }
        }
        "file" | "path" | "uri" => "File".to_string(),
        "symbol" | "code" => "Symbol".to_string(),
        "test" => "Test".to_string(),
        "metric" => "Metric".to_string(),
        "validation" => "Validation".to_string(),
        "rationale" => "Rationale".to_string(),
        "diff" => "Diff".to_string(),
        other => {
            let mut chars = other.chars();
            if let Some(first) = chars.next() {
                format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
            } else {
                "Unknown".to_string()
            }
        }
    }
}

pub(super) fn artifact_schema_accepts(entity_type: &str, artifact_type: &str) -> bool {
    let normalized = artifact_type.to_ascii_lowercase();
    accepted_evidence_artifact_schema(entity_type)
        .iter()
        .any(|candidate| {
            *candidate == normalized || (*candidate == "document" && normalized == "file")
        })
}

pub(super) fn project_code_from_canonical_entity_id(entity_id: &str) -> Option<String> {
    let mut parts = entity_id.split('-');
    let _prefix = parts.next()?;
    let project_code = parts.next()?.trim();
    if project_code.len() == 3
        && project_code
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() && !ch.is_ascii_lowercase())
    {
        Some(project_code.to_string())
    } else {
        None
    }
}

/// REQ-AXO-066 Phase 1 (DEC-AXO-064 Option A): standardised `project_code`
/// scoping fragment for SOLL/IST queries.
///
/// - `Some(code)` validated by [`is_valid_project_code`] →
///   `" AND <column_prefix>project_code = '<code>'"`.
/// - `None` or empty/invalid code → `""` (caller is responsible for unscoped reads).
///
/// Single quotes inside `code` are escaped per the existing codebase
/// convention (`code.replace('\'', "''")`); valid project codes never
/// contain quotes, but the escape is kept defensively.
pub(crate) fn scoped_query_filter(project_code: Option<&str>, column_prefix: &str) -> String {
    let Some(code) = project_code else {
        return String::new();
    };
    let trimmed = code.trim();
    if trimmed.is_empty() || !is_valid_project_code(trimmed) {
        return String::new();
    }
    let escaped = trimmed.replace('\'', "''");
    format!(" AND {column_prefix}project_code = '{escaped}'")
}

#[cfg(test)]
mod scoped_query_filter_tests {
    use super::scoped_query_filter;

    #[test]
    fn returns_empty_when_project_code_is_none() {
        assert_eq!(scoped_query_filter(None, ""), "");
        assert_eq!(scoped_query_filter(None, "n."), "");
    }

    #[test]
    fn returns_empty_when_project_code_is_blank() {
        assert_eq!(scoped_query_filter(Some(""), ""), "");
        assert_eq!(scoped_query_filter(Some("   "), "n."), "");
    }

    #[test]
    fn returns_empty_when_project_code_is_invalid() {
        // is_valid_project_code requires exactly 3 ascii alphanumerics; case
        // insensitive (uppercase is the convention but not enforced).
        assert_eq!(scoped_query_filter(Some("AX"), ""), "");
        assert_eq!(scoped_query_filter(Some("AXON"), ""), "");
        assert_eq!(scoped_query_filter(Some("AX!"), ""), "");
    }

    #[test]
    fn applies_filter_with_unprefixed_column() {
        assert_eq!(
            scoped_query_filter(Some("AXO"), ""),
            " AND project_code = 'AXO'"
        );
    }

    #[test]
    fn applies_filter_with_qualified_column_prefix() {
        assert_eq!(
            scoped_query_filter(Some("BKS"), "n."),
            " AND n.project_code = 'BKS'"
        );
        assert_eq!(
            scoped_query_filter(Some("PRO"), "soll.Node."),
            " AND soll.Node.project_code = 'PRO'"
        );
    }

    #[test]
    fn trims_whitespace_around_valid_code() {
        assert_eq!(
            scoped_query_filter(Some("  AXO  "), ""),
            " AND project_code = 'AXO'"
        );
    }
}
