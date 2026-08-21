use super::*;

/// REQ-AXO-902337 piste 1 — one broken file-evidence reference, named.
/// A count alone forced SWT to `SELECT ... FROM soll.Traceability` to find
/// what to purge; carrying the offending traceability id + path makes the
/// signal actionable without leaving the tool.
#[derive(Clone, Debug)]
pub(super) struct BrokenFileEvidence {
    pub(super) traceability_id: String,
    pub(super) artifact_ref: String,
}

#[derive(Clone, Debug)]
pub(super) struct RequirementCoverageEntry {
    pub(super) id: String,
    pub(super) status: String,
    pub(super) evidence_count: usize,
    pub(super) validation_count: usize,
    pub(super) has_criteria: bool,
    pub(super) broken_file_evidence_count: usize,
    /// REQ-AXO-902337 piste 1 — the named offenders behind
    /// `broken_file_evidence_count` (empty when the count is 0).
    pub(super) broken_file_evidence: Vec<BrokenFileEvidence>,
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

/// REQ-AXO-136 / REQ-AXO-902173 — canonical terminal-status set for the requirement
/// verifier. "Terminal" = the requirement needs no further work, whether it was
/// DELIVERED (completed / delivered / closed / done / complete / partially_closed) or
/// CLOSED-WITHOUT-DELIVERY (archived / superseded / rejected / cancelled / wont_do /
/// obsolete). BOTH classes must drop out of the partial/gap count — a rejected REQ is
/// as finished, work-wise, as a delivered one (mcp_feedback #37: a `status='rejected'`
/// REQ carrying criteria was mis-counted as a `partial` gap, keeping the verifier
/// inflated forever). Single source of truth shared by `requirement_state_from` and
/// `requirement_missing_dimensions` so the two lists can never drift (GUI-PRO-013).
/// `deferred` is deliberately NOT terminal (work postponed, still owed).
pub(super) fn is_terminal_requirement_status(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "delivered"
            | "closed"
            | "archived"
            | "superseded"
            | "done"
            | "complete"
            | "partially_closed"
            | "rejected"
            | "cancelled"
            | "wont_do"
            | "obsolete"
    )
}

pub(super) fn requirement_state_from(
    status: &str,
    criteria: &str,
    evidence_count: usize,
    broken_file_evidence_count: usize,
) -> &'static str {
    // Terminal-status requirements are done by definition (see
    // is_terminal_requirement_status): no metadata cross-check is required — closing a
    // REQ (completed/delivered) OR terminating it without delivery (rejected/superseded/
    // archived/…) is the canonical "no more work here" signal the verifier must mirror,
    // so soll_verify_requirements tracks the operator-visible decline of partial/missing.
    if is_terminal_requirement_status(status) {
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
    // REQ-AXO-136 / REQ-AXO-902173: a terminal status (is_terminal_requirement_status)
    // OR an active one (current/accepted) is the strongest "status" signal, not a
    // missing-status gap. Everything else (planned, proposed, deferred, …) flags it.
    // Shares the terminal set with requirement_state_from so the two never drift.
    if !is_terminal_requirement_status(status) && !matches!(status, "current" | "accepted") {
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
        "skill" | "ski" => "skill".to_string(), // REQ-AXO-91578
        "prompt_template" | "prompttemplate" | "prt" => "prompt_template".to_string(), // REQ-AXO-91579
        other => other.to_string(),
    }
}

/// REQ-AXO-902321 — the canonical SOLL entity kinds, i.e. exactly the values
/// `normalize_traceability_entity_type` can RESOLVE. Its `other => other` arm is a
/// deliberate passthrough (callers pass already-canonical strings), which meant an
/// unknown kind travelled all the way into `soll.Traceability`: sending
/// `entity_type: "exigence"` wrote a row typed `exigence`, invisible to every query
/// that filters on the canonical kinds. Evidence that exists and cannot be found is
/// worse than evidence that was refused. The list lives HERE so the guard and the
/// normaliser cannot drift.
pub(super) const CANONICAL_TRACEABILITY_ENTITY_TYPES: &[&str] = &[
    "vision",
    "pillar",
    "requirement",
    "concept",
    "decision",
    "milestone",
    "validation",
    "stakeholder",
    "guideline",
    "skill",
    "prompt_template",
];

/// True when `entity_type` resolves to a canonical SOLL kind.
pub(super) fn is_canonical_traceability_entity_type(entity_type: &str) -> bool {
    CANONICAL_TRACEABILITY_ENTITY_TYPES
        .contains(&normalize_traceability_entity_type(entity_type).as_str())
}

pub(super) fn accepted_evidence_artifact_schema(entity_type: &str) -> Vec<&'static str> {
    // REQ-AXO-902390 — `commit`, `soll_ref` and `url` are legal EVERYWHERE: a
    // commit proves any kind of node, and an intent cross-reference is not
    // entity-specific. They were absent from this vocabulary while 6058 `Commit`
    // rows already existed in the live graph (written by `axon_commit_work`), so
    // inferring the right type would have been rejected by the schema.
    // Les valeurs sont comparées via `to_ascii_lowercase()` du type rendu, donc
    // "sollref" et non "soll_ref" — attrapé par le test de schéma.
    let mut accepted = vec!["commit", "sollref", "url"];
    accepted.extend(match normalize_traceability_entity_type(entity_type).as_str() {
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
        "skill" => vec!["document", "file", "symbol", "test", "diff"], // REQ-AXO-91578
        "prompt_template" => vec!["document", "file", "symbol", "test"], // REQ-AXO-91579
        "vision" | "pillar" | "milestone" | "stakeholder" => {
            vec!["document", "file", "symbol", "metric"]
        }
        _ => vec!["document", "file", "symbol"],
    });
    accepted
}

/// REQ-AXO-902418 — every artifact type the handler can accept, for ANY entity
/// kind. This is what the published `inputSchema` enum must declare: JSON Schema
/// carries one enum per field, so the declaration can only be the union, and the
/// per-entity narrowing belongs to the description and the handler.
///
/// The catalog used to spell that union out by hand — while its own description
/// called itself "a mirror of accepted_evidence_artifact_schema, the single
/// source of truth". It had drifted: `commit`, `sollref` and `url`, which
/// `accepted_evidence_artifact_schema` adds for EVERY kind, were missing from
/// it. TE2 measured the cost (`mcp_feedback` #185): five commit SHAs attached as
/// `file` because `commit` was absent from the enum, five rejections, and a
/// second call in `commit` that worked first try. A mirror that no one
/// re-derives is a copy (GUI-PRO-013).
///
/// Derived over `CANONICAL_TRACEABILITY_ENTITY_TYPES`, so a new entity kind or a
/// new artifact type reaches the schema without anyone remembering to.
pub(crate) fn all_accepted_evidence_artifact_types() -> Vec<&'static str> {
    CANONICAL_TRACEABILITY_ENTITY_TYPES
        .iter()
        .flat_map(|kind| accepted_evidence_artifact_schema(kind))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// REQ-AXO-902390 — what SHAPE does this `artifact_ref` have?
///
/// The single place that answers "is this thing a filesystem path?". Three call
/// sites need the same answer and disagreed: evidence attachment (which typed a
/// commit hash as `Document`), the broken-evidence sweep (which then stat()ed it),
/// and `soll_remove_evidence(broken_only=true)` (which would have DELETED it).
///
/// Measured on `axon_live` 2026-08-20: 493 commit hashes and 113 SOLL ids stored
/// as `artifact_type='Document'`, contributing to 1173 rows marked `broken`. APS
/// hit the same defect and checked all 22 of their "broken" refs by hand — 21 were
/// valid. Had they trusted the tool's own suggested remedy, they would have
/// destroyed 3 commit attachments and 18 intent references (inbox 12093).
///
/// Deliberately conservative: anything not RECOGNISED is `Unknown`, which callers
/// treat as "leave it alone". Guessing is what created the mess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactRefShape {
    /// Looks like a filesystem path — the only shape a disk check may resolve.
    Path,
    /// A git object id: 7-40 lowercase hex characters and nothing else.
    CommitHash,
    /// A canonical SOLL id (`TYPE-PROJ-N`, DEC-AXO-085) or a `SOLL:` reference.
    SollRef,
    Url,
    /// Recognised as none of the above. NEVER resolved as a path.
    Unknown,
}

pub(super) fn classify_artifact_ref(artifact_ref: &str) -> ArtifactRefShape {
    let raw = artifact_ref.trim();
    if raw.is_empty() {
        return ArtifactRefShape::Unknown;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return ArtifactRefShape::Url;
    }
    if raw.starts_with("SOLL:") || is_canonical_soll_id_prefix(raw) {
        return ArtifactRefShape::SollRef;
    }
    // Git revisions in every stored form. The `git:` prefix and `HEAD` were found
    // among the rows still flagged broken AFTER the first pass of this fix — the
    // first shape rules were too narrow, and the survivors proved it.
    let git_body = raw.strip_prefix("git:").unwrap_or(raw);
    if is_git_object_id(git_body) || is_git_symbolic_rev(git_body) {
        return ArtifactRefShape::CommitHash;
    }
    // Whitespace settles it: a path does not contain spaces in this corpus, but a
    // note or a shell command does. Live examples that were being stat()ed:
    // `mix compile --warnings-as-errors`, `axon-dev-brain tmux 2026-05-23T04:38:43`,
    // `session-50 2026-05-23 soll_work_plan(project_code=MLD, top=5)`.
    // These are provenance notes recorded in the ref field — real evidence, wrong
    // column. Never a missing file.
    if raw.chars().any(char::is_whitespace) {
        return ArtifactRefShape::Unknown;
    }
    // A `scheme:value` ref is STRUCTURED, not a path. Generalised rather than
    // enumerated: `git:`, `SOLL:` and `commit:` were each found separately, and
    // `live:axon_live:symbol_count` / `disposition:session-2026-06-20` showed the
    // scheme vocabulary is open. Callers coin their own; the shape is the invariant.
    // A scheme must precede any separator, so `docs/a:b.md` stays a path.
    if let Some(colon) = raw.find(':') {
        let scheme = &raw[..colon];
        let before_separator = !raw[..colon].contains('/') && !raw[..colon].contains('\\');
        if before_separator
            && !scheme.is_empty()
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            // `commit:<hash>` is a commit; any other scheme is simply not a path.
            let value = &raw[colon + 1..];
            return if is_git_object_id(value) {
                ArtifactRefShape::CommitHash
            } else {
                ArtifactRefShape::Unknown
            };
        }
    }
    if raw.contains('/') || raw.contains('\\') || raw.contains('.') {
        return ArtifactRefShape::Path;
    }
    ArtifactRefShape::Unknown
}

/// 7-40 lowercase hex and nothing else. The length window excludes a 6-char word;
/// requiring ALL hex excludes `deadbeef.txt` and `docs/abc123.md`.
fn is_git_object_id(raw: &str) -> bool {
    (7..=40).contains(&raw.len())
        && raw
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// `HEAD`, `HEAD~1`, `HEAD^`, `ORIG_HEAD`, `FETCH_HEAD` — symbolic revisions.
fn is_git_symbolic_rev(raw: &str) -> bool {
    let head = raw
        .split(['~', '^'])
        .next()
        .unwrap_or(raw);
    matches!(head, "HEAD" | "ORIG_HEAD" | "FETCH_HEAD" | "MERGE_HEAD")
}

/// `TYPE-PROJ-N` with a 3-char uppercase project code — the DEC-AXO-085 format.
/// Kept separate from `project_code_from_canonical_entity_id` because that one
/// answers "which project", not "is this an id at all".
fn is_canonical_soll_id_prefix(raw: &str) -> bool {
    let head = raw.split(['#', ':']).next().unwrap_or(raw);
    let mut parts = head.split('-');
    let (Some(kind), Some(project), Some(number)) = (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    kind.len() >= 3
        && kind.chars().all(|c| c.is_ascii_uppercase())
        && project.len() == 3
        && project.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

/// REQ-AXO-902390 — may a disk existence check decide this row is broken?
///
/// Both the declared type AND the ref shape must agree. The declared type alone
/// was the bug: `Document` is the fallback bucket, so everything unrecognised
/// landed there and got stat()ed.
pub(super) fn evidence_ref_is_disk_checkable(artifact_type: &str, artifact_ref: &str) -> bool {
    matches!(
        artifact_type.trim().to_ascii_lowercase().as_str(),
        "file" | "document"
    ) && classify_artifact_ref(artifact_ref) == ArtifactRefShape::Path
}

pub(super) fn normalize_evidence_artifact_type(raw: &str, artifact_ref: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        // REQ-AXO-902390 — `Document` was the FALLBACK bucket: anything without a
        // `/` or a `.md` suffix landed there, including commit hashes and SOLL
        // ids, which the broken-evidence sweep then resolved as filesystem paths.
        // Type from the SHAPE first; `Document` now means "a document", not
        // "whatever is left".
        "" | "document" | "doc" => match classify_artifact_ref(artifact_ref) {
            ArtifactRefShape::CommitHash => "Commit".to_string(),
            ArtifactRefShape::SollRef => "SollRef".to_string(),
            ArtifactRefShape::Url => "Url".to_string(),
            ArtifactRefShape::Path => "File".to_string(),
            ArtifactRefShape::Unknown => "Document".to_string(),
        },
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

/// REQ-AXO-902289 — the `soll_manager` entity implied by a canonical id.
///
/// The id format (DEC-AXO-085, `TYPE-PROJ-N`) makes the entity a FUNCTION of the
/// prefix: the live graph holds twelve prefixes and each maps to exactly one
/// node type, no overlap. So an `update`/`link` call that carries `data.id` and
/// omits `entity` is not ambiguous — it is under-specified in a recoverable way.
///
/// Returns the lowercase entity name the tool's enum expects, or `None` for an
/// unknown prefix (the caller then keeps the ordinary missing-field rejection —
/// guessing past an unrecognised prefix is exactly what this must not do).
pub(crate) fn soll_entity_from_canonical_id(entity_id: &str) -> Option<&'static str> {
    match entity_id.split('-').next()?.trim() {
        "VIS" => Some("vision"),
        "PIL" => Some("pillar"),
        "REQ" => Some("requirement"),
        "DEC" => Some("decision"),
        "CPT" => Some("concept"),
        "GUI" => Some("guideline"),
        "MIL" => Some("milestone"),
        "VAL" => Some("validation"),
        "STK" => Some("stakeholder"),
        "SKI" => Some("skill"),
        "PRT" => Some("prompt_template"),
        "TMG" => Some("technology_migration"),
        _ => None,
    }
}

/// REQ-AXO-139 slice (`soll_attach_evidence`): per-kind required-field hint
/// returned via `data.parameter_repair.required_field_hint` when an artifact
/// is rejected for a missing `artifact_ref`. The kind values are the
/// normalized artifact types produced by [`normalize_evidence_artifact_type`].
pub(super) fn required_field_hint_for_artifact_kind(kind: &str) -> &'static str {
    match kind {
        "File" => {
            "supply a file path (relative to the project root or absolute) — \
             accepted aliases: `artifact_ref`, `path`, `file_path`, `uri`"
        }
        "Document" => {
            "supply a document reference (file path or URL) — \
             accepted aliases: `artifact_ref`, `path`, `file_path`, `uri`"
        }
        "Symbol" => "supply a canonical symbol id (e.g. `module::function`) in `artifact_ref`",
        "Test" => {
            "supply a qualified test path (e.g. `module::tests::test_name`) \
             or canonical test id in `artifact_ref`"
        }
        "Metric" => "supply a metric name or dashboard URL in `artifact_ref`",
        "Validation" => "supply a canonical validation id (`VAL-CODE-NNN`) in `artifact_ref`",
        "Rationale" => "supply rationale text or a document reference in `artifact_ref`",
        "Diff" => "supply a commit SHA or a path to a `.diff` artifact in `artifact_ref`",
        _ => "supply a non-empty `artifact_ref` value matching the artifact_type",
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
mod requirement_state_tests {
    use super::{requirement_missing_dimensions, requirement_state_from};

    /// MIL-AXO-016 wave 9 + REQ-AXO-902173: every terminal status must short-circuit
    /// the verifier into "done", whether DELIVERED (closed / archived / superseded /
    /// partially_closed / done / complete) or CLOSED-WITHOUT-DELIVERY (rejected /
    /// cancelled / wont_do / obsolete). Otherwise soll_verify_requirements stays
    /// inflated long after the operator has finished (or abandoned) the work.
    #[test]
    fn terminal_statuses_count_as_done() {
        for status in [
            "completed",
            "delivered",
            "closed",
            "archived",
            "superseded",
            "done",
            "complete",
            "partially_closed",
            // REQ-AXO-902173 — terminal-without-delivery (mcp_feedback #37).
            "rejected",
            "cancelled",
            "wont_do",
            "obsolete",
        ] {
            assert_eq!(
                requirement_state_from(status, "", 0, 0),
                "done",
                "status={status} should map to done"
            );
        }
    }

    /// REQ-AXO-902173 — the exact mcp_feedback #37 shape: a `rejected` REQ that still
    /// carries acceptance criteria + evidence must NOT be counted as a `partial` gap.
    #[test]
    fn rejected_requirement_with_signals_is_not_a_gap_902173() {
        assert_eq!(requirement_state_from("rejected", "AC1: foo", 1, 0), "done");
        assert_eq!(requirement_state_from("rejected", "AC1: foo", 0, 0), "done");
        // deferred is NOT terminal — postponed work is still owed, stays a gap.
        assert_ne!(requirement_state_from("deferred", "AC1: foo", 0, 0), "done");
    }

    /// Active statuses still need evidence + criteria + zero broken
    /// file evidence to be "done"; otherwise they degrade to partial.
    #[test]
    fn active_statuses_need_full_coverage_to_be_done() {
        for status in ["current", "accepted"] {
            assert_eq!(requirement_state_from(status, "AC1: foo", 1, 0), "done");
            // Missing evidence → partial, not done.
            assert_eq!(requirement_state_from(status, "AC1: foo", 0, 0), "partial");
            // Broken file evidence → partial.
            assert_eq!(requirement_state_from(status, "AC1: foo", 1, 1), "partial");
        }
    }

    /// Empty status with no signals stays missing — no closure marker
    /// short-circuits us out of the missing branch.
    #[test]
    fn empty_status_with_no_signals_is_missing() {
        assert_eq!(requirement_state_from("", "", 0, 0), "missing");
        assert_eq!(requirement_state_from("planned", "", 0, 0), "missing");
    }

    /// Terminal statuses also clear the "status" missing-dimension flag.
    #[test]
    fn terminal_statuses_do_not_flag_status_dimension() {
        for status in [
            "completed",
            "delivered",
            "closed",
            "archived",
            "superseded",
            "done",
            "complete",
            "partially_closed",
            "rejected",
            "cancelled",
            "wont_do",
            "obsolete",
            "current",
            "accepted",
        ] {
            let dims = requirement_missing_dimensions(status, true, 1, 1, 0);
            assert!(
                !dims.iter().any(|d| d == "status"),
                "status={status} should not flag the status dimension; got {dims:?}"
            );
        }
    }

    #[test]
    fn non_terminal_status_flags_status_dimension() {
        let dims = requirement_missing_dimensions("planned", true, 1, 1, 0);
        assert!(dims.iter().any(|d| d == "status"));
    }
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

#[cfg(test)]
mod req_902390_artifact_ref_shape_tests {
    use super::*;

    #[test]
    fn a_commit_hash_is_never_a_path() {
        // Les formes VERBATIM trouvées typées `Document` dans axon_live.
        for hash in ["01c24be7", "024333dd", "923f0f4", "e5f370f", "56e88a6"] {
            assert_eq!(
                classify_artifact_ref(hash),
                ArtifactRefShape::CommitHash,
                "{hash} devrait être reconnu comme un hash git"
            );
            assert!(
                !evidence_ref_is_disk_checkable("Document", hash),
                "{hash} ne doit JAMAIS être résolu sur disque"
            );
        }
    }

    #[test]
    fn a_soll_reference_is_never_a_path() {
        // Ceux d'APS (inbox 12093) et les nôtres.
        for id in [
            "MIL-APS-001",
            "DEC-APS-003",
            "CPT-AXO-018",
            "CPT-AXO-90044",
            "REQ-AXO-902390",
            "SOLL:REQ-APS-158#kpi-contract",
        ] {
            assert_eq!(
                classify_artifact_ref(id),
                ArtifactRefShape::SollRef,
                "{id} devrait être reconnu comme un renvoi SOLL"
            );
            assert!(!evidence_ref_is_disk_checkable("Document", id));
        }
    }

    #[test]
    fn a_real_path_is_still_disk_checkable() {
        // LA falsification : la branche qui détecte un vrai fichier disparu doit
        // rester ATTEIGNABLE. Un correctif qui rend l'outil muet ne vaut pas
        // mieux que celui qui le rend bavard.
        let real = "/home/dstadel/projects/aps3d/lib/aps3d/tms/route_optimizer.ex";
        assert_eq!(classify_artifact_ref(real), ArtifactRefShape::Path);
        assert!(evidence_ref_is_disk_checkable("File", real));
        assert!(evidence_ref_is_disk_checkable("Document", "docs/architecture.md"));
    }

    #[test]
    fn the_aps_case_yields_one_offender_not_twenty_two() {
        // Rejeu du décompte exact d'APS : 3 hashes + 18 renvois + 1 chemin.
        let mut refs: Vec<String> = vec![
            "923f0f4".to_string(),
            "e5f370f".to_string(),
            "56e88a6".to_string(),
            "/home/dstadel/projects/aps3d/lib/aps3d/tms/route_optimizer.ex".to_string(),
        ];
        refs.push("MIL-APS-001".to_string());
        refs.push("DEC-APS-003".to_string());
        refs.push("SOLL:REQ-APS-158#kpi-contract".to_string());
        for n in 216..=228 {
            refs.push(format!("REQ-APS-{n}"));
        }
        refs.push("REQ-APS-158".to_string());
        refs.push("REQ-APS-159".to_string());
        assert_eq!(refs.len(), 22, "le cas d'APS compte bien 22 refs");

        let checkable: Vec<&String> = refs
            .iter()
            .filter(|r| evidence_ref_is_disk_checkable("Document", r))
            .collect();
        assert_eq!(
            checkable.len(),
            1,
            "une seule ref est vérifiable sur disque, pas 22 : {checkable:?}"
        );
    }

    #[test]
    fn document_is_no_longer_the_fallback_bucket() {
        // La cause racine : `Document` recevait tout ce qui n'avait ni `/` ni `.md`.
        assert_eq!(normalize_evidence_artifact_type("", "01c24be7"), "Commit");
        assert_eq!(normalize_evidence_artifact_type("document", "CPT-AXO-018"), "SollRef");
        assert_eq!(
            normalize_evidence_artifact_type("doc", "https://example.test/x"),
            "Url"
        );
        assert_eq!(normalize_evidence_artifact_type("", "docs/x.md"), "File");
        // Et un vrai document reste un document.
        assert_eq!(
            normalize_evidence_artifact_type("document", "cahier des charges"),
            "Document"
        );
    }

    #[test]
    fn the_inferred_types_are_accepted_by_the_schema() {
        // Sans ça, inférer le BON type le ferait rejeter — 6058 lignes `Commit`
        // existent déjà dans le graphe live alors que le vocabulaire les ignorait.
        for entity in ["requirement", "decision", "concept", "vision"] {
            assert!(artifact_schema_accepts(entity, "Commit"), "{entity} / Commit");
            assert!(artifact_schema_accepts(entity, "SollRef"), "{entity} / SollRef");
            assert!(artifact_schema_accepts(entity, "Url"), "{entity} / Url");
        }
    }

    #[test]
    fn the_survivors_of_the_first_pass_are_recognised() {
        // Formes VERBATIM encore marquées `broken` APRÈS la première passe de ce
        // correctif : elles ont prouvé que mes règles initiales étaient trop
        // étroites. Un correctif dont on ne vérifie pas les survivants n'est
        // vérifié qu'à moitié.
        for git_ref in ["git:f9a2da1", "git:23fcb7a", "HEAD", "HEAD~1", "ORIG_HEAD"] {
            assert_eq!(
                classify_artifact_ref(git_ref),
                ArtifactRefShape::CommitHash,
                "{git_ref} est une révision git"
            );
            assert!(!evidence_ref_is_disk_checkable("Document", git_ref));
        }
        // Notes de provenance et commandes rangées dans le champ ref : une preuve
        // réelle, dans la mauvaise colonne. Jamais un fichier disparu.
        for note in [
            "mix compile --warnings-as-errors",
            "axon-dev-brain tmux 2026-05-23T04:38:43",
            "session-50 2026-05-23 soll_work_plan(project_code=MLD, top=5)",
        ] {
            assert_eq!(
                classify_artifact_ref(note),
                ArtifactRefShape::Unknown,
                "{note} n'est pas un chemin"
            );
            assert!(!evidence_ref_is_disk_checkable("Document", note));
        }
    }

    #[test]
    fn a_scheme_prefixed_ref_is_never_a_path() {
        // Formes VERBATIM encore marquées broken après la DEUXIÈME passe. Le
        // vocabulaire des schémas est ouvert — les appelants inventent le leur —
        // donc la règle porte sur la FORME, pas sur une liste.
        assert_eq!(
            classify_artifact_ref("commit:532b8cab"),
            ArtifactRefShape::CommitHash
        );
        for structured in [
            "live:axon_live:symbol_count",
            "disposition:session-2026-06-20",
            "run:2026-06-20T10:00:00Z",
        ] {
            assert_eq!(
                classify_artifact_ref(structured),
                ArtifactRefShape::Unknown,
                "{structured} est un ref structuré, pas un chemin"
            );
            assert!(!evidence_ref_is_disk_checkable("Document", structured));
        }
        // Mais un chemin qui contient un `:` APRÈS un séparateur reste un chemin.
        assert_eq!(classify_artifact_ref("docs/a:b.md"), ArtifactRefShape::Path);
    }

    #[test]
    fn a_hex_looking_filename_is_a_path_not_a_hash() {
        // Le piège inverse : ne pas classer un fichier comme un commit.
        assert_eq!(classify_artifact_ref("deadbeef.txt"), ArtifactRefShape::Path);
        assert_eq!(classify_artifact_ref("docs/abc123.md"), ArtifactRefShape::Path);
        // Trop court pour un hash abrégé, et pas un chemin : on ne devine pas.
        assert_eq!(classify_artifact_ref("abc12"), ArtifactRefShape::Unknown);
        assert!(!evidence_ref_is_disk_checkable("Document", "abc12"));
    }
}
