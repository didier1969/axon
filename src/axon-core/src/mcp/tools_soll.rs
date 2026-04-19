use anyhow::anyhow;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::format::format_standard_contract;
use super::soll::{
    canonical_soll_export_dir, canonical_soll_site_dir, find_latest_soll_export, parse_soll_export,
    SollRestoreCounts,
};
use super::McpServer;
use crate::project_meta::{
    discover_project_identities, is_valid_project_code, resolve_canonical_project_identity,
};

#[allow(dead_code)]
const SOLL_RELATION_EXPORTS: [(&str, &str); 12] = [
    ("EPITOMIZES", "soll.EPITOMIZES"),
    ("BELONGS_TO", "soll.BELONGS_TO"),
    ("EXPLAINS", "soll.EXPLAINS"),
    ("SOLVES", "soll.SOLVES"),
    ("TARGETS", "soll.TARGETS"),
    ("VERIFIES", "soll.VERIFIES"),
    ("ORIGINATES", "soll.ORIGINATES"),
    ("SUPERSEDES", "soll.SUPERSEDES"),
    ("CONTRIBUTES_TO", "soll.CONTRIBUTES_TO"),
    ("REFINES", "soll.REFINES"),
    ("IMPACTS", "IMPACTS"),
    ("SUBSTANTIATES", "SUBSTANTIATES"),
];

#[allow(dead_code)]
type SollContextCache = HashMap<String, (i64, Value)>;

#[allow(dead_code)]
static SOLL_CONTEXT_CACHE: OnceLock<Mutex<SollContextCache>> = OnceLock::new();

#[allow(dead_code)]
const SOLL_CONTEXT_CACHE_TTL_MS: i64 = 180_000;
const SOLL_PROJECT_DOCS_GENERATOR_VERSION: &str = "soll_generate_docs_v3";
const SOLL_ROOT_DOCS_GENERATOR_VERSION: &str = "soll_generate_docs_root_v2";

impl McpServer {
    #[cfg(not(test))]
    fn soll_context_cache() -> &'static Mutex<SollContextCache> {
        SOLL_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    #[cfg(not(test))]
    fn read_soll_context_cache(key: &str, now_ms: i64) -> Option<Value> {
        let guard = Self::soll_context_cache().lock().ok()?;
        let (stored_at, value) = guard.get(key)?;
        if now_ms.saturating_sub(*stored_at) > SOLL_CONTEXT_CACHE_TTL_MS {
            return None;
        }
        Some(value.clone())
    }

    #[cfg(test)]
    fn read_soll_context_cache(_key: &str, _now_ms: i64) -> Option<Value> {
        None
    }

    #[cfg(not(test))]
    fn write_soll_context_cache(key: String, now_ms: i64, value: &Value) {
        if let Ok(mut guard) = Self::soll_context_cache().lock() {
            guard.insert(key, (now_ms, value.clone()));
        }
    }

    #[cfg(test)]
    fn write_soll_context_cache(_key: String, _now_ms: i64, _value: &Value) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WorkPlanEntityType {
    Decision,
    Requirement,
    Milestone,
}

impl WorkPlanEntityType {
    fn label(&self) -> &'static str {
        match self {
            Self::Decision => "Decision",
            Self::Requirement => "Requirement",
            Self::Milestone => "Milestone",
        }
    }

    fn sort_rank(&self) -> usize {
        match self {
            Self::Decision => 0,
            Self::Requirement => 1,
            Self::Milestone => 2,
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct WorkPlanNode {
    id: String,
    title: String,
    entity_type: WorkPlanEntityType,
    status: String,
    priority: String,
    requirement_state: Option<String>,
    evidence_count: usize,
    descendants: usize,
    ist_degraded_links: usize,
    backlog_visible: bool,
    score: i64,
    reasons: Vec<String>,
    validation_gates: Vec<String>,
    ist_signals: Vec<String>,
}

#[derive(Clone, Debug)]
struct WorkPlanWave {
    wave_index: usize,
    items: Vec<WorkPlanNode>,
}

#[derive(Clone, Debug)]
struct WorkPlanCycle {
    node_ids: Vec<String>,
}

#[derive(Clone, Debug)]
struct WorkPlanBlocker {
    id: String,
    entity_type: String,
    reason: String,
}

#[derive(Clone, Debug)]
struct RequirementCoverageEntry {
    id: String,
    status: String,
    evidence_count: usize,
    state: String,
}

#[derive(Clone, Debug)]
struct SollDocNode {
    id: String,
    entity_type: String,
    title: String,
    description: String,
    status: String,
    metadata: String,
}

#[derive(Clone, Debug)]
struct SollDocEdge {
    source_id: String,
    target_id: String,
    relation_type: String,
}

#[derive(Clone, Debug)]
struct SollDocPageSpec {
    relative_path: String,
    title: String,
    html: String,
    node_ids: Vec<String>,
    edge_keys: Vec<String>,
}

#[derive(Clone, Debug)]
struct SollMutationCandidate {
    id: String,
    entity_type: String,
    title: String,
    score: usize,
    reasons: Vec<String>,
}

#[derive(Clone, Debug)]
struct SollMutationInference {
    project_code: String,
    statement: String,
    candidate_entity_type: String,
    confidence: String,
    impacted_candidates: Vec<SollMutationCandidate>,
    target_ids: Vec<String>,
    ambiguity_warnings: Vec<String>,
    proposed_operation_kind: String,
}

#[derive(Clone, Debug)]
struct SollDerivedProjectEntry {
    project_code: String,
    project_name: String,
    project_path: String,
    node_count: usize,
    has_docs: bool,
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
struct RequirementCoverageSummary {
    done: usize,
    partial: usize,
    missing: usize,
    entries: Vec<RequirementCoverageEntry>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SollCompletenessSnapshot {
    project_scope: String,
    total_nodes: usize,
    orphan_requirements: Vec<String>,
    validations_without_verifies: Vec<String>,
    decisions_without_links: Vec<String>,
    uncovered_requirements: Vec<String>,
    duplicate_title_rows: Vec<Vec<String>>,
    duplicate_ids: Vec<String>,
    relation_policy_violations: Vec<String>,
    requirement_coverage: RequirementCoverageSummary,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkEndpointKind {
    Soll(&'static str),
    Artifact,
}

impl LinkEndpointKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Soll(prefix) => prefix,
            Self::Artifact => "ART",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ProjectionRole {
    Primary,
    Lateral,
    Supporting,
}

impl ProjectionRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Lateral => "lateral",
            Self::Supporting => "supporting",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct KindProjectionPolicy {
    breadcrumb_eligible: bool,
    root_eligible: bool,
    tree_order_rank: usize,
}

#[derive(Clone, Copy, Debug)]
struct RelationProjectionPolicy {
    role: ProjectionRole,
    parent_preference_rank: usize,
    child_order_rank: usize,
}

#[derive(Clone, Copy, Debug)]
struct RelationPolicy {
    allowed: &'static [&'static str],
    default: Option<&'static str>,
    allow_multiple_types: bool,
    projection: RelationProjectionPolicy,
}

const SOLL_RELATION_ENDPOINT_KINDS: &[&str] = &[
    "VIS", "PIL", "REQ", "CPT", "DEC", "MIL", "VAL", "STK", "GUI", "ART",
];

fn relation_table_name(_relation_type: &str) -> Option<&'static str> {
    Some("soll.Edge")
}

fn soll_entity_table_name(prefix: &str) -> Option<&'static str> {
    match prefix {
        "VIS" | "PIL" | "REQ" | "CPT" | "DEC" | "MIL" | "VAL" | "STK" | "GUI" => Some("soll.Node"),
        _ => None,
    }
}

fn relation_policy_for_pair(source_type: &str, target_type: &str) -> Option<RelationPolicy> {
    match (source_type, target_type) {
        ("PIL", "VIS") => Some(RelationPolicy {
            allowed: &["EPITOMIZES"],
            default: Some("EPITOMIZES"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 10,
                child_order_rank: 20,
            },
        }),
        ("REQ", "PIL") => Some(RelationPolicy {
            allowed: &["BELONGS_TO"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 10,
                child_order_rank: 30,
            },
        }),
        ("CPT", "REQ") => Some(RelationPolicy {
            allowed: &["EXPLAINS", "REFINES"],
            default: Some("EXPLAINS"),
            allow_multiple_types: true,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 40,
                child_order_rank: 70,
            },
        }),
        ("DEC", "REQ") => Some(RelationPolicy {
            allowed: &["SOLVES", "REFINES"],
            default: Some("SOLVES"),
            allow_multiple_types: true,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 10,
                child_order_rank: 40,
            },
        }),
        ("MIL", "REQ") => Some(RelationPolicy {
            allowed: &["TARGETS"],
            default: Some("TARGETS"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 50,
                child_order_rank: 80,
            },
        }),
        ("VAL", "REQ") => Some(RelationPolicy {
            allowed: &["VERIFIES"],
            default: Some("VERIFIES"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 20,
                child_order_rank: 50,
            },
        }),
        ("STK", "REQ") => Some(RelationPolicy {
            allowed: &["ORIGINATES", "CONTRIBUTES_TO"],
            default: Some("ORIGINATES"),
            allow_multiple_types: true,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 60,
                child_order_rank: 90,
            },
        }),
        ("GUI", "GUI") => Some(RelationPolicy {
            allowed: &["INHERITS_FROM"],
            default: Some("INHERITS_FROM"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 80,
                child_order_rank: 60,
            },
        }),
        ("REQ", "GUI") => Some(RelationPolicy {
            allowed: &["BELONGS_TO", "COMPLIES_WITH"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: true,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Supporting,
                parent_preference_rank: 90,
                child_order_rank: 999,
            },
        }),
        ("DEC", "GUI") => Some(RelationPolicy {
            allowed: &["COMPLIES_WITH"],
            default: Some("COMPLIES_WITH"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 90,
                child_order_rank: 999,
            },
        }),
        ("DEC", "DEC") => Some(RelationPolicy {
            allowed: &["SUPERSEDES", "REFINES"],
            default: None,
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 95,
                child_order_rank: 999,
            },
        }),
        ("REQ", "REQ") => Some(RelationPolicy {
            allowed: &["REFINES", "BELONGS_TO"],
            default: Some("REFINES"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 95,
                child_order_rank: 999,
            },
        }),
        ("MIL", "MIL") => Some(RelationPolicy {
            allowed: &["SUPERSEDES"],
            default: None,
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 95,
                child_order_rank: 999,
            },
        }),
        ("VAL", "VAL") => Some(RelationPolicy {
            allowed: &["REFINES", "SUPERSEDES"],
            default: None,
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 95,
                child_order_rank: 999,
            },
        }),
        ("DEC", "ART") => Some(RelationPolicy {
            allowed: &["IMPACTS", "SUBSTANTIATES"],
            default: Some("IMPACTS"),
            allow_multiple_types: true,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Supporting,
                parent_preference_rank: 100,
                child_order_rank: 999,
            },
        }),
        ("REQ", "ART") | ("VAL", "ART") => Some(RelationPolicy {
            allowed: &["SUBSTANTIATES"],
            default: Some("SUBSTANTIATES"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Supporting,
                parent_preference_rank: 100,
                child_order_rank: 999,
            },
        }),
        _ => None,
    }
}

fn kind_projection_policy(kind: &str) -> Option<KindProjectionPolicy> {
    match kind {
        "VIS" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: true,
            tree_order_rank: 10,
        }),
        "PIL" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: false,
            tree_order_rank: 20,
        }),
        "REQ" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: false,
            tree_order_rank: 30,
        }),
        "DEC" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: false,
            tree_order_rank: 40,
        }),
        "VAL" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: false,
            tree_order_rank: 50,
        }),
        "GUI" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: false,
            tree_order_rank: 60,
        }),
        "CPT" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: false,
            tree_order_rank: 70,
        }),
        "MIL" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: false,
            tree_order_rank: 80,
        }),
        "STK" => Some(KindProjectionPolicy {
            breadcrumb_eligible: true,
            root_eligible: false,
            tree_order_rank: 90,
        }),
        "ART" => Some(KindProjectionPolicy {
            breadcrumb_eligible: false,
            root_eligible: false,
            tree_order_rank: 100,
        }),
        _ => None,
    }
}

fn projection_metadata_payload(
    source_type: &str,
    target_type: &str,
    policy: RelationPolicy,
) -> Value {
    let source_projection = kind_projection_policy(source_type);
    let target_projection = kind_projection_policy(target_type);
    json!({
        "role": policy.projection.role.as_str(),
        "parent_preference_rank": policy.projection.parent_preference_rank,
        "child_order_rank": policy.projection.child_order_rank,
        "source_breadcrumb_eligible": source_projection.map(|value| value.breadcrumb_eligible),
        "target_breadcrumb_eligible": target_projection.map(|value| value.breadcrumb_eligible),
        "source_root_eligible": source_projection.map(|value| value.root_eligible),
        "target_root_eligible": target_projection.map(|value| value.root_eligible),
        "source_tree_order_rank": source_projection.map(|value| value.tree_order_rank),
        "target_tree_order_rank": target_projection.map(|value| value.tree_order_rank)
    })
}

fn relation_policy_payload(source_type: &str, target_type: &str) -> Value {
    if let Some(policy) = relation_policy_for_pair(source_type, target_type) {
        json!({
            "pair_allowed": true,
            "source_kind": source_type,
            "target_kind": target_type,
            "allowed_relations": policy.allowed,
            "default_relation": policy.default,
            "allow_multiple_types": policy.allow_multiple_types,
            "projection": projection_metadata_payload(source_type, target_type, policy),
            "guidance_source": "derived_from_relation_policy"
        })
    } else {
        json!({
            "pair_allowed": false,
            "source_kind": source_type,
            "target_kind": target_type,
            "allowed_relations": [],
            "default_relation": Value::Null,
            "allow_multiple_types": false,
            "projection": Value::Null,
            "guidance_source": "derived_from_relation_policy"
        })
    }
}

fn graph_role_for_kind(kind: &str) -> &'static str {
    match kind {
        "VIS" => "project north star",
        "PIL" => "structural pillar under a vision",
        "REQ" => "obligation that must be satisfied",
        "CPT" => "concept that explains or refines a requirement",
        "DEC" => "decision that solves, refines, or impacts implementation",
        "MIL" => "delivery checkpoint tied to a requirement",
        "VAL" => "evidence or proof that verifies a requirement",
        "STK" => "stakeholder source of demand or contribution",
        "GUI" => "guideline or policy constraint",
        "ART" => "implementation or runtime artifact",
        _ => "soll graph node",
    }
}

fn relation_example_sentence(source_kind: &str, target_kind: &str, relation_type: &str) -> String {
    format!(
        "A {} node typically uses `{}` to connect to a {} node.",
        source_kind, relation_type, target_kind
    )
}

fn allowed_relation_targets_from_source(source_type: &str) -> Vec<Value> {
    SOLL_RELATION_ENDPOINT_KINDS
        .iter()
        .filter_map(|target_type| {
            relation_policy_for_pair(source_type, target_type).map(|policy| {
                let default_relation = policy.default.unwrap_or(policy.allowed[0]);
                json!({
                    "source_kind": source_type,
                    "target_kind": target_type,
                    "allowed_relations": policy.allowed,
                    "default_relation": policy.default,
                    "allow_multiple_types": policy.allow_multiple_types,
                    "projection": projection_metadata_payload(source_type, target_type, policy),
                    "source_graph_role": graph_role_for_kind(source_type),
                    "target_graph_role": graph_role_for_kind(target_type),
                    "canonical_example": relation_example_sentence(source_type, target_type, default_relation),
                    "guidance_source": "derived_from_relation_policy"
                })
            })
        })
        .collect()
}

fn incoming_relation_sources_for_target(target_type: &str) -> Vec<Value> {
    SOLL_RELATION_ENDPOINT_KINDS
        .iter()
        .filter_map(|source_type| {
            relation_policy_for_pair(source_type, target_type).map(|policy| {
                let default_relation = policy.default.unwrap_or(policy.allowed[0]);
                json!({
                    "source_kind": source_type,
                    "target_kind": target_type,
                    "allowed_relations": policy.allowed,
                    "default_relation": policy.default,
                    "allow_multiple_types": policy.allow_multiple_types,
                    "projection": projection_metadata_payload(source_type, target_type, policy),
                    "source_graph_role": graph_role_for_kind(source_type),
                    "target_graph_role": graph_role_for_kind(target_type),
                    "canonical_example": relation_example_sentence(source_type, target_type, default_relation),
                    "guidance_source": "derived_from_relation_policy"
                })
            })
        })
        .collect()
}

fn repair_guidance_entry(
    category: &str,
    ids: &[String],
    summary: &str,
    next_steps: &[&str],
) -> Value {
    json!({
        "category": category,
        "summary": summary,
        "ids": ids,
        "next_steps": next_steps,
        "guidance_source": "server-side canonical soll validation"
    })
}

fn relation_scope_matches(source_id: &str, target_id: &str, project_code: Option<&str>) -> bool {
    match project_code {
        Some(code) => {
            let marker = format!("-{}-", code);
            source_id.contains(&marker) || target_id.contains(&marker)
        }
        None => true,
    }
}

fn recommendation_kind(node: &WorkPlanNode) -> &'static str {
    if node.descendants > 0 {
        "unblocker"
    } else if node
        .requirement_state
        .as_deref()
        .is_some_and(|state| matches!(state, "missing" | "partial"))
    {
        "proof_gap"
    } else if matches!(node.entity_type, WorkPlanEntityType::Milestone) {
        "checkpoint"
    } else {
        "task"
    }
}

fn recommendation_reason(node: &WorkPlanNode) -> String {
    if node.descendants > 0 {
        format!("debloque {} descendant(s)", node.descendants)
    } else if node
        .requirement_state
        .as_deref()
        .is_some_and(|state| matches!(state, "missing" | "partial"))
    {
        format!(
            "fermer le gap de preuve ({})",
            node.requirement_state.as_deref().unwrap_or("unknown")
        )
    } else if matches!(node.entity_type, WorkPlanEntityType::Milestone) {
        "jalon a cadrer ou rattacher".to_string()
    } else {
        node.reasons
            .first()
            .cloned()
            .unwrap_or_else(|| "action immediate".to_string())
    }
}

fn requirement_state_from(status: &str, criteria: &str, evidence_count: usize) -> &'static str {
    let has_criteria = !criteria.trim().is_empty() && criteria.trim() != "[]";
    if evidence_count > 0 && has_criteria && matches!(status, "current" | "accepted") {
        "done"
    } else if evidence_count > 0 || has_criteria {
        "partial"
    } else {
        "missing"
    }
}

fn normalize_traceability_entity_type(entity_type: &str) -> String {
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

fn project_code_from_canonical_entity_id(entity_id: &str) -> Option<String> {
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

fn tokenize_inference_text(input: &str) -> Vec<String> {
    input
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .map(|token| token.to_string())
        .collect()
}

fn preferred_entity_type_for_statement(statement: &str) -> &'static str {
    let lower = statement.to_ascii_lowercase();
    if ["constraint", "must", "should", "need", "requires", "rule"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "Requirement"
    } else if [
        "decision",
        "choose",
        "adopt",
        "use ",
        "switch",
        "architecture",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        "Decision"
    } else if ["guideline", "policy", "convention", "standard"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "Guideline"
    } else if ["concept", "means", "term", "definition"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "Concept"
    } else {
        "Requirement"
    }
}

fn canonical_blocker_ids(snapshot: &SollCompletenessSnapshot) -> BTreeSet<String> {
    snapshot.canonical_orphan_intent_ids()
}

fn build_adjacency_map(edges: &[(String, String)]) -> HashMap<String, BTreeSet<String>> {
    let mut adjacency: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (source, target) in edges {
        adjacency
            .entry(source.clone())
            .or_default()
            .insert(target.clone());
        adjacency.entry(target.clone()).or_default();
    }
    adjacency
}

fn detect_cycle_sets<'a, I>(
    node_ids: I,
    adjacency: &HashMap<String, BTreeSet<String>>,
) -> Vec<HashSet<String>>
where
    I: IntoIterator<Item = &'a String>,
{
    struct TarjanState {
        index: usize,
        indices: HashMap<String, usize>,
        lowlinks: HashMap<String, usize>,
        stack: Vec<String>,
        on_stack: HashSet<String>,
        components: Vec<HashSet<String>>,
    }

    fn strong_connect(
        node: &str,
        adjacency: &HashMap<String, BTreeSet<String>>,
        state: &mut TarjanState,
    ) {
        let current_index = state.index;
        state.indices.insert(node.to_string(), current_index);
        state.lowlinks.insert(node.to_string(), current_index);
        state.index += 1;
        state.stack.push(node.to_string());
        state.on_stack.insert(node.to_string());

        if let Some(neighbors) = adjacency.get(node) {
            for neighbor in neighbors {
                if !state.indices.contains_key(neighbor) {
                    strong_connect(neighbor, adjacency, state);
                    let neighbor_low = *state.lowlinks.get(neighbor).unwrap_or(&current_index);
                    if let Some(low) = state.lowlinks.get_mut(node) {
                        *low = (*low).min(neighbor_low);
                    }
                } else if state.on_stack.contains(neighbor) {
                    let neighbor_index = *state.indices.get(neighbor).unwrap_or(&current_index);
                    if let Some(low) = state.lowlinks.get_mut(node) {
                        *low = (*low).min(neighbor_index);
                    }
                }
            }
        }

        if state.indices.get(node) == state.lowlinks.get(node) {
            let mut component = HashSet::new();
            while let Some(member) = state.stack.pop() {
                state.on_stack.remove(&member);
                component.insert(member.clone());
                if member == node {
                    break;
                }
            }

            let is_cycle = if component.len() > 1 {
                true
            } else {
                component.iter().next().is_some_and(|single| {
                    adjacency
                        .get(single)
                        .is_some_and(|neighbors| neighbors.contains(single))
                })
            };
            if is_cycle {
                state.components.push(component);
            }
        }
    }

    let mut state = TarjanState {
        index: 0,
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        stack: Vec::new(),
        on_stack: HashSet::new(),
        components: Vec::new(),
    };

    let mut ordered_ids = node_ids.into_iter().cloned().collect::<Vec<_>>();
    ordered_ids.sort();
    for node in ordered_ids {
        if !state.indices.contains_key(&node) {
            strong_connect(&node, adjacency, &mut state);
        }
    }

    state.components
}

fn collect_blocked_by_cycles(
    adjacency: &HashMap<String, BTreeSet<String>>,
    cycle_node_ids: &HashSet<String>,
) -> HashSet<String> {
    let mut blocked = HashSet::new();
    let mut queue = cycle_node_ids.iter().cloned().collect::<VecDeque<_>>();
    while let Some(node) = queue.pop_front() {
        if let Some(children) = adjacency.get(&node) {
            for child in children {
                if cycle_node_ids.contains(child) || !blocked.insert(child.clone()) {
                    continue;
                }
                queue.push_back(child.clone());
            }
        }
    }
    blocked
}

fn filter_adjacency(
    adjacency: &HashMap<String, BTreeSet<String>>,
    allowed_ids: &HashSet<String>,
) -> HashMap<String, BTreeSet<String>> {
    let mut filtered = HashMap::new();
    for id in allowed_ids {
        let neighbors = adjacency
            .get(id)
            .map(|items| {
                items
                    .iter()
                    .filter(|child| allowed_ids.contains(*child))
                    .cloned()
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        filtered.insert(id.clone(), neighbors);
    }
    filtered
}

fn compute_descendant_counts(
    schedulable_ids: &HashSet<String>,
    adjacency: &HashMap<String, BTreeSet<String>>,
) -> HashMap<String, usize> {
    let mut descendants = HashMap::new();
    let mut ordered_ids = schedulable_ids.iter().cloned().collect::<Vec<_>>();
    ordered_ids.sort();
    for node_id in ordered_ids {
        let mut seen = HashSet::new();
        let mut stack = adjacency
            .get(&node_id)
            .map(|children| children.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        while let Some(next) = stack.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            if let Some(children) = adjacency.get(&next) {
                stack.extend(children.iter().cloned());
            }
        }
        descendants.insert(node_id, seen.len());
    }
    descendants
}

fn score_node(node: &WorkPlanNode, include_ist: bool) -> (i64, Vec<String>, Vec<String>) {
    let mut score = (node.descendants as i64) * 40;
    let mut reasons = vec![format!("debloque {} descendant(s)", node.descendants)];
    let mut validation_gates = Vec::new();

    match node.priority.as_str() {
        "P0" => {
            score += 20;
            reasons.push("priorite P0".to_string());
        }
        "P1" => {
            score += 15;
            reasons.push("priorite P1".to_string());
        }
        "P2" => {
            score += 8;
            reasons.push("priorite P2".to_string());
        }
        _ => {}
    }

    if let Some(state) = node.requirement_state.as_deref() {
        match state {
            "missing" => {
                score += 15;
                reasons.push("requirement missing".to_string());
                validation_gates.push("define acceptance criteria and evidence".to_string());
            }
            "partial" => {
                score += 8;
                reasons.push("requirement partial".to_string());
                validation_gates.push("complete missing proof or acceptance criteria".to_string());
            }
            _ => {}
        }
    }

    if node.evidence_count == 0 {
        score += 10;
        reasons.push("aucune evidence rattachee".to_string());
        validation_gates.push("attach evidence".to_string());
    }

    if include_ist && node.ist_degraded_links > 0 {
        score += 8;
        reasons.push("scope IST degrade".to_string());
        validation_gates.push("reindex degraded scope".to_string());
    }

    if node.backlog_visible {
        score += 5;
        reasons.push("backlog visible sur le projet".to_string());
        validation_gates.push("reduce project backlog before closure".to_string());
    }

    if matches!(node.entity_type, WorkPlanEntityType::Milestone) && node.descendants == 0 {
        score -= 10;
        reasons.push("milestone isole".to_string());
    }

    (score, reasons, validation_gates)
}

fn build_waves(
    nodes: &HashMap<String, WorkPlanNode>,
    edges: &[(String, String)],
    schedulable_ids: &HashSet<String>,
) -> Vec<WorkPlanWave> {
    let mut indegree = schedulable_ids
        .iter()
        .map(|id| (id.clone(), 0usize))
        .collect::<HashMap<_, _>>();
    let mut adjacency = HashMap::<String, Vec<String>>::new();

    for (source, target) in edges {
        if !schedulable_ids.contains(source) || !schedulable_ids.contains(target) {
            continue;
        }
        adjacency
            .entry(source.clone())
            .or_default()
            .push(target.clone());
        *indegree.entry(target.clone()).or_insert(0) += 1;
        indegree.entry(source.clone()).or_insert(0);
    }

    let mut ready = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    ready.sort();

    let mut waves = Vec::new();
    let mut wave_index = 1usize;
    while !ready.is_empty() {
        let mut current_ids = std::mem::take(&mut ready);
        current_ids.sort();
        let mut items = current_ids
            .iter()
            .filter_map(|id| nodes.get(id).cloned())
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| right.descendants.cmp(&left.descendants))
                .then_with(|| {
                    left.entity_type
                        .sort_rank()
                        .cmp(&right.entity_type.sort_rank())
                })
                .then_with(|| left.id.cmp(&right.id))
        });
        waves.push(WorkPlanWave { wave_index, items });
        wave_index += 1;

        let mut next_ready = BTreeSet::new();
        for current_id in current_ids {
            if let Some(children) = adjacency.get(&current_id) {
                for child in children {
                    if let Some(entry) = indegree.get_mut(child) {
                        *entry = entry.saturating_sub(1);
                        if *entry == 0 {
                            next_ready.insert(child.clone());
                        }
                    }
                }
            }
            indegree.remove(&current_id);
        }
        ready = next_ready.into_iter().collect();
    }

    waves
}

fn apply_wave_limit(waves: &[WorkPlanWave], limit: usize) -> (Vec<WorkPlanWave>, usize, bool) {
    let mut remaining = limit;
    let mut returned_items = 0usize;
    let mut limited = Vec::new();
    for wave in waves {
        if remaining == 0 {
            break;
        }
        if wave.items.len() <= remaining {
            returned_items += wave.items.len();
            remaining -= wave.items.len();
            limited.push(wave.clone());
            continue;
        }
        let items = wave.items[..remaining].to_vec();
        returned_items += items.len();
        limited.push(WorkPlanWave {
            wave_index: wave.wave_index,
            items,
        });
        remaining = 0;
    }

    let total_items = waves.iter().map(|wave| wave.items.len()).sum::<usize>();
    (limited, returned_items, returned_items < total_items)
}

fn blocker_to_json(blocker: &WorkPlanBlocker) -> Value {
    json!({
        "id": blocker.id,
        "entity_type": blocker.entity_type,
        "reason": blocker.reason
    })
}

fn cycle_to_json(cycle: &WorkPlanCycle) -> Value {
    json!({
        "node_ids": cycle.node_ids
    })
}

fn wave_to_json(wave: &WorkPlanWave) -> Value {
    json!({
        "wave_index": wave.wave_index,
        "items": wave.items.iter().map(|item| {
            json!({
                "id": item.id,
                "entity_type": item.entity_type.label(),
                "title": item.title,
                "score": item.score,
                "reasons": item.reasons,
                "validation_gates": item.validation_gates,
                "ist_signals": item.ist_signals
            })
        }).collect::<Vec<_>>()
    })
}

impl McpServer {
    fn canonical_next_link_hints(&self, entity_type_cap: &str) -> Vec<Value> {
        let outgoing = allowed_relation_targets_from_source(entity_type_cap);
        if !outgoing.is_empty() {
            outgoing
        } else {
            incoming_relation_sources_for_target(entity_type_cap)
        }
    }

    fn derive_next_best_actions_from_snapshot(
        &self,
        snapshot: &SollCompletenessSnapshot,
    ) -> Vec<String> {
        if !snapshot.orphan_requirements.is_empty() {
            vec![
                "link each orphan requirement to its pillar or guideline".to_string(),
                "use `soll_relation_schema` before retrying if canonical edges are unclear"
                    .to_string(),
            ]
        } else if !snapshot.validations_without_verifies.is_empty() {
            vec![
                "attach each validation to a requirement with `VERIFIES`".to_string(),
                "rerun `soll_validate` after adding the missing proof links".to_string(),
            ]
        } else if !snapshot.uncovered_requirements.is_empty() {
            vec![
                "add acceptance criteria or evidence to uncovered requirements".to_string(),
                "use `soll_attach_evidence` or update requirement metadata".to_string(),
            ]
        } else {
            vec![
                "rerun `soll_work_plan` to open the next delivery wave".to_string(),
                "use `soll_verify_requirements` for requirement-level proof status".to_string(),
            ]
        }
    }

    fn mutation_feedback_payload(
        &self,
        before: &SollCompletenessSnapshot,
        after: &SollCompletenessSnapshot,
        changed_entities: Vec<Value>,
        topology_delta: Value,
    ) -> Value {
        let before_blockers = canonical_blocker_ids(before);
        let after_blockers = canonical_blocker_ids(after);
        let newly_unblocked = before_blockers
            .difference(&after_blockers)
            .cloned()
            .collect::<Vec<_>>();
        let remaining_blockers = after_blockers.into_iter().collect::<Vec<_>>();

        json!({
            "changed_entities": changed_entities,
            "topology_delta": topology_delta,
            "newly_unblocked": newly_unblocked,
            "remaining_blockers": remaining_blockers,
            "next_best_actions": self.derive_next_best_actions_from_snapshot(after),
            "completeness_before": {
                "concept_completeness": before.concept_complete(),
                "implementation_completeness": before.implementation_complete(),
                "structurally_connected": before.structurally_connected(),
                "evidence_ready": before.evidence_ready(),
                "duplicate_free": before.duplicate_free()
            },
            "completeness_after": {
                "concept_completeness": after.concept_complete(),
                "implementation_completeness": after.implementation_complete(),
                "structurally_connected": after.structurally_connected(),
                "evidence_ready": after.evidence_ready(),
                "duplicate_free": after.duplicate_free()
            },
            "guidance_source": "server-side canonical soll mutation feedback"
        })
    }

    fn infer_soll_mutation_internal(
        &self,
        project_code: &str,
        statement: &str,
    ) -> anyhow::Result<SollMutationInference> {
        let project_code = self.resolve_project_code(project_code)?;
        let preferred_type = preferred_entity_type_for_statement(statement);
        let tokens = tokenize_inference_text(statement);
        let rows_raw = self.graph_store.query_json(&format!(
            "SELECT id, type, COALESCE(title,''), COALESCE(description,'')
             FROM soll.Node
             WHERE project_code = '{}'
             ORDER BY type, id",
            escape_sql(&project_code)
        ))?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();

        let mut candidates = rows
            .into_iter()
            .filter(|row| row.len() >= 4)
            .map(|row| {
                let haystack = format!("{} {}", row[2], row[3]).to_ascii_lowercase();
                let token_hits = tokens
                    .iter()
                    .filter(|token| haystack.contains(token.as_str()))
                    .count();
                let type_bonus = usize::from(row[1] == preferred_type) * 2;
                let score = token_hits + type_bonus;
                let mut reasons = Vec::new();
                if token_hits > 0 {
                    reasons.push(format!("matched {} statement token(s)", token_hits));
                }
                if row[1] == preferred_type {
                    reasons.push(format!("preferred entity type `{}`", preferred_type));
                }
                SollMutationCandidate {
                    id: row[0].clone(),
                    entity_type: row[1].clone(),
                    title: row[2].clone(),
                    score,
                    reasons,
                }
            })
            .filter(|candidate| candidate.score > 0)
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.id.cmp(&right.id))
        });
        candidates.truncate(5);

        let target_ids = candidates
            .iter()
            .take(2)
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let confidence = if candidates
            .first()
            .is_some_and(|candidate| candidate.score >= 4)
        {
            "high"
        } else if !candidates.is_empty() {
            "medium"
        } else {
            "low"
        };
        let mut ambiguity_warnings = Vec::new();
        if candidates.is_empty() {
            ambiguity_warnings.push(
                "No existing canonical nodes matched strongly; wave 1 entrenchment will not create new entities automatically.".to_string(),
            );
        } else if candidates.len() > 1
            && candidates
                .get(1)
                .is_some_and(|candidate| candidate.score == candidates[0].score)
        {
            ambiguity_warnings
                .push("Multiple candidate nodes scored equally; confirm target_ids explicitly before write mode.".to_string());
        }

        Ok(SollMutationInference {
            project_code,
            statement: statement.to_string(),
            candidate_entity_type: preferred_type.to_string(),
            confidence: confidence.to_string(),
            impacted_candidates: candidates.clone(),
            target_ids,
            ambiguity_warnings,
            proposed_operation_kind: if candidates.is_empty() {
                "needs_manual_scope".to_string()
            } else {
                "update_existing_entities".to_string()
            },
        })
    }

    pub(crate) fn axon_infer_soll_mutation(&self, args: &Value) -> Option<Value> {
        let project_code = args.get("project_code").and_then(|value| value.as_str())?;
        let statement = args.get("statement").and_then(|value| value.as_str())?;
        match self.infer_soll_mutation_internal(project_code, statement) {
            Ok(inference) => Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "Assistive SOLL inference for `{}` suggests `{}` with {} impacted candidate(s).",
                        inference.project_code,
                        inference.proposed_operation_kind,
                        inference.impacted_candidates.len()
                    )
                }],
                "data": {
                    "project_code": inference.project_code,
                    "statement": inference.statement,
                    "candidate_entity_type": inference.candidate_entity_type,
                    "proposed_operation_kind": inference.proposed_operation_kind,
                    "confidence": inference.confidence,
                    "target_ids": inference.target_ids,
                    "impacted_candidates": inference.impacted_candidates.iter().map(|candidate| json!({
                        "id": candidate.id,
                        "entity_type": candidate.entity_type,
                        "title": candidate.title,
                        "score": candidate.score,
                        "reasons": candidate.reasons
                    })).collect::<Vec<_>>(),
                    "ambiguity_warnings": inference.ambiguity_warnings,
                    "next_best_actions": if inference.impacted_candidates.is_empty() {
                        vec![
                            "inspect the current SOLL context and choose explicit target_ids".to_string(),
                            "create or update canonical nodes manually with `soll_manager` if the nuance truly requires a new entity".to_string()
                        ]
                    } else {
                        vec![
                            "confirm the target_ids and call `entrench_nuance` with `confirm=true`".to_string(),
                            "override target_ids explicitly if the proposed scope is too broad".to_string()
                        ]
                    }
                }
            })),
            Err(error) => Some(json!({
                "content": [{ "type": "text", "text": format!("Inference failed: {}", error) }],
                "isError": true
            })),
        }
    }

    pub(crate) fn axon_entrench_nuance(&self, args: &Value) -> Option<Value> {
        let project_code = args.get("project_code").and_then(|value| value.as_str())?;
        let statement = args.get("statement").and_then(|value| value.as_str())?;
        let confirm = args
            .get("confirm")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        let inference = match self.infer_soll_mutation_internal(project_code, statement) {
            Ok(inference) => inference,
            Err(error) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Entrenchment failed: {}", error) }],
                    "isError": true
                }))
            }
        };

        let explicit_target_ids = args
            .get("target_ids")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let target_ids = if explicit_target_ids.is_empty() {
            inference.target_ids.clone()
        } else {
            explicit_target_ids.clone()
        };

        if !confirm {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": "Entrenchment proposal only. Re-run with `confirm=true` to apply bounded updates on the selected canonical entities."
                }],
                "data": {
                    "project_code": inference.project_code,
                    "statement": inference.statement,
                    "confirm_required": true,
                    "candidate_entity_type": inference.candidate_entity_type,
                    "proposed_operation_kind": inference.proposed_operation_kind,
                    "target_ids": target_ids,
                    "impacted_candidates": inference.impacted_candidates.iter().map(|candidate| json!({
                        "id": candidate.id,
                        "entity_type": candidate.entity_type,
                        "title": candidate.title,
                        "score": candidate.score,
                        "reasons": candidate.reasons
                    })).collect::<Vec<_>>(),
                    "ambiguity_warnings": inference.ambiguity_warnings,
                    "next_best_actions": vec![
                        "review the proposed target_ids".to_string(),
                        "rerun with `confirm=true` once the scope is explicit".to_string()
                    ]
                }
            }));
        }

        if target_ids.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "Wave 1 entrenchment cannot write without explicit or inferred existing target_ids." }],
                "isError": true,
                "data": {
                    "project_code": inference.project_code,
                    "confirm_required": false,
                    "target_ids": [],
                    "next_best_actions": [
                        "call `infer_soll_mutation` to inspect impacted nodes",
                        "provide `target_ids` explicitly or use `soll_manager` for manual graph changes"
                    ]
                }
            }));
        }

        if explicit_target_ids.is_empty() && !inference.ambiguity_warnings.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "Entrenchment confirmation refused because the inferred scope is still ambiguous. Provide explicit `target_ids` first." }],
                "isError": true,
                "data": {
                    "project_code": inference.project_code,
                    "confirm_required": false,
                    "target_ids": target_ids,
                    "ambiguity_warnings": inference.ambiguity_warnings,
                    "next_best_actions": [
                        "review the impacted_candidates returned by `infer_soll_mutation`",
                        "rerun `entrench_nuance` with explicit `target_ids` once the scope is fully explicit"
                    ]
                }
            }));
        }

        let cross_project_targets = target_ids
            .iter()
            .filter(|target_id| {
                project_code_from_canonical_entity_id(target_id)
                    .is_none_or(|candidate_project| candidate_project != inference.project_code)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !cross_project_targets.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "Entrenchment confirmation refused because some target_ids do not belong to the requested project_code." }],
                "isError": true,
                "data": {
                    "project_code": inference.project_code,
                    "confirm_required": false,
                    "target_ids": target_ids,
                    "invalid_target_ids": cross_project_targets,
                    "next_best_actions": [
                        "use only canonical IDs that belong to the requested project_code",
                        "re-run `infer_soll_mutation` if the intended scope is uncertain"
                    ]
                }
            }));
        }

        let before = match self.soll_completeness_snapshot(Some(&inference.project_code)) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Entrenchment baseline failed: {}", error) }],
                    "isError": true
                }))
            }
        };

        let mut changed_entities = Vec::new();
        for target_id in &target_ids {
            let row = match self.query_named_row(
                &format!(
                    "SELECT title, description, status, metadata FROM soll.Node WHERE id = '{}'",
                    escape_sql(target_id)
                ),
                4,
            ) {
                Ok(row) => row,
                Err(error) => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!("Entrenchment target lookup failed for `{}`: {}", target_id, error) }],
                        "isError": true
                    }))
                }
            };
            let mut metadata: Value = serde_json::from_str(&row[3]).unwrap_or(json!({}));
            let entry = json!({
                "statement": statement,
                "source": "entrench_nuance",
                "entrenched_at": now_unix_ms()
            });
            if !metadata
                .get("nuances")
                .is_some_and(|value| value.is_array())
            {
                metadata["nuances"] = json!([]);
            }
            if let Some(items) = metadata
                .get_mut("nuances")
                .and_then(|value| value.as_array_mut())
            {
                items.push(entry);
            }
            metadata["updated_at"] = json!(now_unix_ms());

            if let Err(error) = self.graph_store.execute_param(
                "UPDATE soll.Node SET metadata = ? WHERE id = ?",
                &json!([metadata.to_string(), target_id]),
            ) {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Entrenchment update failed for `{}`: {}", target_id, error) }],
                    "isError": true
                }));
            }

            changed_entities.push(json!({
                "id": target_id,
                "change_kind": "metadata_update",
                "fields": ["metadata.nuances", "metadata.updated_at"]
            }));
        }

        let after = match self.soll_completeness_snapshot(Some(&inference.project_code)) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Entrenchment follow-up failed: {}", error) }],
                    "isError": true
                }))
            }
        };

        let mutation_feedback = self.mutation_feedback_payload(
            &before,
            &after,
            changed_entities.clone(),
            json!({
                "nodes_created": 0,
                "nodes_updated": changed_entities.len(),
                "edges_created": 0
            }),
        );

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Nuance entrenched across {} canonical node(s).",
                    changed_entities.len()
                )
            }],
            "data": {
                "project_code": inference.project_code,
                "statement": inference.statement,
                "confirm_required": false,
                "target_ids": target_ids,
                "mutation_feedback": mutation_feedback
            }
        }))
    }

    fn classify_attach_status_from_error(&self, error_text: &str) -> &'static str {
        if error_text.contains("Relation explicite requise") {
            "needs_relation_hint"
        } else if error_text.contains("introuvable") {
            "invalid_target_id"
        } else if error_text.contains("Aucune relation canonique autorisee") {
            "invalid_target_kind"
        } else {
            "attach_failed"
        }
    }

    pub(crate) fn axon_soll_manager(&self, args: &Value) -> Option<Value> {
        let action = args.get("action")?.as_str()?;
        let entity = args.get("entity")?.as_str()?;
        let data = args.get("data")?;

        match action {
            "create" => {
                let project_code_raw = args
                    .get("project_code")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("project_code").and_then(|v| v.as_str()))
                    .map(str::trim);
                let project_code = match self.require_registered_mutation_project_code(
                    project_code_raw,
                    "soll_manager create",
                ) {
                    Ok(code) => code,
                    Err(e) => {
                        return Some(json!({
                            "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                            "isError": true
                        }))
                    }
                };
                let before_snapshot = self.soll_completeness_snapshot(Some(&project_code)).ok();
                let reserved_id = args.get("reserved_id").and_then(|value| value.as_str());
                let (_requested_code, canonical_code, formatted_id) = if let Some(reserved_id) =
                    reserved_id
                {
                    match self.resolve_canonical_project_identity_for_mutation(&project_code) {
                        Ok((canonical_code, project_code)) => {
                            (canonical_code, project_code, reserved_id.to_string())
                        }
                        Err(e) => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Erreur registre: {}", e) }], "isError": true }),
                            )
                        }
                    }
                } else {
                    match self.next_soll_numeric_id(&project_code, entity) {
                        Ok((canonical_code, project_code, prefix, next_num)) => (
                            canonical_code,
                            project_code.clone(),
                            format!("{}-{}-{:03}", prefix, project_code, next_num),
                        ),
                        Err(e) => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Erreur registre: {}", e) }], "isError": true }),
                            )
                        }
                    }
                };

                let mut meta = data.get("metadata").cloned().unwrap_or(json!({}));
                let title = data
                    .get("title")
                    .or_else(|| data.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let description = data
                    .get("description")
                    .or_else(|| data.get("explanation"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let status = data.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let status = if entity == "validation" {
                    data.get("result")
                        .and_then(|v| v.as_str())
                        .unwrap_or(status)
                } else {
                    status
                };

                if let Some(goal) = data.get("goal") {
                    meta["goal"] = goal.clone();
                }
                if let Some(priority) = data.get("priority") {
                    meta["priority"] = priority.clone();
                }
                if let Some(owner) = data.get("owner") {
                    meta["owner"] = owner.clone();
                }
                if let Some(ac) = data.get("acceptance_criteria") {
                    meta["acceptance_criteria"] = ac.clone();
                }
                if let Some(er) = data.get("evidence_refs") {
                    meta["evidence_refs"] = er.clone();
                }
                if let Some(rat) = data.get("rationale") {
                    meta["rationale"] = rat.clone();
                }
                if let Some(ctx) = data.get("context") {
                    meta["context"] = ctx.clone();
                }
                if let Some(sup) = data.get("supersedes_decision_id") {
                    meta["supersedes_decision_id"] = sup.clone();
                }
                if let Some(imp) = data.get("impact_scope") {
                    meta["impact_scope"] = imp.clone();
                }
                if let Some(role) = data.get("role") {
                    meta["role"] = role.clone();
                }
                if let Some(method) = data.get("method") {
                    meta["method"] = method.clone();
                }
                if let Some(result) = data.get("result") {
                    meta["result"] = result.clone();
                }

                meta["updated_at"] = json!(now_unix_ms());

                let entity_type_cap = match entity {
                    "vision" => "Vision",
                    "pillar" => "Pillar",
                    "requirement" => "Requirement",
                    "concept" => "Concept",
                    "decision" => "Decision",
                    "milestone" => "Milestone",
                    "stakeholder" => "Stakeholder",
                    "validation" => "Validation",
                    _ => {
                        return Some(
                            json!({ "content": [{ "type": "text", "text": "Unknown entity" }], "isError": true }),
                        )
                    }
                };

                let q = "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET project_code = EXCLUDED.project_code, title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata";
                let attach_to = data.get("attach_to").and_then(|v| v.as_str());
                let relation_hint = data.get("relation_hint").and_then(|v| v.as_str());

                let insert_res = self.graph_store.execute_param(
                    q,
                    &json!([
                        formatted_id,
                        entity_type_cap,
                        canonical_code,
                        title,
                        description,
                        status,
                        meta.to_string()
                    ]),
                );

                match insert_res {
                    Ok(_) => {
                        let created_id = formatted_id.clone();
                        let mut report = format!("✅ Entité SOLL créée : `{}`", created_id);
                        let mut response_data = json!({
                            "created_id": created_id,
                            "entity_type": entity_type_cap,
                            "project_code": canonical_code,
                            "canonical_next_links": self.canonical_next_link_hints(entity_type_cap),
                            "attach_attempted": attach_to.is_some(),
                            "attached": false,
                            "attached_to": attach_to.map(Value::from).unwrap_or(Value::Null),
                            "applied_relation": Value::Null,
                            "attach_status": if attach_to.is_some() { Value::from("not_attempted") } else { Value::Null }
                        });

                        if let Some(target_id) = attach_to {
                            match self.select_relation_type_for_link(
                                &formatted_id,
                                target_id,
                                relation_hint,
                            ) {
                                Ok((relation_type, policy)) => {
                                    match self.insert_validated_relation(
                                        relation_type,
                                        &formatted_id,
                                        target_id,
                                        policy,
                                    ) {
                                        Ok(inserted) => {
                                            response_data["attached"] = Value::from(true);
                                            response_data["attached_to"] = Value::from(target_id);
                                            response_data["applied_relation"] =
                                                Value::from(relation_type);
                                            response_data["attach_status"] =
                                                Value::from(if inserted {
                                                    "attached"
                                                } else {
                                                    "already_present"
                                                });
                                            report.push_str(&format!(
                                                "\n✅ Liaison canonique appliquée : `{}` -> `{}` via `{}`",
                                                formatted_id, target_id, relation_type
                                            ));
                                        }
                                        Err(error) => {
                                            let error_text = error.to_string();
                                            response_data["attach_status"] = Value::from(
                                                self.classify_attach_status_from_error(&error_text),
                                            );
                                            response_data["attach_guidance"] = self
                                                .relation_guidance_for_link(
                                                    &formatted_id,
                                                    target_id,
                                                    relation_hint,
                                                );
                                            report.push_str(&format!(
                                                "\n⚠️ Attachement canonique refusé : {}",
                                                error_text
                                            ));
                                        }
                                    }
                                }
                                Err(error) => {
                                    let error_text = error.to_string();
                                    response_data["attach_status"] = Value::from(
                                        self.classify_attach_status_from_error(&error_text),
                                    );
                                    response_data["attach_guidance"] = self
                                        .relation_guidance_for_link(
                                            &formatted_id,
                                            target_id,
                                            relation_hint,
                                        );
                                    report.push_str(&format!(
                                        "\n⚠️ Attachement canonique refusé : {}",
                                        error_text
                                    ));
                                }
                            }
                        }

                        if let (Some(before), Ok(after)) = (
                            before_snapshot.as_ref(),
                            self.soll_completeness_snapshot(Some(&canonical_code)),
                        ) {
                            response_data["mutation_feedback"] = self.mutation_feedback_payload(
                                before,
                                &after,
                                vec![json!({
                                    "id": formatted_id,
                                    "change_kind": "created",
                                    "entity_type": entity_type_cap
                                })],
                                json!({
                                    "nodes_created": 1,
                                    "nodes_updated": 0,
                                    "edges_created": usize::from(response_data["attached"].as_bool().unwrap_or(false))
                                }),
                            );
                        }

                        Some(json!({
                            "content": [{ "type": "text", "text": report }],
                            "data": response_data
                        }))
                    }
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur d'insertion: {}", e) }], "isError": true }),
                    ),
                }
            }
            "update" => {
                let id = data.get("id")?.as_str()?;
                let project_code = project_code_from_canonical_entity_id(id);
                let before_snapshot = project_code
                    .as_deref()
                    .and_then(|code| self.soll_completeness_snapshot(Some(code)).ok());

                let update_res: anyhow::Result<()> = (|| {
                    let current = self.query_named_row(
                        &format!("SELECT title, description, status, metadata FROM soll.Node WHERE id = '{}'", escape_sql(id)),
                        4,
                    )?;
                    let mut meta: Value = serde_json::from_str(&current[3]).unwrap_or(json!({}));

                    let title = data
                        .get("title")
                        .or_else(|| data.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[0]);
                    let description = data
                        .get("description")
                        .or_else(|| data.get("explanation"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[1]);
                    let status = data
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[2]);

                    if let Some(m) = data.get("metadata") {
                        if let Some(obj) = m.as_object() {
                            for (k, v) in obj {
                                meta[k] = v.clone();
                            }
                        }
                    }
                    if let Some(goal) = data.get("goal") {
                        meta["goal"] = goal.clone();
                    }
                    if let Some(priority) = data.get("priority") {
                        meta["priority"] = priority.clone();
                    }
                    if let Some(owner) = data.get("owner") {
                        meta["owner"] = owner.clone();
                    }
                    if let Some(ac) = data.get("acceptance_criteria") {
                        meta["acceptance_criteria"] = ac.clone();
                    }
                    if let Some(er) = data.get("evidence_refs") {
                        meta["evidence_refs"] = er.clone();
                    }
                    if let Some(rat) = data.get("rationale") {
                        meta["rationale"] = rat.clone();
                    }
                    if let Some(ctx) = data.get("context") {
                        meta["context"] = ctx.clone();
                    }
                    if let Some(sup) = data.get("supersedes_decision_id") {
                        meta["supersedes_decision_id"] = sup.clone();
                    }
                    if let Some(imp) = data.get("impact_scope") {
                        meta["impact_scope"] = imp.clone();
                    }
                    if let Some(role) = data.get("role") {
                        meta["role"] = role.clone();
                    }
                    if let Some(method) = data.get("method") {
                        meta["method"] = method.clone();
                    }
                    if let Some(result) = data.get("result") {
                        meta["result"] = result.clone();
                    }

                    meta["updated_at"] = json!(now_unix_ms());

                    let q = "UPDATE soll.Node SET title = ?, description = ?, status = ?, metadata = ? WHERE id = ?";
                    self.graph_store.execute_param(
                        q,
                        &json!([title, description, status, meta.to_string(), id]),
                    )
                })();

                match update_res {
                    Ok(_) => {
                        let mut payload = json!({
                            "content": [{ "type": "text", "text": format!("✅ Mise à jour réussie pour `{}`", id) }],
                            "data": {}
                        });
                        if let (Some(code), Some(before), Ok(after)) = (
                            project_code.as_deref(),
                            before_snapshot.as_ref(),
                            project_code
                                .as_deref()
                                .ok_or_else(|| anyhow!("missing project"))
                                .and_then(|value| self.soll_completeness_snapshot(Some(value))),
                        ) {
                            let _ = code;
                            payload["data"]["mutation_feedback"] = self.mutation_feedback_payload(
                                before,
                                &after,
                                vec![json!({
                                    "id": id,
                                    "change_kind": "updated",
                                    "fields": ["title", "description", "status", "metadata"]
                                })],
                                json!({
                                    "nodes_created": 0,
                                    "nodes_updated": 1,
                                    "edges_created": 0
                                }),
                            );
                        }
                        Some(payload)
                    }
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur update: {}", e) }], "isError": true }),
                    ),
                }
            }
            "link" => {
                let src = data.get("source_id")?.as_str()?;
                let tgt = data.get("target_id")?.as_str()?;
                let explicit_rel = data.get("relation_type").and_then(|v| v.as_str());
                let project_code = project_code_from_canonical_entity_id(src)
                    .or_else(|| project_code_from_canonical_entity_id(tgt));
                let before_snapshot = project_code
                    .as_deref()
                    .and_then(|code| self.soll_completeness_snapshot(Some(code)).ok());
                match self.select_relation_type_for_link(src, tgt, explicit_rel) {
                    Ok((relation_type, policy)) => {
                        let rel_table = relation_table_name(relation_type).unwrap_or(relation_type);
                        match self.insert_validated_relation(relation_type, src, tgt, policy) {
                            Ok(inserted) => {
                                let mut payload = json!({
                                    "content": [{ "type": "text", "text": if inserted {
                                        format!("✅ Liaison établie : `{}` -> `{}` (via {})", src, tgt, rel_table)
                                    } else {
                                        format!("ℹ️ Liaison déjà présente : `{}` -> `{}` (via {})", src, tgt, rel_table)
                                    }}],
                                    "data": {}
                                });
                                if inserted {
                                    if let (Some(before), Some(code), Ok(after)) = (
                                        before_snapshot.as_ref(),
                                        project_code.as_deref(),
                                        project_code
                                            .as_deref()
                                            .ok_or_else(|| anyhow!("missing project"))
                                            .and_then(|value| {
                                                self.soll_completeness_snapshot(Some(value))
                                            }),
                                    ) {
                                        let _ = code;
                                        payload["data"]["mutation_feedback"] =
                                            self.mutation_feedback_payload(
                                                before,
                                                &after,
                                                vec![json!({
                                                    "id": format!("{}:{}:{}", src, relation_type, tgt),
                                                    "change_kind": "edge_created",
                                                    "source_id": src,
                                                    "target_id": tgt,
                                                    "relation_type": relation_type
                                                })],
                                                json!({
                                                    "nodes_created": 0,
                                                    "nodes_updated": 0,
                                                    "edges_created": 1
                                                }),
                                            );
                                    }
                                }
                                Some(payload)
                            }
                            Err(e) => Some(json!({
                                "content": [{ "type": "text", "text": format!("Erreur liaison: {}", e) }],
                                "isError": true,
                                "data": self.relation_guidance_for_link(src, tgt, explicit_rel)
                            })),
                        }
                    }
                    Err(e) => Some(json!({
                        "content": [{ "type": "text", "text": format!("Erreur liaison: {}", e) }],
                        "isError": true,
                        "data": self.relation_guidance_for_link(src, tgt, explicit_rel)
                    })),
                }
            }
            _ => None,
        }
    }

    fn load_soll_doc_nodes(&self, project_code: &str) -> Result<Vec<SollDocNode>, String> {
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT id, type, COALESCE(title, ''), COALESCE(description, ''), COALESCE(status, ''), COALESCE(metadata, '{{}}') \
                 FROM soll.Node{} ORDER BY type, id",
                project_scope_clause_for_table("id", Some(project_code))
            ))
            .map_err(|e| e.to_string())?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 6)
            .map(|row| SollDocNode {
                id: row[0].clone(),
                entity_type: row[1].clone(),
                title: row[2].clone(),
                description: row[3].clone(),
                status: row[4].clone(),
                metadata: row[5].clone(),
            })
            .collect())
    }

    fn load_soll_doc_edges(&self, project_code: &str) -> Result<Vec<SollDocEdge>, String> {
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT source_id, target_id, relation_type FROM soll.Edge{}",
                project_scope_clause_for_relation(Some(project_code))
            ))
            .map_err(|e| e.to_string())?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 3)
            .map(|row| SollDocEdge {
                source_id: row[0].clone(),
                target_id: row[1].clone(),
                relation_type: row[2].clone(),
            })
            .collect())
    }

    fn generate_soll_doc_pages(
        &self,
        project_code: &str,
        nodes: &[SollDocNode],
        edges: &[SollDocEdge],
    ) -> Vec<SollDocPageSpec> {
        let nodes_by_id = nodes
            .iter()
            .map(|node| (node.id.clone(), node.clone()))
            .collect::<HashMap<_, _>>();
        let mut incoming = HashMap::<String, Vec<SollDocEdge>>::new();
        let mut outgoing = HashMap::<String, Vec<SollDocEdge>>::new();
        for edge in edges {
            incoming
                .entry(edge.target_id.clone())
                .or_default()
                .push(edge.clone());
            outgoing
                .entry(edge.source_id.clone())
                .or_default()
                .push(edge.clone());
        }

        let preferred_parent_map =
            build_preferred_hierarchy_parent_map(nodes, &outgoing, &nodes_by_id);
        let hierarchy_children_map =
            build_hierarchy_children_map(&preferred_parent_map, &nodes_by_id);
        let hierarchy_root_ids = hierarchy_root_ids_for_project(nodes, &preferred_parent_map);
        let unattached_ids = hierarchy_unattached_ids_for_project(nodes, &preferred_parent_map);

        let mut pages = Vec::new();
        let project_summary = {
            let counts = nodes
                .iter()
                .fold(HashMap::<String, usize>::new(), |mut acc, node| {
                    *acc.entry(node.entity_type.clone()).or_insert(0) += 1;
                    acc
                });
            let mut items = counts.into_iter().collect::<Vec<_>>();
            items.sort_by(|left, right| left.0.cmp(&right.0));
            items
                .into_iter()
                .map(|(kind, count)| {
                    format!(
                        "<div class=\"cell\"><strong>{}</strong><div>{}</div></div>",
                        html_escape(&kind),
                        count
                    )
                })
                .collect::<String>()
        };

        let project_tree_html = render_project_tree_html(
            project_code,
            &hierarchy_root_ids,
            &hierarchy_children_map,
            &nodes_by_id,
            None,
            "nodes/",
            "index.html",
            true,
            &HashSet::new(),
        );
        let project_focus_nodes = hierarchy_root_ids
            .iter()
            .filter_map(|root_id| nodes_by_id.get(root_id).cloned())
            .collect::<Vec<_>>();
        let mut project_graph_nodes = vec![SollDocNode {
            id: format!("PRJ-{}", project_code),
            entity_type: "Project".to_string(),
            title: project_code.to_string(),
            description: format!("Derived project root for {}", project_code),
            status: "derived".to_string(),
            metadata: "{}".to_string(),
        }];
        project_graph_nodes.extend(project_focus_nodes.clone());
        let project_graph_edges = project_focus_nodes
            .iter()
            .map(|node| SollDocEdge {
                source_id: format!("PRJ-{}", project_code),
                target_id: node.id.clone(),
                relation_type: "CONTAINS".to_string(),
            })
            .collect::<Vec<_>>();
        let project_graph_links = project_focus_nodes
            .iter()
            .map(|node| {
                (
                    node.id.clone(),
                    format!("nodes/{}", node_file_name(&node.id)),
                )
            })
            .chain(std::iter::once((
                format!("PRJ-{}", project_code),
                "index.html".to_string(),
            )))
            .collect::<HashMap<_, _>>();
        let project_right_html = format!(
            "{}{}{}{}{}",
            linked_node_list_html("Vision Children", &hierarchy_root_ids, &nodes_by_id, "nodes/"),
            linked_node_list_html(
                "Unattached Node Pages",
                &unattached_ids,
                &nodes_by_id,
                "nodes/"
            ),
            linked_node_list_html(
                "All Node Pages",
                &nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>(),
                &nodes_by_id,
                "nodes/"
            ),
            linked_page_list_html(
                "Compatibility Subtree Pages",
                &hierarchy_root_ids
                    .iter()
                    .filter_map(|node_id| nodes_by_id.get(node_id))
                    .map(|node| (
                        format!("subtrees/{}", subtree_file_name(&node.id)),
                        format!("{} subtree · {}", entity_type_short_label(&node.entity_type), node.title),
                        node.id.clone()
                    ))
                    .collect::<Vec<_>>()
            ),
            "<section class=\"card\"><h3>Reading Model</h3><p class=\"muted\">Project root on the left, attached visions on the right. Use the tree to descend, or click a focus child to open its own page.</p></section>"
        );
        pages.push(SollDocPageSpec {
            relative_path: "index.html".to_string(),
            title: format!("{} Project Root", project_code),
            html: render_site_page(
                &format!("{} Project Root", project_code),
                "SOLL Derived Project",
                "Project-level hierarchy page derived from live SOLL. This is a human-readable navigation surface, not canonical truth.",
                &format!("<a href=\"../index.html\">GLO</a><span>/</span><span>{}</span>", html_escape(project_code)),
                "Project Tree",
                &project_tree_html,
                "Hierarchy Focus",
                &render_mermaid_graph(&project_graph_nodes, &project_graph_edges, &project_graph_links),
                "Details",
                &project_right_html,
                &format!(
                    "{}<div class=\"cell\"><strong>Focus</strong><div>{}</div></div><div class=\"cell\"><strong>Boundary</strong><div>derived / non-canonical</div></div>",
                    project_summary,
                    html_escape(project_code)
                ),
            ),
            node_ids: project_focus_nodes.iter().map(|node| node.id.clone()).collect(),
            edge_keys: project_graph_edges.iter().map(edge_key).collect(),
        });

        let mut subtree_roots = nodes
            .iter()
            .filter(|node| subtree_anchor_type(&node.entity_type))
            .cloned()
            .collect::<Vec<_>>();
        subtree_roots.sort_by(|left, right| left.id.cmp(&right.id));
        let mut subtree_membership = HashMap::<String, Vec<String>>::new();
        for root in subtree_roots {
            let mut subtree_ids = HashSet::new();
            let mut queue = vec![root.id.clone()];
            while let Some(current) = queue.pop() {
                if !subtree_ids.insert(current.clone()) {
                    continue;
                }
                if let Some(parent_edges) = incoming.get(&current) {
                    queue.extend(parent_edges.iter().map(|edge| edge.source_id.clone()));
                }
            }
            if let Some(root_outgoing) = outgoing.get(&root.id) {
                subtree_ids.extend(root_outgoing.iter().map(|edge| edge.target_id.clone()));
            }
            for node_id in &subtree_ids {
                subtree_membership
                    .entry(node_id.clone())
                    .or_default()
                    .push(root.id.clone());
            }

            let mut subtree_nodes = subtree_ids
                .iter()
                .filter_map(|id| nodes_by_id.get(id).cloned())
                .collect::<Vec<_>>();
            subtree_nodes.sort_by(|left, right| left.id.cmp(&right.id));
            let subtree_edges = edges
                .iter()
                .filter(|edge| {
                    subtree_ids.contains(&edge.source_id) && subtree_ids.contains(&edge.target_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            let subtree_links = subtree_nodes
                .iter()
                .map(|node| {
                    (
                        node.id.clone(),
                        format!("../nodes/{}", node_file_name(&node.id)),
                    )
                })
                .collect::<HashMap<_, _>>();
            let inbound_ids = incoming
                .get(&root.id)
                .map(|items| {
                    items
                        .iter()
                        .map(|edge| edge.source_id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let outbound_ids = outgoing
                .get(&root.id)
                .map(|items| {
                    items
                        .iter()
                        .map(|edge| edge.target_id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let hierarchy_root_set = hierarchy_root_ids.iter().cloned().collect::<HashSet<_>>();
            let related_subtree_items = inbound_ids
                .iter()
                .chain(outbound_ids.iter())
                .filter(|candidate| hierarchy_root_set.contains(*candidate))
                .filter_map(|candidate| nodes_by_id.get(candidate))
                .map(|candidate| {
                    (
                        subtree_file_name(&candidate.id),
                        format!(
                            "{} · {}",
                            entity_type_short_label(&candidate.entity_type),
                            candidate.title
                        ),
                        candidate.id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let subtree_node_ids = subtree_nodes
                .iter()
                .map(|node| node.id.clone())
                .collect::<Vec<_>>();
            let left_tree_html = render_project_tree_html(
                project_code,
                &hierarchy_root_ids,
                &hierarchy_children_map,
                &nodes_by_id,
                Some(&root.id),
                "../nodes/",
                "../index.html",
                true,
                &ancestor_chain_ids(&root.id, &preferred_parent_map),
            );
            let right_html = format!(
                "{}{}{}{}<section class=\"card\"><h3>Relations</h3>{}</section>",
                linked_page_list_html("Related Subtrees", &related_subtree_items),
                linked_node_list_html(
                    "Feeds Into This Root",
                    &inbound_ids,
                    &nodes_by_id,
                    "../nodes/"
                ),
                linked_node_list_html(
                    "Root Outgoing Context",
                    &outbound_ids,
                    &nodes_by_id,
                    "../nodes/"
                ),
                linked_node_list_html(
                    "All Nodes In This Subtree",
                    &subtree_node_ids,
                    &nodes_by_id,
                    "../nodes/"
                ),
                relation_line_html(&subtree_edges, &nodes_by_id)
            );
            let summary_html = format!(
                "<div class=\"cell\"><strong>Root</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Nodes</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Edges</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Boundary</strong><div>derived / non-canonical</div></div>",
                html_escape(&root.id),
                subtree_nodes.len(),
                subtree_edges.len()
            );
            let subtree_graph =
                render_mermaid_graph(&subtree_nodes, &subtree_edges, &subtree_links);
            pages.push(SollDocPageSpec {
                relative_path: format!("subtrees/{}", subtree_file_name(&root.id)),
                title: format!("{} · {} subtree", root.id, root.title),
                html: render_site_page(
                    &format!("{} · {}", root.id, root.title),
                    &format!("{} subtree", root.entity_type),
                    "Subtree page derived from reverse reachability plus immediate root context. This page is navigable and non-canonical.",
                    &format!(
                        "<a href=\"../../index.html\">GLO</a><span>/</span><a href=\"../index.html\">{}</a><span>/</span><span>{}</span>",
                        html_escape(project_code),
                        html_escape(&root.id)
                    ),
                    "Project Tree",
                    &left_tree_html,
                    "Hierarchy Focus",
                    &subtree_graph,
                    "Details",
                    &right_html,
                    &summary_html,
                ),
                node_ids: subtree_nodes.iter().map(|node| node.id.clone()).collect(),
                edge_keys: subtree_edges.iter().map(edge_key).collect(),
            });
        }

        let mut ordered_nodes = nodes.to_vec();
        ordered_nodes.sort_by(|left, right| left.id.cmp(&right.id));
        for node in ordered_nodes {
            let incoming_edges = incoming.get(&node.id).cloned().unwrap_or_default();
            let outgoing_edges = outgoing.get(&node.id).cloned().unwrap_or_default();
            let parent_ids = hierarchy_candidate_parent_ids(&node.id, &outgoing, &nodes_by_id);
            let child_ids = hierarchy_children_map
                .get(&node.id)
                .cloned()
                .unwrap_or_default();
            let mut local_ids = HashSet::from([node.id.clone()]);
            local_ids.extend(parent_ids.iter().cloned());
            local_ids.extend(child_ids.iter().cloned());
            let mut local_edges = Vec::new();
            for parent_id in &parent_ids {
                if let Some(edge) = outgoing_edges
                    .iter()
                    .find(|edge| edge.target_id == *parent_id)
                {
                    local_edges.push(SollDocEdge {
                        source_id: parent_id.clone(),
                        target_id: node.id.clone(),
                        relation_type: edge.relation_type.clone(),
                    });
                } else {
                    local_edges.push(SollDocEdge {
                        source_id: parent_id.clone(),
                        target_id: node.id.clone(),
                        relation_type: "PARENT_OF".to_string(),
                    });
                }
            }
            for child_id in &child_ids {
                if let Some(edge) = incoming_edges
                    .iter()
                    .find(|edge| edge.source_id == *child_id)
                {
                    local_edges.push(SollDocEdge {
                        source_id: node.id.clone(),
                        target_id: child_id.clone(),
                        relation_type: edge.relation_type.clone(),
                    });
                } else {
                    local_edges.push(SollDocEdge {
                        source_id: node.id.clone(),
                        target_id: child_id.clone(),
                        relation_type: "CHILD_OF".to_string(),
                    });
                }
            }
            local_edges.sort_by(|left, right| {
                (&left.source_id, &left.relation_type, &left.target_id).cmp(&(
                    &right.source_id,
                    &right.relation_type,
                    &right.target_id,
                ))
            });
            local_edges.dedup_by(|left, right| edge_key(left) == edge_key(right));
            let mut local_nodes = local_ids
                .iter()
                .filter_map(|id| nodes_by_id.get(id).cloned())
                .collect::<Vec<_>>();
            local_nodes.sort_by(|left, right| left.id.cmp(&right.id));
            let local_links = local_nodes
                .iter()
                .map(|candidate| (candidate.id.clone(), node_file_name(&candidate.id)))
                .collect::<HashMap<_, _>>();
            let incoming_ids = incoming_edges
                .iter()
                .map(|edge| edge.source_id.clone())
                .collect::<Vec<_>>();
            let outgoing_ids = outgoing_edges
                .iter()
                .map(|edge| edge.target_id.clone())
                .collect::<Vec<_>>();
            let containing_subtree_items = subtree_membership
                .get(&node.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|root_id| nodes_by_id.get(&root_id).cloned())
                .map(|root_node| {
                    (
                        format!("../subtrees/{}", subtree_file_name(&root_node.id)),
                        format!(
                            "{} subtree · {}",
                            entity_type_short_label(&root_node.entity_type),
                            root_node.title
                        ),
                        root_node.id,
                    )
                })
                .collect::<Vec<_>>();
            let parent_page_items = parent_ids
                .iter()
                .filter_map(|candidate| nodes_by_id.get(candidate))
                .map(|candidate| {
                    (
                        node_file_name(&candidate.id),
                        format!(
                            "{} · {}",
                            entity_type_short_label(&candidate.entity_type),
                            candidate.title
                        ),
                        candidate.id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let child_page_items = child_ids
                .iter()
                .filter_map(|candidate| nodes_by_id.get(candidate))
                .map(|candidate| {
                    (
                        node_file_name(&candidate.id),
                        format!(
                            "{} · {}",
                            entity_type_short_label(&candidate.entity_type),
                            candidate.title
                        ),
                        candidate.id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let summary_html = format!(
                "<div class=\"cell\"><strong>Type</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Status</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Parents</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Children</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Boundary</strong><div>derived / non-canonical</div></div>",
                html_escape(&node.entity_type),
                html_escape(if node.status.is_empty() {
                    "n/a"
                } else {
                    &node.status
                }),
                parent_ids.len(),
                child_ids.len()
            );
            let left_tree_html = render_project_tree_html(
                project_code,
                &hierarchy_root_ids,
                &hierarchy_children_map,
                &nodes_by_id,
                Some(&node.id),
                "../nodes/",
                "../index.html",
                true,
                &ancestor_chain_ids(&node.id, &preferred_parent_map),
            );
            let detail_section = format!(
                "<section class=\"card\"><h3>Description</h3><p>{}</p></section>\
                 <section class=\"card\"><h3>Metadata</h3><pre>{}</pre></section>\
                 <section class=\"card\"><h3>Projection Boundary</h3><p class=\"muted\">Parent/child sections below show only the primary hierarchy projection. Supporting or lateral relations remain visible under neighbor and relation sections.</p></section>\
                 {}{}{}{}{}{}{}\
                 <section class=\"card\"><h3>Canonical Relations</h3>{}</section>",
                html_escape(if node.description.is_empty() {
                    "No description."
                } else {
                    &node.description
                }),
                html_escape(&node.metadata),
                linked_page_list_html("Containing Subtrees", &containing_subtree_items),
                linked_page_list_html("Primary Parent Node Pages", &parent_page_items),
                linked_page_list_html("Primary Child Node Pages", &child_page_items),
                linked_node_list_html("Primary Hierarchy Parents", &parent_ids, &nodes_by_id, ""),
                linked_node_list_html("Primary Hierarchy Children", &child_ids, &nodes_by_id, ""),
                linked_node_list_html("Incoming Neighbors", &incoming_ids, &nodes_by_id, ""),
                linked_node_list_html("Outgoing Neighbors", &outgoing_ids, &nodes_by_id, ""),
                relation_line_html(
                    &incoming_edges
                        .iter()
                        .chain(outgoing_edges.iter())
                        .cloned()
                        .collect::<Vec<_>>(),
                    &nodes_by_id
                )
            );
            let node_graph = render_mermaid_graph(&local_nodes, &local_edges, &local_links);
            pages.push(SollDocPageSpec {
                relative_path: format!("nodes/{}", node_file_name(&node.id)),
                title: format!("{} · {}", node.id, node.title),
                html: render_site_page(
                    &format!("{} · {}", node.id, node.title),
                    &node.entity_type,
                    "Node detail page derived from live SOLL. Use this as a readable lens, not as canonical restore input.",
                    &format!(
                        "<a href=\"../../index.html\">GLO</a><span>/</span><a href=\"../index.html\">{}</a><span>/</span><span>{}</span>",
                        html_escape(project_code),
                        html_escape(&node.id)
                    ),
                    "Project Tree",
                    &left_tree_html,
                    "Hierarchy Focus",
                    &node_graph,
                    "Details",
                    &detail_section,
                    &summary_html,
                ),
                node_ids: local_nodes.iter().map(|candidate| candidate.id.clone()).collect(),
                edge_keys: local_edges.iter().map(edge_key).collect(),
            });
        }

        pages
    }

    fn delete_obsolete_derived_doc_paths(
        &self,
        manifest_path: &Path,
        output_root: &Path,
        current_relative_paths: &HashSet<String>,
    ) -> Result<Vec<String>, String> {
        let existing_manifest = match std::fs::read_to_string(manifest_path) {
            Ok(contents) => contents,
            Err(_) => return Ok(Vec::new()),
        };
        let manifest: Value =
            serde_json::from_str(&existing_manifest).unwrap_or_else(|_| json!({}));
        let mut deleted = Vec::new();
        if let Some(pages) = manifest.get("pages").and_then(|value| value.as_array()) {
            for relative_path in pages
                .iter()
                .filter_map(|page| page.get("path").and_then(|value| value.as_str()))
            {
                if current_relative_paths.contains(relative_path) {
                    continue;
                }
                let stale_path = output_root.join(relative_path);
                if stale_path.is_file() {
                    std::fs::remove_file(&stale_path).map_err(|error| error.to_string())?;
                    deleted.push(stale_path.to_string_lossy().to_string());
                }
            }
        }
        Ok(deleted)
    }

    fn should_use_incremental_project_docs(&self, manifest_path: &Path) -> bool {
        let Ok(existing_manifest) = std::fs::read_to_string(manifest_path) else {
            return false;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&existing_manifest) else {
            return false;
        };
        manifest
            .get("generator_version")
            .and_then(|value| value.as_str())
            .map(|value| value == SOLL_PROJECT_DOCS_GENERATOR_VERSION)
            .unwrap_or(false)
            && manifest
                .get("pages")
                .and_then(|value| value.as_array())
                .is_some()
    }

    fn load_soll_derived_project_entries(&self, site_root: &Path) -> Vec<SollDerivedProjectEntry> {
        let _ = self.sync_project_code_registry_from_meta();
        let registry_raw = self
            .graph_store
            .query_json(
                "SELECT project_code, COALESCE(project_name,''), COALESCE(project_path,'') \
                 FROM soll.ProjectCodeRegistry ORDER BY project_code ASC, project_name ASC",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let registry_rows: Vec<Vec<String>> =
            serde_json::from_str(&registry_raw).unwrap_or_default();

        let counts_raw = self
            .graph_store
            .query_json(
                "SELECT project_code, CAST(COUNT(*) AS TEXT) FROM soll.Node GROUP BY project_code ORDER BY project_code ASC",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let count_rows: Vec<Vec<String>> = serde_json::from_str(&counts_raw).unwrap_or_default();
        let node_counts = count_rows
            .into_iter()
            .filter(|row| row.len() >= 2)
            .map(|row| (row[0].clone(), row[1].parse::<usize>().unwrap_or_default()))
            .collect::<HashMap<_, _>>();

        let mut entries = registry_rows
            .into_iter()
            .filter(|row| row.len() >= 3)
            .map(|row| {
                let project_code = row[0].clone();
                let has_docs = site_root.join(&project_code).join("index.html").is_file();
                SollDerivedProjectEntry {
                    node_count: *node_counts.get(&project_code).unwrap_or(&0),
                    has_docs,
                    project_code,
                    project_name: row[1].clone(),
                    project_path: row[2].clone(),
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            (&left.project_code, &left.project_name)
                .cmp(&(&right.project_code, &right.project_name))
        });
        entries
    }

    fn render_soll_root_page(&self, entries: &[SollDerivedProjectEntry]) -> String {
        let mut graph_nodes = vec![SollDocNode {
            id: "GLO".to_string(),
            entity_type: "Portfolio".to_string(),
            title: "Global portfolio".to_string(),
            description: "Derived reading root for all known projects".to_string(),
            status: "derived".to_string(),
            metadata: "{}".to_string(),
        }];
        let mut graph_edges = Vec::new();
        let mut links = HashMap::new();
        let mut cards = String::new();
        let mut tree_items = String::new();
        let docs_ready = entries.iter().filter(|entry| entry.has_docs).count();

        for entry in entries {
            let entry_label = if entry.project_name.is_empty() {
                entry.project_code.clone()
            } else {
                format!("{} · {}", entry.project_code, entry.project_name)
            };
            graph_nodes.push(SollDocNode {
                id: entry.project_code.clone(),
                entity_type: "Project".to_string(),
                title: if entry.project_name.is_empty() {
                    entry.project_code.clone()
                } else {
                    format!("{} · {}", entry.project_code, entry.project_name)
                },
                description: entry.project_path.clone(),
                status: if entry.has_docs { "ready" } else { "pending" }.to_string(),
                metadata: "{}".to_string(),
            });
            graph_edges.push(SollDocEdge {
                source_id: "GLO".to_string(),
                target_id: entry.project_code.clone(),
                relation_type: "CONTAINS".to_string(),
            });
            if entry.has_docs {
                links.insert(
                    entry.project_code.clone(),
                    format!("{}/index.html", entry.project_code),
                );
            }
            cards.push_str(&format!(
                "<section class=\"card\"><h3>{}</h3><p class=\"muted\">{}</p><p><strong>Nodes:</strong> {}<br><strong>Status:</strong> {}</p>{}</section>",
                html_escape(&entry.project_code),
                html_escape(if entry.project_name.is_empty() { "Unnamed project" } else { &entry.project_name }),
                entry.node_count,
                if entry.has_docs { "docs ready" } else { "docs pending" },
                if entry.has_docs {
                    format!("<p><a href=\"{}/index.html\">Open project docs</a><br><span class=\"muted\">{}</span></p>", html_escape(&entry.project_code), html_escape(&entry.project_path))
                } else {
                    format!("<p class=\"muted\">No derived site yet.<br>{}</p>", html_escape(&entry.project_path))
                }
            ));
            if entry.has_docs {
                tree_items.push_str(&format!(
                    "<li class=\"tree-item leaf\"><a class=\"tree-link\" href=\"{}/index.html\"><span class=\"tree-tag\">PRJ</span><span>{}</span></a></li>",
                    html_escape(&entry.project_code),
                    html_escape(&entry_label)
                ));
            } else {
                tree_items.push_str(&format!(
                    "<li class=\"tree-item leaf\"><span class=\"tree-link muted\"><span class=\"tree-tag\">PRJ</span><span>{}</span></span></li>",
                    html_escape(&entry_label)
                ));
            }
        }

        let summary_html = format!(
            "<div class=\"cell\"><strong>Projects</strong><div>{}</div></div>\
             <div class=\"cell\"><strong>Docs Ready</strong><div>{}</div></div>\
             <div class=\"cell\"><strong>Scope</strong><div>all projects</div></div>\
             <div class=\"cell\"><strong>Boundary</strong><div>derived / non-canonical</div></div>",
            entries.len(),
            docs_ready,
        );

        let root_graph = render_mermaid_graph(&graph_nodes, &graph_edges, &links);
        let left_tree_html = format!(
            "<nav class=\"tree-shell\" aria-label=\"Portfolio tree\"><ul class=\"tree-root\">\
               <li class=\"tree-item branch root\"><details open>\
                 <summary><a class=\"tree-link current\" href=\"index.html\"><span class=\"tree-tag\">GLO</span><span>Global portfolio</span></a></summary>\
                 <ul class=\"tree-children\">{}</ul>\
               </details></li>\
             </ul></nav>",
            tree_items
        );
        render_site_page(
            "SOLL Derived Projects",
            "SOLL Derived Root",
            "Global human-readable index derived from live SOLL. This root is generated, incrementally refreshed when possible, and non-canonical.",
            "<span>GLO</span>",
            "Portfolio Tree",
            &left_tree_html,
            "Portfolio Focus",
            &root_graph,
            "Details",
            &cards,
            &summary_html,
        )
    }

    pub(crate) fn generate_soll_derived_docs(
        &self,
        project_code: &str,
        site_root: Option<&Path>,
        project_output_root: &Path,
    ) -> Result<SollDerivedDocsRefreshSummary, String> {
        if let Err(error) = self.resolve_canonical_project_identity_for_mutation(project_code) {
            return Err(format!("Projet canonique invalide: {}", error));
        }

        let nodes = match self.load_soll_doc_nodes(project_code) {
            Ok(items) => items,
            Err(error) => return Err(format!("Erreur de lecture SOLL: {}", error)),
        };

        let edges = self
            .load_soll_doc_edges(project_code)
            .map_err(|error| format!("Erreur de lecture des relations SOLL: {}", error))?;

        let generated_at_ms = now_unix_ms();
        let project_manifest_path = project_output_root.join("_manifest.json");
        let refresh_mode = if self.should_use_incremental_project_docs(&project_manifest_path) {
            "incremental"
        } else {
            if project_output_root.exists() {
                let _ = std::fs::remove_dir_all(project_output_root);
            }
            "full"
        };
        let pages = self.generate_soll_doc_pages(project_code, &nodes, &edges);
        let current_relative_paths = pages
            .iter()
            .map(|page| page.relative_path.clone())
            .collect::<HashSet<_>>();
        let deleted_paths = self.delete_obsolete_derived_doc_paths(
            &project_manifest_path,
            project_output_root,
            &current_relative_paths,
        )?;

        let mut pages_written = 0usize;
        let mut pages_unchanged = 0usize;
        let mut manifest_pages = Vec::new();
        let mut page_paths = Vec::new();
        for page in &pages {
            let page_path = project_output_root.join(&page.relative_path);
            match write_if_changed(&page_path, &page.html) {
                Ok(true) => pages_written += 1,
                Ok(false) => pages_unchanged += 1,
                Err(error) => {
                    return Err(format!("Erreur d'écriture des docs dérivées: {}", error))
                }
            }
            manifest_pages.push(json!({
                "path": page.relative_path,
                "title": page.title,
                "content_hash": content_hash_hex(&page.html),
                "node_ids": page.node_ids,
                "edge_keys": page.edge_keys,
            }));
            page_paths.push(page_path.to_string_lossy().to_string());
        }

        let project_manifest = json!({
            "project_code": project_code,
            "generator_version": SOLL_PROJECT_DOCS_GENERATOR_VERSION,
            "refresh_mode": refresh_mode,
            "generated_at": generated_at_ms,
            "pages_total": pages.len(),
            "pages_written": pages_written,
            "pages_unchanged": pages_unchanged,
            "pages_deleted": deleted_paths.len(),
            "deleted_paths": deleted_paths,
            "pages": manifest_pages,
        });
        let project_manifest_pretty = serde_json::to_string_pretty(&project_manifest)
            .map_err(|error| format!("Erreur de sérialisation du manifeste: {}", error))?;
        write_if_changed(&project_manifest_path, &project_manifest_pretty)
            .map_err(|error| format!("Erreur d'écriture du manifeste: {}", error))?;

        let (site_root_value, root_manifest_value, root_index_value, root_written) =
            if let Some(site_root) = site_root {
                let entries = self.load_soll_derived_project_entries(site_root);
                let root_index_path = site_root.join("index.html");
                let root_manifest_path = site_root.join("_root_manifest.json");
                let root_html = self.render_soll_root_page(&entries);
                let root_written = write_if_changed(&root_index_path, &root_html)
                    .map_err(|error| format!("Erreur d'écriture du root dérivé: {}", error))?;
                let root_manifest = json!({
                    "generator_version": SOLL_ROOT_DOCS_GENERATOR_VERSION,
                    "refresh_mode": refresh_mode,
                    "generated_at": generated_at_ms,
                    "projects_total": entries.len(),
                    "projects_with_docs": entries.iter().filter(|entry| entry.has_docs).count(),
                    "projects": entries.iter().map(|entry| json!({
                        "project_code": entry.project_code,
                        "project_name": entry.project_name,
                        "project_path": entry.project_path,
                        "node_count": entry.node_count,
                        "has_docs": entry.has_docs
                    })).collect::<Vec<_>>()
                });
                let root_manifest_pretty =
                    serde_json::to_string_pretty(&root_manifest).map_err(|error| {
                        format!("Erreur de sérialisation du root manifest: {}", error)
                    })?;
                write_if_changed(&root_manifest_path, &root_manifest_pretty)
                    .map_err(|error| format!("Erreur d'écriture du root manifest: {}", error))?;
                (
                    site_root.to_string_lossy().to_string(),
                    root_manifest_path.to_string_lossy().to_string(),
                    root_index_path.to_string_lossy().to_string(),
                    root_written,
                )
            } else {
                (String::new(), String::new(), String::new(), false)
            };

        Ok(SollDerivedDocsRefreshSummary {
            project_code: project_code.to_string(),
            site_root: site_root_value,
            project_output_root: project_output_root.to_string_lossy().to_string(),
            project_manifest_path: project_manifest_path.to_string_lossy().to_string(),
            root_manifest_path: root_manifest_value,
            root_index_path: root_index_value,
            refresh_mode: refresh_mode.to_string(),
            pages_total: pages.len(),
            pages_written,
            pages_unchanged,
            pages_deleted: deleted_paths.len(),
            deleted_paths,
            root_written,
            stale_docs: false,
        })
    }

    pub(crate) fn axon_soll_generate_docs(&self, args: &serde_json::Value) -> Option<Value> {
        let project_code = match args.get("project_code").and_then(|value| value.as_str()) {
            Some(value) if !value.trim().is_empty() => value.trim().to_ascii_uppercase(),
            _ => {
                return Some(json!({
                    "content": [{ "type": "text", "text": "`project_code` est obligatoire pour `soll_generate_docs`." }],
                    "isError": true
                }))
            }
        };

        let explicit_project_root = args.get("output_dir").and_then(|value| value.as_str());
        let explicit_site_root = args.get("site_root_dir").and_then(|value| value.as_str());
        let (site_root, project_output_root) = if let Some(site_root_dir) = explicit_site_root {
            let site_root = Path::new(site_root_dir).to_path_buf();
            (Some(site_root.clone()), site_root.join(&project_code))
        } else if let Some(project_root) = explicit_project_root {
            (None, Path::new(project_root).to_path_buf())
        } else {
            match canonical_soll_site_dir() {
                Some(path) => (Some(path.clone()), path.join(&project_code)),
                None => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": "Impossible de résoudre le répertoire canonique docs/derived/soll du dépôt." }],
                        "isError": true
                    }))
                }
            }
        };

        match self.generate_soll_derived_docs(
            &project_code,
            site_root.as_deref(),
            &project_output_root,
        ) {
            Ok(summary) => Some(json!({
                "content": [{ "type": "text", "text": format!(
                    "Generated navigable SOLL docs for `{}`.\nSite root: {}\nProject root: {}\nRefresh mode: {}\nPages total: {}\nPages written: {}\nPages unchanged: {}\nPages deleted: {}\nProject manifest: {}\nRoot index: {}",
                    summary.project_code,
                    summary.site_root,
                    summary.project_output_root,
                    summary.refresh_mode,
                    summary.pages_total,
                    summary.pages_written,
                    summary.pages_unchanged,
                    summary.pages_deleted,
                    summary.project_manifest_path,
                    summary.root_index_path
                ) }],
                "data": {
                    "project_code": summary.project_code,
                    "site_root": json_optional_string(&summary.site_root),
                    "output_root": summary.project_output_root,
                    "manifest_path": summary.project_manifest_path,
                    "root_manifest_path": json_optional_string(&summary.root_manifest_path),
                    "root_index_path": json_optional_string(&summary.root_index_path),
                    "refresh_mode": summary.refresh_mode,
                    "pages_total": summary.pages_total,
                    "pages_written": summary.pages_written,
                    "pages_unchanged": summary.pages_unchanged,
                    "pages_deleted": summary.pages_deleted,
                    "deleted_paths": summary.deleted_paths,
                    "root_written": summary.root_written,
                    "stale_docs": summary.stale_docs,
                    "canonical_boundary": "Derived human docs only. Live SOLL and SOLL_EXPORT remain canonical."
                }
            })),
            Err(error) => Some(json!({
                "content": [{ "type": "text", "text": error }],
                "isError": true
            })),
        }
    }

    pub(crate) fn axon_export_soll(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        let project_code = args.get("project_code").and_then(|v| v.as_str());
        let project_code = match project_code
            .map(|code| self.resolve_project_code(code))
            .transpose()
        {
            Ok(code) => code,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                    "isError": true
                }))
            }
        };
        let mut markdown = String::from(
            "# SOLL Extraction

",
        );

        let now = std::time::SystemTime::now();
        let datetime: chrono::DateTime<chrono::Local> = now.into();
        let timestamp_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        markdown.push_str(&format!(
            "*Généré le : {}*

",
            timestamp_str
        ));

        if let Some(ref code) = project_code {
            markdown.push_str(&format!(
                "*Portée : projet `{}`*

",
                code
            ));
        }

        markdown.push_str(
            "## Topologie (Mermaid)
```mermaid
graph TD;
",
        );
        if let Ok(res) = self.graph_store.query_json(&format!(
            "SELECT source_id, target_id, relation_type FROM soll.Edge{}",
            project_scope_clause_for_relation(project_code.as_deref())
        )) {
            let edges: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for edge in edges {
                if edge.len() >= 3 {
                    markdown.push_str(&format!(
                        "  {} -- {} --> {};
",
                        edge[0], edge[2], edge[1]
                    ));
                }
            }
        }
        markdown.push_str(
            "```

",
        );

        if let Ok(res) = self
            .graph_store
            .query_json(&format!(
                "SELECT id, type, title, description, status, metadata FROM soll.Node{} ORDER BY type, id",
                project_scope_clause_for_table("id", project_code.as_deref())
            ))
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            let mut current_type = String::new();
            for r in rows {
                let n_id = &r[0];
                let n_type = &r[1];
                let title = &r[2];
                let desc = &r[3];
                let status = &r[4];
                let meta = r.get(5).cloned().unwrap_or_default();

                if n_type != &current_type {
                    markdown.push_str(&format!("## Entités : {}\n", n_type));
                    current_type = n_type.clone();
                }

                markdown.push_str(&format!("### {} - {}\n", n_id, title));
                if !desc.is_empty() {
                    markdown.push_str(&format!("**Description:** {}\n", desc));
                }
                if !status.is_empty() {
                    markdown.push_str(&format!("**Status:** {}\n", status));
                }
                if meta != "{}" {
                    markdown.push_str(&format!("**Meta:** `{}`\n", meta));
                }
                markdown.push_str("\n");
            }
        }

        let export_dir = match canonical_soll_export_dir() {
            Some(path) => path,
            None => {
                return Some(serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": "Erreur d'écriture: impossible de résoudre le répertoire canonique docs/vision du dépôt"
                    }],
                    "isError": true
                }))
            }
        };

        let file_name = format!("SOLL_EXPORT_{}.md", datetime.format("%Y-%m-%d_%H%M%S_%3f"));
        let file_path = export_dir.join(file_name);

        let _ = std::fs::create_dir_all(&export_dir);
        match std::fs::write(&file_path, &markdown) {
            Ok(_) => {
                let report = format!(
                    "✅ Exported to {}

---

{}",
                    file_path.display(),
                    markdown.chars().take(300).collect::<String>()
                );
                Some(serde_json::json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                serde_json::json!({ "content": [{ "type": "text", "text": format!("Erreur d'écriture: {}", e) }], "isError": true }),
            ),
        }
    }

    pub(crate) fn axon_validate_soll(&self, args: &Value) -> Option<Value> {
        let project_code = args.get("project_code").and_then(|v| v.as_str());
        let snapshot = match self.soll_completeness_snapshot(project_code) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                    "isError": true
                }))
            }
        };
        let violation_count = snapshot.orphan_requirements.len()
            + snapshot.validations_without_verifies.len()
            + snapshot.decisions_without_links.len()
            + snapshot.uncovered_requirements.len()
            + snapshot.duplicate_title_rows.len()
            + snapshot.relation_policy_violations.len();

        let mut repair_guidance = Vec::new();
        if !snapshot.orphan_requirements.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "orphan_requirements",
                &snapshot.orphan_requirements,
                "Requirements should be structurally attached to the graph.",
                &[
                    "link each requirement to its pillar or guideline with `soll_manager`",
                    "call `soll_relation_schema` with `source_id` or `target_id` before retrying if the valid edge is unclear",
                ],
            ));
        }
        if !snapshot.validations_without_verifies.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "validations_without_verifies",
                &snapshot.validations_without_verifies,
                "Validation nodes should verify at least one requirement.",
                &[
                    "add a `VERIFIES` edge from each validation to the requirement it proves",
                    "use `soll_relation_schema` on the validation id to inspect canonical targets if needed",
                ],
            ));
        }
        if !snapshot.decisions_without_links.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "decisions_without_solves_or_impacts",
                &snapshot.decisions_without_links,
                "Decision nodes should solve a requirement or impact an artifact.",
                &[
                    "link each decision to a requirement with `SOLVES` or `REFINES` when it addresses a need",
                    "link each decision to an artifact with `IMPACTS` or `SUBSTANTIATES` when it changes implementation reality",
                ],
            ));
        }
        if !snapshot.uncovered_requirements.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "requirements_without_evidence_or_criteria",
                &snapshot.uncovered_requirements,
                "Requirements should have acceptance criteria or explicit supporting evidence.",
                &[
                    "update requirement metadata with `acceptance_criteria`",
                    "attach evidence refs or add concept / decision / validation nodes that explain, solve, or verify the requirement",
                ],
            ));
        }
        if !snapshot.duplicate_ids.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "duplicate_titles",
                &snapshot.duplicate_ids,
                "Duplicate SOLL titles usually signal overlapping concepts, requirements, or decisions.",
                &[
                    "merge or supersede duplicates instead of keeping parallel semantic copies",
                    "prefer stable logical keys or update existing ids rather than creating near-identical nodes",
                ],
            ));
        }
        if !snapshot.relation_policy_violations.is_empty() {
            repair_guidance.push(json!({
                "category": "relation_policy_violations",
                "summary": "Some edges violate the canonical SOLL relation policy.",
                "ids": [],
                "details": snapshot.relation_policy_violations,
                "next_steps": [
                    "remove or replace invalid edges with canonical pairs from `soll_relation_schema`",
                    "retry the link only after the source/target kinds and default relation are confirmed"
                ],
                "guidance_source": "server-side canonical soll validation"
            }));
        }

        let completeness = json!({
            "populated": snapshot.total_nodes > 0,
            "structurally_connected": snapshot.structurally_connected(),
            "evidence_ready": snapshot.evidence_ready(),
            "duplicate_free": snapshot.duplicate_free(),
            "concept_completeness": snapshot.concept_complete(),
            "implementation_completeness": snapshot.implementation_complete()
        });

        let mut evidence = format!(
            "Validation SOLL: {} violation(s) de cohérence minimale détectée(s).\n",
            violation_count
        );
        evidence.push_str("Mode: lecture seule, sans auto-réparation.\n");

        if !snapshot.orphan_requirements.is_empty() {
            evidence.push_str("\n- Requirements orphelins:\n");
            for id in &snapshot.orphan_requirements {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }

        if !snapshot.validations_without_verifies.is_empty() {
            evidence.push_str("\n- Validations sans lien VERIFIES:\n");
            for id in &snapshot.validations_without_verifies {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }

        if !snapshot.decisions_without_links.is_empty() {
            evidence.push_str("\n- Decisions sans lien SOLVES/IMPACTS:\n");
            for id in &snapshot.decisions_without_links {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }

        if !snapshot.uncovered_requirements.is_empty() {
            evidence.push_str("\n- Requirements sans critères/preuves:\n");
            for id in &snapshot.uncovered_requirements {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }

        if !snapshot.duplicate_title_rows.is_empty() {
            evidence.push_str("\n- Titres dupliqués (risque de doublon métier):\n");
            for row in &snapshot.duplicate_title_rows {
                if row.len() < 3 {
                    continue;
                }
                evidence.push_str(&format!("  - {} :: {} -> {}\n", row[0], row[1], row[2]));
            }
        }

        if !snapshot.relation_policy_violations.is_empty() {
            evidence.push_str("\n- Relations invalides:\n");
            for violation in &snapshot.relation_policy_violations {
                evidence.push_str(&format!("  - {}\n", violation));
            }
        }

        let status = if violation_count == 0 {
            "ok"
        } else {
            "warn_soll_invariants"
        };
        let confidence = if violation_count == 0 {
            "high"
        } else {
            "medium"
        };
        let summary = if violation_count == 0 {
            "minimal soll invariants verified"
        } else {
            "minimal soll invariants violations detected"
        };
        let report = format!(
            "### 🧭 Validation SOLL\n\n{}",
            format_standard_contract(
                status,
                summary,
                &snapshot.project_scope,
                &evidence,
                &[
                    "run `soll_verify_requirements` for requirement-level coverage",
                    "apply targeted SOLL links with `soll_manager` if needed",
                    "deduplicate by updating existing nodes or using stable `logical_key` in `soll_apply_plan`"
                ],
                confidence,
            )
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": status,
                "summary": summary,
                "scope": snapshot.project_scope,
                "violations": {
                    "orphan_requirements": snapshot.orphan_requirements,
                    "validations_without_verifies": snapshot.validations_without_verifies,
                    "decisions_without_links": snapshot.decisions_without_links,
                    "uncovered_requirements": snapshot.uncovered_requirements,
                    "duplicate_title_rows": snapshot.duplicate_title_rows,
                    "relation_policy_violations": snapshot.relation_policy_violations
                },
                "repair_guidance": repair_guidance,
                "completeness": completeness,
                "requirement_coverage": {
                    "done": snapshot.requirement_coverage.done,
                    "partial": snapshot.requirement_coverage.partial,
                    "missing": snapshot.requirement_coverage.missing
                },
                "guidance_source": "server-side canonical soll validation"
            }
        }))
    }

    pub(crate) fn axon_restore_soll(&self, args: &Value) -> Option<Value> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(find_latest_soll_export)?;

        let markdown = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("SOLL restore read error: {}", e) }],
                    "isError": true
                }))
            }
        };

        let restore = match parse_soll_export(&markdown) {
            Ok(parsed) => parsed,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("SOLL restore parse error: {}", e) }],
                    "isError": true
                }))
            }
        };

        if let Err(e) = self.graph_store.execute(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_prv, last_rev)
             VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_code) DO NOTHING"
        ) {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("SOLL restore registry error: {}", e) }],
                "isError": true
            }));
        }

        let mut restored = SollRestoreCounts::default();

        for vision in restore.vision {
            let mut meta_out: serde_json::Value = vision
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if !vision.goal.is_empty() {
                let goal = vision.goal.clone();
                if let Some(obj) = meta_out.as_object_mut() {
                    obj.insert("goal".to_string(), serde_json::Value::String(goal));
                }
            }
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('VIS-AXO-001', 'Vision', 'AXO', $title, $description, NULL, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "title": vision.title,
                    "description": vision.description,
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore vision error: {}", e) }], "isError": true }));
            }
            restored.vision += 1;
        }

        for pillar in restore.pillars {
            let metadata = pillar.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, title, description, metadata)
                 VALUES ($id, 'Pillar', $title, $description, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": pillar.id,
                    "title": pillar.title,
                    "description": pillar.description,
                    "metadata": metadata
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore pillar error: {}", e) }], "isError": true }));
            }
            restored.pillars += 1;
        }

        for req in restore.requirements {
            let mut meta_out: serde_json::Value = req
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if let Some(obj) = meta_out.as_object_mut() {
                if !req.priority.is_empty() {
                    let priority = req.priority.clone();
                    obj.insert("priority".to_string(), serde_json::Value::String(priority));
                }
                if false {
                    let owner = String::new();
                    obj.insert("owner".to_string(), serde_json::Value::String(owner));
                }
                if false {
                    let ac = String::new();
                    obj.insert(
                        "acceptance_criteria".to_string(),
                        serde_json::Value::String(ac),
                    );
                }
            }
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, title, description, status, metadata)
                 VALUES ($id, 'Requirement', $title, $description, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": req.id,
                    "title": req.title,
                    "description": req.description,
                    "status": req.status.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore requirement error: {}", e) }], "isError": true }));
            }
            restored.requirements += 1;
        }

        for dec in restore.decisions {
            let mut meta_out: serde_json::Value = dec
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if let Some(obj) = meta_out.as_object_mut() {
                if false {
                    let ctx = String::new();
                    obj.insert("context".to_string(), serde_json::Value::String(ctx));
                }
                if false {
                    let rat = String::new();
                    obj.insert("rationale".to_string(), serde_json::Value::String(rat));
                }
            }
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, title, description, status, metadata)
                 VALUES ($id, 'Decision', $title, $description, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": dec.id,
                    "title": dec.title,
                    "description": dec.description,
                    "status": dec.status.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore decision error: {}", e) }], "isError": true }));
            }
            restored.decisions += 1;
        }

        for mil in restore.milestones {
            let metadata = mil.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, title, status, metadata)
                 VALUES ($id, 'Milestone', $title, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": mil.id,
                    "title": mil.title,
                    "status": mil.status.clone(),
                    "metadata": metadata
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore milestone error: {}", e) }], "isError": true }));
            }
            restored.milestones += 1;
        }

        for val in restore.validations {
            let mut meta_out: serde_json::Value = val
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if let Some(obj) = meta_out.as_object_mut() {
                if false {
                    let m = String::new();
                    obj.insert("method".to_string(), serde_json::Value::String(m));
                }
                if false {
                    let t: i64 = 0;
                    obj.insert("timestamp".to_string(), serde_json::Value::Number(t.into()));
                }
            }
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, status, metadata)
                 VALUES ($id, 'Validation', $result, $metadata)
                 ON CONFLICT (id) DO UPDATE SET status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": val.id,
                    "result": val.result.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore validation error: {}", e) }], "isError": true }));
            }
            restored.validations += 1;
        }

        for cpt in restore.concepts {
            let mut meta_out: serde_json::Value = cpt
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if let Some(obj) = meta_out.as_object_mut() {
                if !cpt.rationale.is_empty() {
                    let rat = cpt.rationale.clone();
                    obj.insert("rationale".to_string(), serde_json::Value::String(rat));
                }
            }
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, project_code, title, description, metadata)
                 VALUES ($id, 'Concept', $project_code, $name, $explanation, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": cpt.id,
                    "project_code": "AXO".to_string(),
                    "name": cpt.name,
                    "explanation": cpt.explanation,
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore concept error: {}", e) }], "isError": true }));
            }
            restored.concepts += 1;
        }

        for rel in restore.relations {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata) VALUES (?, ?, ?, '{}') ON CONFLICT DO NOTHING",
                &serde_json::json!([rel.source_id, rel.target_id, rel.relation_type])
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore relation error: {}", e) }], "isError": true }));
            }
            restored.relations += 1;
        }

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "### Restauration SOLL terminee\n\nSource: `{}`\n\nRestaure en mode merge:\n- Vision: {}\n- Pillars: {}\n- Concepts: {}\n- Milestones: {}\n- Requirements: {}\n- Decisions: {}\n- Validations: {}\n- Relations: {}\n\nNote: ce chemin de restauration reconstruit les entites conceptuelles depuis le format Markdown officiel d'export. Les metadonnees et liaisons presentes dans l'export sont rejouees en mode merge; les champs absents conservent le comportement historique tolerant.",
                    path,
                    restored.vision,
                    restored.pillars,
                    restored.concepts,
                    restored.milestones,
                    restored.requirements,
                    restored.decisions,
                    restored.validations,
                    restored.relations
                )
            }]
        }))
    }

    fn query_single_column(&self, query: &str) -> anyhow::Result<Vec<String>> {
        let res = self.graph_store.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect())
    }

    fn requirement_coverage_summary(
        &self,
        project_code: &str,
    ) -> anyhow::Result<RequirementCoverageSummary> {
        let project_code = self.resolve_project_code(project_code)?;
        let query = format!(
            "SELECT r.id,
                    COALESCE(r.status,''),
                    COALESCE(CAST(json_extract(r.metadata, '$.acceptance_criteria') AS VARCHAR), ''),
                    COUNT(t.id)
             FROM soll.Node r
             LEFT JOIN soll.Traceability t
               ON lower(t.soll_entity_type) = lower(r.type)
              AND t.soll_entity_id = r.id
             WHERE r.type='Requirement' AND r.id LIKE 'REQ-{}-%'
             GROUP BY 1,2,3
             ORDER BY r.id",
            escape_sql(&project_code)
        );
        let rows_raw = self.graph_store.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
        let mut summary = RequirementCoverageSummary::default();

        for row in rows {
            if row.len() < 4 {
                continue;
            }
            let id = row[0].clone();
            let status = row[1].clone();
            let criteria = row[2].clone();
            let evidence_count = row[3].parse::<usize>().unwrap_or(0);
            let state = requirement_state_from(status.as_str(), &criteria, evidence_count);

            match state {
                "done" => summary.done += 1,
                "partial" => summary.partial += 1,
                _ => summary.missing += 1,
            }

            summary.entries.push(RequirementCoverageEntry {
                id,
                status,
                evidence_count,
                state: state.to_string(),
            });
        }

        Ok(summary)
    }

    pub(crate) fn soll_completeness_snapshot(
        &self,
        project_code: Option<&str>,
    ) -> anyhow::Result<SollCompletenessSnapshot> {
        let resolved_project_code = match project_code {
            Some(code) => Some(self.resolve_project_code(code)?),
            None => None,
        };
        let project_scope = resolved_project_code
            .clone()
            .map(|code| format!("project:{code}"))
            .unwrap_or_else(|| "workspace:*".to_string());
        let project_scope_predicate = |id_column: &str, project_code: Option<&str>| {
            project_code
                .map(|code| format!("AND {id_column} LIKE '%-{}-%'", escape_sql(code)))
                .unwrap_or_default()
        };

        let total_nodes = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node n WHERE 1=1 {}",
                resolved_project_code
                    .as_deref()
                    .map(|code| format!("AND n.project_code = '{}'", escape_sql(code)))
                    .unwrap_or_default()
            ))
            .unwrap_or(0) as usize;

        let orphan_requirements = self.query_single_column(&format!(
            "SELECT id FROM soll.Node r
             WHERE type = 'Requirement'
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE source_id = r.id OR target_id = r.id)
               {}
             ORDER BY id",
            project_scope_predicate("r.id", resolved_project_code.as_deref())
        ))?;

        let validations_without_verifies = self.query_single_column(&format!(
            "SELECT id FROM soll.Node v
             WHERE type = 'Validation'
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = v.id OR target_id = v.id) AND relation_type = 'VERIFIES')
               {}
             ORDER BY id",
            project_scope_predicate("v.id", resolved_project_code.as_deref())
        ))?;

        let decisions_without_links = self.query_single_column(&format!(
            "SELECT id FROM soll.Node d
             WHERE type = 'Decision'
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = d.id OR target_id = d.id) AND relation_type IN ('SOLVES', 'IMPACTS'))
               {}
             ORDER BY id",
            project_scope_predicate("d.id", resolved_project_code.as_deref())
        ))?;

        let uncovered_requirements = self.query_single_column(&format!(
            "SELECT r.id FROM soll.Node r
             LEFT JOIN soll.Traceability t
               ON lower(t.soll_entity_type) = lower(r.type)
              AND t.soll_entity_id = r.id
             WHERE r.type = 'Requirement'
               {}
             GROUP BY r.id, r.status, r.metadata
             HAVING COUNT(t.id) = 0
                AND COALESCE(CAST(json_extract(r.metadata, '$.acceptance_criteria') AS VARCHAR), '') IN ('', '[]')
             ORDER BY r.id",
            project_scope_predicate("r.id", resolved_project_code.as_deref())
        ))?;

        let duplicate_title_rows_raw = self.graph_store.query_json(&format!(
            "SELECT type, title, string_agg(id, ', ' ORDER BY id)
             FROM soll.Node
             WHERE type IN ('Requirement', 'Decision', 'Concept')
               AND COALESCE(title, '') <> ''
               {}
             GROUP BY type, title
             HAVING COUNT(*) > 1
             ORDER BY type, title",
            resolved_project_code
                .as_deref()
                .map(|code| format!("AND project_code = '{}'", escape_sql(code)))
                .unwrap_or_default()
        ))?;
        let duplicate_title_rows: Vec<Vec<String>> =
            serde_json::from_str(&duplicate_title_rows_raw).unwrap_or_default();

        let duplicate_ids = duplicate_title_rows
            .iter()
            .filter_map(|row| row.get(2).cloned())
            .flat_map(|ids| {
                ids.split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let relation_policy_violations =
            self.collect_relation_policy_violations(resolved_project_code.as_deref())?;
        let requirement_coverage = match resolved_project_code.as_deref() {
            Some(code) => self.requirement_coverage_summary(code)?,
            None => RequirementCoverageSummary::default(),
        };

        Ok(SollCompletenessSnapshot {
            project_scope,
            total_nodes,
            orphan_requirements,
            validations_without_verifies,
            decisions_without_links,
            uncovered_requirements,
            duplicate_title_rows,
            duplicate_ids,
            relation_policy_violations,
            requirement_coverage,
        })
    }

    fn query_named_row(&self, query: &str, expected_columns: usize) -> anyhow::Result<Vec<String>> {
        let res = self.graph_store.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Entité SOLL introuvable"))?;
        if row.len() < expected_columns {
            return Err(anyhow!("Résultat SOLL incomplet pour la mise à jour"));
        }
        Ok(row)
    }

    fn classify_existing_link_endpoint(&self, id: &str) -> anyhow::Result<LinkEndpointKind> {
        let prefix = id.split('-').next().unwrap_or("");
        if let Some(table_name) = soll_entity_table_name(prefix) {
            let exists = self.graph_store.query_count(&format!(
                "SELECT count(*) FROM {} WHERE id = '{}'",
                table_name,
                escape_sql(id)
            ))?;
            if exists == 0 {
                return Err(anyhow!("ID `{}` introuvable", id));
            }
            let canonical_prefix = match prefix {
                "VIS" => "VIS",
                "PIL" => "PIL",
                "REQ" => "REQ",
                "CPT" => "CPT",
                "DEC" => "DEC",
                "MIL" => "MIL",
                "VAL" => "VAL",
                "STK" => "STK",
                "GUI" => "GUI",
                _ => return Err(anyhow!("Préfixe SOLL `{}` non géré", prefix)),
            };
            return Ok(LinkEndpointKind::Soll(canonical_prefix));
        }

        for table_name in ["File", "Symbol", "Chunk"] {
            let column = if table_name == "File" { "path" } else { "id" };
            let exists = self.graph_store.query_count(&format!(
                "SELECT count(*) FROM {} WHERE {} = '{}'",
                table_name,
                column,
                escape_sql(id)
            ))?;
            if exists > 0 {
                return Ok(LinkEndpointKind::Artifact);
            }
        }

        Err(anyhow!("ID `{}` introuvable", id))
    }

    fn select_relation_type_for_link(
        &self,
        source_id: &str,
        target_id: &str,
        explicit_relation_type: Option<&str>,
    ) -> anyhow::Result<(&'static str, RelationPolicy)> {
        let source_kind = self.classify_existing_link_endpoint(source_id)?;
        let target_kind = self.classify_existing_link_endpoint(target_id)?;
        let policy = relation_policy_for_pair(source_kind.label(), target_kind.label())
            .ok_or_else(|| {
                anyhow!(
                    "Aucune relation canonique autorisee pour {} -> {}",
                    source_kind.label(),
                    target_kind.label()
                )
            })?;

        let selected = if let Some(relation_type) = explicit_relation_type {
            let normalized = relation_type.to_uppercase();
            if !policy.allowed.iter().any(|allowed| *allowed == normalized) {
                return Err(anyhow!(
                    "Relation `{}` interdite pour {} -> {}. Relations autorisées: {}. Défaut: {}",
                    normalized,
                    source_kind.label(),
                    target_kind.label(),
                    policy.allowed.join(", "),
                    policy.default.unwrap_or("aucun")
                ));
            }
            normalized
        } else if let Some(default_relation) = policy.default {
            default_relation.to_string()
        } else {
            return Err(anyhow!(
                "Relation explicite requise pour {} -> {}. Relations autorisées: {}",
                source_kind.label(),
                target_kind.label(),
                policy.allowed.join(", ")
            ));
        };

        let selected_static = policy
            .allowed
            .iter()
            .find(|allowed| **allowed == selected)
            .copied()
            .ok_or_else(|| {
                anyhow!(
                    "Relation `{}` introuvable dans la politique canonique",
                    selected
                )
            })?;

        Ok((selected_static, policy))
    }

    fn relation_guidance_for_link(
        &self,
        source_id: &str,
        target_id: &str,
        explicit_relation_type: Option<&str>,
    ) -> Value {
        let requested_relation = explicit_relation_type.map(|value| value.to_ascii_uppercase());
        let source_kind = self.classify_existing_link_endpoint(source_id);
        let target_kind = self.classify_existing_link_endpoint(target_id);

        match (source_kind, target_kind) {
            (Ok(source_kind), Ok(target_kind)) => {
                let source_label = source_kind.label();
                let target_label = target_kind.label();
                let mut payload = relation_policy_payload(source_label, target_label);
                payload["source_id"] = json!(source_id);
                payload["target_id"] = json!(target_id);
                payload["requested_relation"] = requested_relation
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null);
                payload["allowed_target_kinds_from_source"] =
                    Value::Array(allowed_relation_targets_from_source(source_label));
                payload["recommended_incoming_links_to_source_kind"] =
                    Value::Array(incoming_relation_sources_for_target(source_label));
                payload["recommended_incoming_links_to_target_kind"] =
                    Value::Array(incoming_relation_sources_for_target(target_label));
                payload["source_graph_role"] = Value::from(graph_role_for_kind(source_label));
                payload["target_graph_role"] = Value::from(graph_role_for_kind(target_label));
                payload["canonical_examples"] = Value::Array(
                    payload
                        .get("allowed_relations")
                        .and_then(|value| value.as_array())
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|value| value.as_str().map(|relation| {
                            json!({
                                "relation_type": relation,
                                "example": relation_example_sentence(source_label, target_label, relation)
                            })
                        }))
                        .collect(),
                );
                payload["suggested_next_actions"] = if payload
                    .get("pair_allowed")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    let default_relation = payload
                        .get("default_relation")
                        .and_then(|value| value.as_str());
                    let mut actions = Vec::new();
                    if let Some(default_relation) = default_relation {
                        actions.push(format!(
                            "retry `soll_manager` link with relation_type `{}`",
                            default_relation
                        ));
                    }
                    actions.push(
                        "call `soll_relation_schema` with the same source/target ids".to_string(),
                    );
                    actions.push(
                        "if the graph is still incomplete, inspect `recommended_incoming_links_to_target_kind` for the target node".to_string(),
                    );
                    Value::Array(actions.into_iter().map(Value::from).collect())
                } else {
                    Value::Array(vec![
                        Value::from("call `soll_relation_schema` with `source_id` to inspect allowed target kinds"),
                        Value::from("choose a target id whose kind matches one of `allowed_target_kinds_from_source`"),
                        Value::from("inspect `recommended_incoming_links_to_target_kind` if the current target should be reached from another source kind"),
                    ])
                };
                payload
            }
            (source_result, target_result) => {
                let mut errors = Vec::new();
                if let Err(error) = source_result {
                    errors.push(format!("source lookup failed: {}", error));
                }
                if let Err(error) = target_result {
                    errors.push(format!("target lookup failed: {}", error));
                }
                json!({
                    "pair_allowed": false,
                    "source_id": source_id,
                    "target_id": target_id,
                    "requested_relation": requested_relation,
                    "lookup_errors": errors,
                    "suggested_next_actions": [
                        "verify that both ids exist and are canonical",
                        "call `soll_relation_schema` with the known ids or kinds before retrying"
                    ]
                })
            }
        }
    }

    fn insert_validated_relation(
        &self,
        relation_type: &str,
        source_id: &str,
        target_id: &str,
        policy: RelationPolicy,
    ) -> anyhow::Result<bool> {
        let same_relation_exists = self.graph_store.query_count(&format!(
            "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = '{}'",
            escape_sql(source_id),
            escape_sql(target_id),
            escape_sql(relation_type)
        ))?;
        if same_relation_exists > 0 {
            return Ok(false);
        }

        if !policy.allow_multiple_types {
            for other_relation in policy.allowed {
                if *other_relation == relation_type {
                    continue;
                }
                let count = self.graph_store.query_count(&format!(
                    "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = '{}'",
                    escape_sql(source_id),
                    escape_sql(target_id),
                    escape_sql(other_relation)
                ))?;
                if count > 0 {
                    return Err(anyhow::anyhow!(
                        "Conflit de cardinalité: `{}` existe déjà pour `{}` -> `{}`; `{}` est exclusif sur cette paire",
                        other_relation,
                        source_id,
                        target_id,
                        relation_type
                    ));
                }
            }
        }

        self.graph_store.execute_param(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata) VALUES (?, ?, ?, '{}') ON CONFLICT DO NOTHING",
            &serde_json::json!([source_id, target_id, relation_type]),
        )?;
        Ok(true)
    }

    fn collect_relation_policy_violations(
        &self,
        project_code: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let mut violations = Vec::new();
        let mut exclusive_pairs: std::collections::HashMap<
            (String, String),
            std::collections::HashSet<String>,
        > = std::collections::HashMap::new();

        let rows_raw = self.graph_store.query_json("SELECT source_id, target_id, relation_type FROM soll.Edge ORDER BY source_id, target_id")?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let source_id = &row[0];
            let target_id = &row[1];
            let relation_type = &row[2];
            if !relation_scope_matches(source_id, target_id, project_code) {
                continue;
            }

            let source_kind = match self.classify_existing_link_endpoint(source_id) {
                Ok(kind) => kind,
                Err(e) => {
                    violations.push(format!(
                        "{}: {} -> {} ({})",
                        relation_type, source_id, target_id, e
                    ));
                    continue;
                }
            };
            let target_kind = match self.classify_existing_link_endpoint(target_id) {
                Ok(kind) => kind,
                Err(e) => {
                    violations.push(format!(
                        "{}: {} -> {} ({})",
                        relation_type, source_id, target_id, e
                    ));
                    continue;
                }
            };

            let Some(policy) = relation_policy_for_pair(source_kind.label(), target_kind.label())
            else {
                violations.push(format!(
                    "{}: {} -> {} (paire {} -> {} interdite)",
                    relation_type,
                    source_id,
                    target_id,
                    source_kind.label(),
                    target_kind.label()
                ));
                continue;
            };

            if !policy
                .allowed
                .iter()
                .any(|allowed| *allowed == relation_type)
            {
                violations.push(format!(
                    "{}: {} -> {} (non autorisée pour {} -> {}; autorisées: {})",
                    relation_type,
                    source_id,
                    target_id,
                    source_kind.label(),
                    target_kind.label(),
                    policy.allowed.join(", ")
                ));
                continue;
            }

            if !policy.allow_multiple_types {
                exclusive_pairs
                    .entry((source_id.clone(), target_id.clone()))
                    .or_default()
                    .insert(relation_type.to_string());
            }
        }

        for ((source_id, target_id), relation_types) in exclusive_pairs {
            if relation_types.len() > 1 {
                let mut rels = relation_types.into_iter().collect::<Vec<_>>();
                rels.sort();
                violations.push(format!(
                    "{} -> {} (relations exclusives en conflit: {})",
                    source_id,
                    target_id,
                    rels.join(", ")
                ));
            }
        }

        violations.sort();
        violations.dedup();
        Ok(violations)
    }

    fn sync_project_code_registry_from_meta(&self) -> anyhow::Result<()> {
        for identity in discover_project_identities() {
            let project_path = identity.project_path.to_string_lossy().to_string();
            self.graph_store.sync_project_registry_entry(
                &identity.code,
                identity.name.as_deref(),
                Some(&project_path),
            )?;
        }
        Ok(())
    }

    fn known_project_codes_hint(&self) -> String {
        self.query_single_column(
            "SELECT project_code FROM soll.ProjectCodeRegistry ORDER BY project_code ASC",
        )
        .map(|codes| {
            let codes: Vec<String> = codes
                .into_iter()
                .filter(|value| !value.trim().is_empty())
                .collect();
            if codes.is_empty() {
                "aucun code connu".to_string()
            } else {
                codes.join(", ")
            }
        })
        .unwrap_or_else(|_| "aucun code connu".to_string())
    }

    fn ensure_soll_registry_row(&self, project_code: &str) -> anyhow::Result<()> {
        self.graph_store.execute_param(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_prv, last_rev)
             VALUES (?, 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_code) DO NOTHING",
            &json!([project_code]),
        )?;
        Ok(())
    }

    pub(crate) fn validate_explicit_canonical_project_code(
        &self,
        project_code: Option<&str>,
        action_label: &str,
    ) -> anyhow::Result<String> {
        let raw = project_code.unwrap_or("").trim();
        if raw.is_empty() {
            return Err(anyhow!(
                "`project_code` est obligatoire pour {}. Utilisez un code canonique de 3 caractères alphanumériques majuscules, par exemple `AXO`.",
                action_label
            ));
        }

        if !is_valid_project_code(raw) || raw != raw.to_ascii_uppercase() {
            return Err(anyhow!(
                "Identifiant projet non canonique `{}` pour {}. Les mutations SOLL acceptent uniquement `project_code` au format canonique de 3 caractères alphanumériques majuscules, par exemple `AXO`. Codes connus: {}",
                raw,
                action_label,
                self.known_project_codes_hint()
            ));
        }

        Ok(raw.to_string())
    }

    fn require_registered_mutation_project_code(
        &self,
        project_code: Option<&str>,
        action_label: &str,
    ) -> anyhow::Result<String> {
        let canonical_code =
            self.validate_explicit_canonical_project_code(project_code, action_label)?;

        let _ = self.sync_project_code_registry_from_meta();
        let escaped = escape_sql(&canonical_code);
        let rows = self.query_single_column(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
            escaped
        ))?;
        if let Some(code) = rows.into_iter().next() {
            self.ensure_soll_registry_row(&code)?;
            return Ok(code);
        }

        if let Ok(identity) = resolve_canonical_project_identity(&canonical_code) {
            let project_path = identity.project_path.to_string_lossy().to_string();
            self.graph_store.sync_project_registry_entry(
                &identity.code,
                identity.name.as_deref(),
                Some(&project_path),
            )?;
            self.ensure_soll_registry_row(&identity.code)?;
            return Ok(identity.code);
        }

        Err(anyhow!(
            "Code projet canonique `{}` introuvable dans soll.ProjectCodeRegistry ou `.axon/meta.json`. Codes connus: {}",
            canonical_code,
            self.known_project_codes_hint()
        ))
    }

    pub(crate) fn derive_project_name_from_path(
        &self,
        project_path: &str,
    ) -> anyhow::Result<String> {
        Path::new(project_path)
            .file_name()
            .map(|value| value.to_string_lossy().trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "Impossible de dériver le nom projet depuis le chemin `{}`",
                    project_path
                )
            })
    }

    fn split_project_name_parts(&self, raw: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut previous_is_lowercase = false;

        for ch in raw.chars() {
            if !ch.is_ascii_alphanumeric() {
                if !current.is_empty() {
                    parts.push(current.clone());
                    current.clear();
                }
                previous_is_lowercase = false;
                continue;
            }

            let is_uppercase = ch.is_ascii_uppercase();
            if is_uppercase && previous_is_lowercase && !current.is_empty() {
                parts.push(current.clone());
                current.clear();
            }
            current.push(ch.to_ascii_uppercase());
            previous_is_lowercase = ch.is_ascii_lowercase();
        }

        if !current.is_empty() {
            parts.push(current);
        }

        parts
    }

    fn candidate_project_codes_for_name(&self, project_name: &str) -> Vec<String> {
        fn is_consonant(ch: char) -> bool {
            matches!(
                ch,
                'B' | 'C'
                    | 'D'
                    | 'F'
                    | 'G'
                    | 'H'
                    | 'J'
                    | 'K'
                    | 'L'
                    | 'M'
                    | 'N'
                    | 'P'
                    | 'Q'
                    | 'R'
                    | 'S'
                    | 'T'
                    | 'V'
                    | 'W'
                    | 'X'
                    | 'Y'
                    | 'Z'
            )
        }

        let normalized: String = project_name
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_uppercase())
            .collect();
        let parts = self.split_project_name_parts(project_name);
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        let mut push_candidate = |candidate: String| {
            if is_valid_project_code(&candidate) && seen.insert(candidate.clone()) {
                candidates.push(candidate);
            }
        };

        if let Some(first) = parts.first() {
            let mut heuristic = String::new();
            if let Some(ch) = first.chars().next() {
                heuristic.push(ch);
            }
            for ch in first.chars().skip(1).filter(|ch| is_consonant(*ch)) {
                if heuristic.len() >= 2 {
                    break;
                }
                heuristic.push(ch);
            }
            for ch in parts.iter().skip(1).filter_map(|part| part.chars().next()) {
                if heuristic.len() >= 3 {
                    break;
                }
                heuristic.push(ch);
            }
            for ch in normalized.chars() {
                if heuristic.len() >= 3 {
                    break;
                }
                heuristic.push(ch);
            }
            push_candidate(heuristic);
        }

        if normalized.len() >= 3 {
            push_candidate(normalized.chars().take(3).collect());
        }

        let chars: Vec<char> = normalized.chars().collect();
        if chars.len() >= 3 {
            for window in chars.windows(3) {
                push_candidate(window.iter().collect());
            }
            push_candidate(format!(
                "{}{}{}",
                chars[0],
                chars[1],
                chars[chars.len() - 1]
            ));
            push_candidate(format!(
                "{}{}{}",
                chars[0],
                chars[chars.len() / 2],
                chars[chars.len() - 1]
            ));
        }

        candidates
    }

    pub(crate) fn assign_project_code_for_init(
        &self,
        project_name: &str,
        project_path: &str,
    ) -> anyhow::Result<String> {
        let _ = self.sync_project_code_registry_from_meta();
        let escaped_path = escape_sql(project_path);
        if let Some(existing) = self
            .query_single_column(&format!(
                "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_path = '{}'",
                escaped_path
            ))?
            .into_iter()
            .next()
        {
            return Ok(existing);
        }

        let known_codes: HashSet<String> = self
            .query_single_column("SELECT project_code FROM soll.ProjectCodeRegistry")?
            .into_iter()
            .collect();
        for candidate in self.candidate_project_codes_for_name(project_name) {
            if !known_codes.contains(&candidate) {
                return Ok(candidate);
            }
        }

        Err(anyhow!(
            "Impossible d'attribuer un `project_code` canonique unique pour `{}` depuis `{}`. Codes connus: {}",
            project_name,
            project_path,
            self.known_project_codes_hint()
        ))
    }

    fn resolve_canonical_project_identity_for_mutation(
        &self,
        project_code: &str,
    ) -> anyhow::Result<(String, String)> {
        let canonical_code = self
            .require_registered_mutation_project_code(Some(project_code), "cette mutation SOLL")?;
        Ok((canonical_code.clone(), canonical_code))
    }

    pub(crate) fn resolve_project_code(&self, project_code: &str) -> anyhow::Result<String> {
        let escaped = escape_sql(project_code);
        let by_code = self.query_single_column(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
            escaped
        ))?;
        if let Some(code) = by_code.into_iter().next() {
            return Ok(code);
        }

        let _ = self.sync_project_code_registry_from_meta();
        let by_code_after_sync = self.query_single_column(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
            escaped
        ))?;
        if let Some(code) = by_code_after_sync.into_iter().next() {
            return Ok(code);
        }

        if let Ok(identity) = resolve_canonical_project_identity(project_code) {
            let project_path = identity.project_path.to_string_lossy().to_string();
            self.graph_store.sync_project_registry_entry(
                &identity.code,
                identity.name.as_deref(),
                Some(&project_path),
            )?;
            return Ok(identity.code);
        }

        if let Err(e) = resolve_canonical_project_identity(project_code) {
            return Err(e);
        }

        Err(anyhow!(
            "Projet canonique `{}` introuvable dans `.axon/meta.json` ou soll.ProjectCodeRegistry",
            project_code
        ))
    }

    pub(crate) fn axon_project_registry_lookup(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let _ = self.sync_project_code_registry_from_meta();

        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let project_name = args
            .get("project_name")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let project_path = args
            .get("project_path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if project_code.is_none() && project_name.is_none() && project_path.is_none() {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": "`project_registry_lookup` attend au moins un de: `project_code`, `project_name`, `project_path`." }],
                "isError": true
            }));
        }

        let mut clauses = Vec::new();
        if let Some(code) = project_code {
            clauses.push(format!("project_code = '{}'", escape_sql(code)));
        }
        if let Some(name) = project_name {
            clauses.push(format!("project_name = '{}'", escape_sql(name)));
        }
        if let Some(path) = project_path {
            clauses.push(format!("project_path = '{}'", escape_sql(path)));
        }

        let query = format!(
            "SELECT project_code, COALESCE(project_name,''), COALESCE(project_path,'')
             FROM soll.ProjectCodeRegistry
             WHERE {}
             ORDER BY project_code ASC",
            clauses.join(" OR ")
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let matches: Vec<serde_json::Value> = rows
            .iter()
            .filter(|row| row.len() >= 3)
            .map(|row| {
                serde_json::json!({
                    "project_code": row[0],
                    "project_name": row[1],
                    "project_path": row[2]
                })
            })
            .collect();

        let first = matches
            .first()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let found = !matches.is_empty();
        let content = if found {
            format!(
                "Projet canonique trouvé: {} ({})",
                first
                    .get("project_name")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                first
                    .get("project_code")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
            )
        } else {
            "Aucun projet canonique trouvé dans ProjectCodeRegistry pour les critères fournis."
                .to_string()
        };

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": content }],
            "data": {
                "found": found,
                "ambiguous": matches.len() > 1,
                "project_code": first.get("project_code").cloned().unwrap_or(serde_json::json!(null)),
                "project_name": first.get("project_name").cloned().unwrap_or(serde_json::json!(null)),
                "project_path": first.get("project_path").cloned().unwrap_or(serde_json::json!(null)),
                "matches": matches
            }
        }))
    }

    pub(crate) fn axon_soll_relation_schema(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let source_type = args
            .get("source_type")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_uppercase());
        let target_type = args
            .get("target_type")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_uppercase());
        let source_id = args
            .get("source_id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let target_id = args
            .get("target_id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if source_type.is_none()
            && target_type.is_none()
            && source_id.is_none()
            && target_id.is_none()
        {
            return Some(json!({
                "content": [{ "type": "text", "text": "`soll_relation_schema` attend au moins un de: `source_type`, `target_type`, `source_id`, `target_id`." }],
                "isError": true
            }));
        }

        let resolved_source_type = match (source_type, source_id) {
            (Some(kind), _) => Some(kind),
            (None, Some(id)) => match self.classify_existing_link_endpoint(id) {
                Ok(kind) => Some(kind.label().to_string()),
                Err(error) => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!("Impossible de résoudre `source_id`. Discovery remains available via guidance fields: {}", error) }],
                        "data": {
                            "resolved": false,
                            "lookup_stage": "source_id",
                            "source_id": id,
                            "target_id": target_id,
                            "suggested_next_actions": [
                                "verify source_id is canonical",
                                "retry with `source_type` if known"
                            ]
                        }
                    }))
                }
            },
            (None, None) => None,
        };
        let resolved_target_type = match (target_type, target_id) {
            (Some(kind), _) => Some(kind),
            (None, Some(id)) => match self.classify_existing_link_endpoint(id) {
                Ok(kind) => Some(kind.label().to_string()),
                Err(error) => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!("Impossible de résoudre `target_id`. Discovery remains available via guidance fields: {}", error) }],
                        "data": {
                            "resolved": false,
                            "lookup_stage": "target_id",
                            "source_id": source_id,
                            "target_id": id,
                            "suggested_next_actions": [
                                "verify target_id is canonical",
                                "retry with `target_type` if known"
                            ]
                        }
                    }))
                }
            },
            (None, None) => None,
        };

        let data = match (
            resolved_source_type.as_deref(),
            resolved_target_type.as_deref(),
        ) {
            (Some(source_kind), Some(target_kind)) => {
                let mut payload = relation_policy_payload(source_kind, target_kind);
                payload["allowed_target_kinds_from_source"] =
                    Value::Array(allowed_relation_targets_from_source(source_kind));
                payload["recommended_incoming_links_to_source_kind"] =
                    Value::Array(incoming_relation_sources_for_target(source_kind));
                payload["recommended_incoming_links_to_target_kind"] =
                    Value::Array(incoming_relation_sources_for_target(target_kind));
                payload["source_graph_role"] = Value::from(graph_role_for_kind(source_kind));
                payload["target_graph_role"] = Value::from(graph_role_for_kind(target_kind));
                payload["canonical_examples"] = Value::Array(
                    relation_policy_for_pair(source_kind, target_kind)
                        .map(|policy| {
                            policy
                                .allowed
                                .iter()
                                .map(|relation| {
                                    json!({
                                        "relation_type": relation,
                                        "example": relation_example_sentence(source_kind, target_kind, relation)
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                );
                payload["source_id"] = source_id.map(Value::from).unwrap_or(Value::Null);
                payload["target_id"] = target_id.map(Value::from).unwrap_or(Value::Null);
                payload
            }
            (Some(source_kind), None) => json!({
                "resolved": true,
                "source_kind": source_kind,
                "graph_role": graph_role_for_kind(source_kind),
                "kind_projection": kind_projection_policy(source_kind).map(|policy| json!({
                    "breadcrumb_eligible": policy.breadcrumb_eligible,
                    "root_eligible": policy.root_eligible,
                    "tree_order_rank": policy.tree_order_rank
                })),
                "allowed_target_kinds_from_source": allowed_relation_targets_from_source(source_kind),
                "recommended_incoming_links_to_source_kind": incoming_relation_sources_for_target(source_kind),
                "guidance_source": "derived_from_relation_policy"
            }),
            (None, Some(target_kind)) => json!({
                "resolved": true,
                "target_kind": target_kind,
                "graph_role": graph_role_for_kind(target_kind),
                "kind_projection": kind_projection_policy(target_kind).map(|policy| json!({
                    "breadcrumb_eligible": policy.breadcrumb_eligible,
                    "root_eligible": policy.root_eligible,
                    "tree_order_rank": policy.tree_order_rank
                })),
                "incoming_from_source_kinds": incoming_relation_sources_for_target(target_kind),
                "guidance_source": "derived_from_relation_policy"
            }),
            (None, None) => unreachable!(),
        };

        Some(json!({
            "content": [{ "type": "text", "text": "Canonical SOLL relation policy resolved." }],
            "data": data
        }))
    }

    pub(crate) fn next_server_numeric_id(
        &self,
        project_code: &str,
        kind: &str,
    ) -> anyhow::Result<(String, String, &'static str, u64)> {
        let (canonical_code, project_code) =
            self.resolve_canonical_project_identity_for_mutation(project_code)?;
        let (prefix, reg_col, table, id_expr) = match kind {
            "vision" => ("VIS", "last_vis", "soll.Node", "id"),
            "pillar" => ("PIL", "last_pil", "soll.Node", "id"),
            "requirement" => ("REQ", "last_req", "soll.Node", "id"),
            "concept" => ("CPT", "last_cpt", "soll.Node", "id"),
            "decision" => ("DEC", "last_dec", "soll.Node", "id"),
            "milestone" => ("MIL", "last_mil", "soll.Node", "id"),
            "validation" => ("VAL", "last_val", "soll.Node", "id"),
            "stakeholder" => ("STK", "last_stk", "soll.Node", "id"),
            "guideline" => ("GUI", "last_gui", "soll.Node", "id"),
            "preview" => ("PRV", "last_prv", "soll.RevisionPreview", "preview_id"),
            "revision" => ("REV", "last_rev", "soll.Revision", "revision_id"),
            _ => return Err(anyhow!("Unknown id kind")),
        };

        self.graph_store.execute_param(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_prv, last_rev) \
             VALUES (?, 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) ON CONFLICT (project_code) DO NOTHING",
            &json!([canonical_code]),
        )?;

        let current_query = format!(
            "SELECT COALESCE({}, 0) FROM soll.Registry WHERE project_code = '{}'",
            reg_col,
            escape_sql(&canonical_code)
        );
        let current = self
            .query_single_column(&current_query)?
            .into_iter()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);

        let ids_query = format!(
            "SELECT {} FROM {} WHERE {} LIKE '{}-{}-%'",
            id_expr,
            table,
            id_expr,
            prefix,
            escape_sql(&project_code)
        );
        let observed_max = self
            .query_single_column(&ids_query)?
            .into_iter()
            .filter_map(|value| parse_numeric_suffix(&value))
            .max()
            .unwrap_or(0);

        let next = current.max(observed_max) + 1;
        self.graph_store.execute(&format!(
            "UPDATE soll.Registry SET {} = {} WHERE project_code = '{}'",
            reg_col,
            next,
            escape_sql(&canonical_code)
        ))?;

        Ok((canonical_code, project_code, prefix, next))
    }

    pub(crate) fn next_soll_numeric_id(
        &self,
        project_code: &str,
        entity: &str,
    ) -> anyhow::Result<(String, String, &'static str, u64)> {
        self.next_server_numeric_id(project_code, entity)
    }

    #[allow(dead_code)]
    fn restore_soll_relation(
        &self,
        relation_type: &str,
        source_id: &str,
        target_id: &str,
    ) -> anyhow::Result<()> {
        let normalized = relation_type.to_uppercase();
        let (selected, policy) =
            self.select_relation_type_for_link(source_id, target_id, Some(&normalized))?;
        self.insert_validated_relation(selected, source_id, target_id, policy)?;
        Ok(())
    }
}

impl McpServer {
    pub(crate) fn axon_soll_apply_plan(&self, args: &Value) -> Option<Value> {
        let project_code = match self.require_registered_mutation_project_code(
            args.get("project_code").and_then(|v| v.as_str()),
            "soll_apply_plan",
        ) {
            Ok(code) => code,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                    "isError": true
                }))
            }
        };
        let author = args
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let _plan = args.get("plan")?;

        let (canonical_project_code, _) = match self
            .resolve_canonical_project_identity_for_mutation(&project_code)
        {
            Ok(identity) => identity,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                    "isError": true
                }))
            }
        };

        let operations = self.build_plan_operations(&canonical_project_code, args);
        let preview_id = if let Some(reserved_preview_id) = args
            .get("reserved_preview_id")
            .and_then(|value| value.as_str())
        {
            reserved_preview_id.to_string()
        } else {
            let (_, project_code, _, next_preview) = match self
                .next_server_numeric_id(&canonical_project_code, "preview")
            {
                Ok(parts) => parts,
                Err(e) => {
                    return Some(json!({
                        "content": [{"type":"text","text": format!("SOLL apply_plan preview id error: {}", e)}],
                        "isError": true
                    }))
                }
            };
            format!("PRV-{}-{:03}", project_code, next_preview)
        };
        let payload = json!({
            "project_code": canonical_project_code,
            "author": author,
            "dry_run": dry_run,
            "operations": operations
        });

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.RevisionPreview (preview_id, author, project_code, payload, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (preview_id) DO UPDATE SET author = EXCLUDED.author, project_code = EXCLUDED.project_code, payload = EXCLUDED.payload, created_at = EXCLUDED.created_at",
            &json!([preview_id, author, canonical_project_code, payload.to_string(), now_unix_ms()]),
        ) {
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan error: {}", e)}],
                "isError": true
            }));
        }

        let counts = summarize_ops(&operations);
        if dry_run {
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan DRY-RUN ready. preview_id={} (create={}, update={})", preview_id, counts.0, counts.1)}],
                "data": { "preview_id": preview_id, "counts": {"create": counts.0, "update": counts.1}, "operations": operations }
            }));
        }

        self.axon_soll_commit_revision(&json!({ "preview_id": preview_id, "author": author }))
    }
}

fn query_first_sql_cell(server: &McpServer, query: &str) -> Option<String> {
    let raw = server.execute_raw_sql(query).ok()?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).ok()?;
    let first = rows.first()?;
    let value = first.first()?;
    if let Some(text) = value.as_str() {
        Some(text.to_string())
    } else {
        Some(value.to_string())
    }
}

impl McpServer {
    fn soll_node_type_for_entity(entity: &str) -> Option<&'static str> {
        match entity {
            "vision" => Some("Vision"),
            "pillar" => Some("Pillar"),
            "requirement" => Some("Requirement"),
            "concept" => Some("Concept"),
            "decision" => Some("Decision"),
            "milestone" => Some("Milestone"),
            "stakeholder" => Some("Stakeholder"),
            "validation" => Some("Validation"),
            "guideline" => Some("Guideline"),
            _ => None,
        }
    }

    fn resolve_soll_id(
        &self,
        entity: &str,
        project_code: &str,
        title: &str,
        logical_key: &str,
    ) -> Option<String> {
        let node_type = Self::soll_node_type_for_entity(entity)?;

        let by_metadata = format!(
            "SELECT id FROM soll.Node WHERE type = '{}' AND project_code = '{}' AND metadata LIKE '%\"logical_key\":\"{}\"%' ORDER BY id DESC LIMIT 1",
            escape_sql(node_type),
            escape_sql(project_code),
            escape_sql(logical_key)
        );
        if let Some(found) = query_first_sql_cell(self, &by_metadata) {
            return Some(found);
        }

        if !title.trim().is_empty() {
            let by_title = format!(
                "SELECT id FROM soll.Node WHERE type = '{}' AND project_code = '{}' AND title = '{}' ORDER BY id DESC LIMIT 1",
                escape_sql(node_type),
                escape_sql(project_code),
                escape_sql(title)
            );
            if let Some(found) = query_first_sql_cell(self, &by_title) {
                return Some(found);
            }
        }

        None
    }
}

fn soll_tool_text(resp: Option<&Value>) -> Option<String> {
    resp.and_then(|v| {
        v.get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|entry| entry.get("text"))
            .and_then(|text| text.as_str())
            .map(|s| s.to_string())
    })
}

fn soll_tool_is_error(resp: Option<&Value>) -> bool {
    resp.and_then(|v| v.get("isError"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn extract_soll_id_from_message(text: String) -> Option<String> {
    let start = text.find('`')?;
    let end = text[start + 1..].find('`')?;
    Some(text[start + 1..start + 1 + end].to_string())
}

fn json_optional_string(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        json!(value)
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn mermaid_escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "&quot;")
        .replace('\n', "<br/>")
}

fn summarize_for_label(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut summary = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    summary.push('…');
    summary
}

fn entity_type_short_label(entity_type: &str) -> &str {
    match entity_type {
        "Portfolio" => "GLO",
        "Project" => "PRJ",
        "Vision" => "VIS",
        "Pillar" => "PIL",
        "Requirement" => "REQ",
        "Decision" => "DEC",
        "Concept" => "CPT",
        "Guideline" => "GUI",
        "Milestone" => "MIL",
        "Validation" => "VAL",
        "Stakeholder" => "STK",
        _ => entity_type,
    }
}

fn content_hash_hex(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn write_if_changed(path: &Path, content: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }

    std::fs::write(path, content)?;
    Ok(true)
}

fn edge_key(edge: &SollDocEdge) -> String {
    format!(
        "{}--{}-->{}",
        edge.source_id, edge.relation_type, edge.target_id
    )
}

fn node_file_name(node_id: &str) -> String {
    format!("{}.html", node_id)
}

fn subtree_file_name(node_id: &str) -> String {
    format!("{}.html", node_id)
}

fn entity_type_to_kind(entity_type: &str) -> Option<&'static str> {
    match entity_type {
        "Vision" => Some("VIS"),
        "Pillar" => Some("PIL"),
        "Requirement" => Some("REQ"),
        "Decision" => Some("DEC"),
        "Concept" => Some("CPT"),
        "Guideline" => Some("GUI"),
        "Milestone" => Some("MIL"),
        "Validation" => Some("VAL"),
        "Stakeholder" => Some("STK"),
        _ => None,
    }
}

fn projection_child_types(parent_type: &str) -> Vec<&'static str> {
    let Some(parent_kind) = entity_type_to_kind(parent_type) else {
        return Vec::new();
    };
    let mut children = SOLL_RELATION_ENDPOINT_KINDS
        .iter()
        .filter_map(|source_kind| {
            let policy = relation_policy_for_pair(source_kind, parent_kind)?;
            if !matches!(policy.projection.role, ProjectionRole::Primary) {
                return None;
            }
            let source_projection = kind_projection_policy(source_kind)?;
            if !source_projection.breadcrumb_eligible {
                return None;
            }
            let child_type = match *source_kind {
                "VIS" => "Vision",
                "PIL" => "Pillar",
                "REQ" => "Requirement",
                "DEC" => "Decision",
                "CPT" => "Concept",
                "GUI" => "Guideline",
                "MIL" => "Milestone",
                "VAL" => "Validation",
                "STK" => "Stakeholder",
                _ => return None,
            };
            Some((policy.projection.child_order_rank, child_type))
        })
        .collect::<Vec<_>>();
    children.sort_by(|left, right| left.cmp(right));
    children
        .into_iter()
        .map(|(_, child_type)| child_type)
        .collect()
}

fn hierarchy_child_types(parent_type: &str) -> &'static [&'static str] {
    match parent_type {
        "Project" => &["Vision"],
        "Vision" => &["Pillar"],
        "Pillar" => &["Requirement"],
        "Requirement" => &[
            "Decision",
            "Validation",
            "Guideline",
            "Concept",
            "Milestone",
            "Stakeholder",
        ],
        _ => &[],
    }
}

fn hierarchy_relation_allowed(parent_type: &str, child_type: &str) -> bool {
    let canonical = projection_child_types(parent_type);
    if !canonical.is_empty() {
        return canonical.iter().any(|candidate| *candidate == child_type);
    }
    hierarchy_child_types(parent_type)
        .iter()
        .any(|candidate| *candidate == child_type)
}

fn entity_type_sort_rank(entity_type: &str) -> usize {
    if let Some(kind) = entity_type_to_kind(entity_type) {
        if let Some(policy) = kind_projection_policy(kind) {
            return policy.tree_order_rank;
        }
    }
    match entity_type {
        "Project" => 0,
        "Vision" => 1,
        "Pillar" => 2,
        "Requirement" => 3,
        "Decision" => 4,
        "Validation" => 5,
        "Guideline" => 6,
        "Concept" => 7,
        "Milestone" => 8,
        "Stakeholder" => 9,
        _ => 99,
    }
}

fn preferred_parent_sort_key(node: &SollDocNode) -> (usize, &str, &str) {
    (
        entity_type_sort_rank(&node.entity_type),
        node.id.as_str(),
        node.title.as_str(),
    )
}

fn hierarchy_candidate_parent_ids(
    node_id: &str,
    outgoing: &HashMap<String, Vec<SollDocEdge>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> Vec<String> {
    let Some(node) = nodes_by_id.get(node_id) else {
        return Vec::new();
    };
    let mut parent_ids = outgoing
        .get(node_id)
        .into_iter()
        .flatten()
        .filter_map(|edge| {
            let candidate = nodes_by_id.get(&edge.target_id)?;
            if hierarchy_relation_allowed(&candidate.entity_type, &node.entity_type) {
                let pair_projection = entity_type_to_kind(&node.entity_type)
                    .zip(entity_type_to_kind(&candidate.entity_type))
                    .and_then(|(child_kind, parent_kind)| {
                        relation_policy_for_pair(child_kind, parent_kind).map(|policy| {
                            (
                                policy.projection.parent_preference_rank,
                                entity_type_sort_rank(&candidate.entity_type),
                            )
                        })
                    })
                    .unwrap_or((usize::MAX, entity_type_sort_rank(&candidate.entity_type)));
                Some((pair_projection, candidate.id.clone()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    parent_ids.sort_by(|left, right| {
        let left_node = nodes_by_id
            .get(&left.1)
            .expect("left hierarchy parent exists");
        let right_node = nodes_by_id
            .get(&right.1)
            .expect("right hierarchy parent exists");
        left.0.cmp(&right.0).then_with(|| {
            preferred_parent_sort_key(left_node).cmp(&preferred_parent_sort_key(right_node))
        })
    });
    parent_ids.dedup_by(|left, right| left.1 == right.1);
    parent_ids.into_iter().map(|(_, id)| id).collect()
}

fn build_preferred_hierarchy_parent_map(
    nodes: &[SollDocNode],
    outgoing: &HashMap<String, Vec<SollDocEdge>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for node in nodes {
        let parent_ids = hierarchy_candidate_parent_ids(&node.id, outgoing, nodes_by_id);
        if let Some(parent_id) = parent_ids.first() {
            map.insert(node.id.clone(), parent_id.clone());
        }
    }
    map
}

fn build_hierarchy_children_map(
    preferred_parent_map: &HashMap<String, String>,
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> HashMap<String, Vec<String>> {
    let mut children = HashMap::<String, Vec<String>>::new();
    for (child_id, parent_id) in preferred_parent_map {
        children
            .entry(parent_id.clone())
            .or_default()
            .push(child_id.clone());
    }
    for child_ids in children.values_mut() {
        child_ids.sort_by(|left, right| {
            let left_node = nodes_by_id.get(left).expect("child node exists");
            let right_node = nodes_by_id.get(right).expect("child node exists");
            (
                entity_type_sort_rank(&left_node.entity_type),
                left_node.id.as_str(),
                left_node.title.as_str(),
            )
                .cmp(&(
                    entity_type_sort_rank(&right_node.entity_type),
                    right_node.id.as_str(),
                    right_node.title.as_str(),
                ))
        });
        child_ids.dedup();
    }
    children
}

fn hierarchy_root_ids_for_project(
    nodes: &[SollDocNode],
    preferred_parent_map: &HashMap<String, String>,
) -> Vec<String> {
    let mut canonical_roots = nodes
        .iter()
        .filter(|node| {
            !preferred_parent_map.contains_key(&node.id)
                && entity_type_to_kind(&node.entity_type)
                    .and_then(kind_projection_policy)
                    .is_some_and(|policy| policy.root_eligible)
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let mut fallback_roots = nodes
        .iter()
        .filter(|node| {
            !preferred_parent_map.contains_key(&node.id)
                && !entity_type_to_kind(&node.entity_type)
                    .and_then(kind_projection_policy)
                    .is_some_and(|policy| policy.root_eligible)
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    canonical_roots.sort();
    fallback_roots.sort();
    if canonical_roots.is_empty() {
        fallback_roots
    } else {
        canonical_roots
    }
}

fn hierarchy_unattached_ids_for_project(
    nodes: &[SollDocNode],
    preferred_parent_map: &HashMap<String, String>,
) -> Vec<String> {
    let mut unattached = nodes
        .iter()
        .filter(|node| {
            !preferred_parent_map.contains_key(&node.id)
                && !entity_type_to_kind(&node.entity_type)
                    .and_then(kind_projection_policy)
                    .is_some_and(|policy| policy.root_eligible)
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    unattached.sort();
    unattached
}

fn ancestor_chain_ids(
    current_node_id: &str,
    preferred_parent_map: &HashMap<String, String>,
) -> HashSet<String> {
    let mut expanded = HashSet::new();
    let mut cursor = Some(current_node_id.to_string());
    while let Some(node_id) = cursor {
        expanded.insert(node_id.clone());
        cursor = preferred_parent_map.get(&node_id).cloned();
    }
    expanded
}

fn subtree_anchor_type(entity_type: &str) -> bool {
    if entity_type_to_kind(entity_type)
        .and_then(kind_projection_policy)
        .is_some_and(|policy| policy.root_eligible)
    {
        return true;
    }
    let parent_kind = entity_type_to_kind("Vision");
    let candidate_kind = entity_type_to_kind(entity_type);
    match (candidate_kind, parent_kind) {
        (Some(source_kind), Some(target_kind)) => {
            relation_policy_for_pair(source_kind, target_kind)
                .is_some_and(|policy| matches!(policy.projection.role, ProjectionRole::Primary))
        }
        _ => false,
    }
}

fn render_tree_node_html(
    node_id: &str,
    children_map: &HashMap<String, Vec<String>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
    current_node_id: Option<&str>,
    node_href_prefix: &str,
    expanded_nodes: &HashSet<String>,
) -> String {
    let Some(node) = nodes_by_id.get(node_id) else {
        return String::new();
    };
    let child_ids = children_map.get(node_id).cloned().unwrap_or_default();
    let is_current = current_node_id.is_some_and(|candidate| candidate == node_id);
    let current_class = if is_current { " current" } else { "" };
    let label_html = format!(
        "<a class=\"tree-link{}\" href=\"{}{}\"><span class=\"tree-tag\">{}</span><span>{}</span></a>",
        current_class,
        node_href_prefix,
        html_escape(&node_file_name(&node.id)),
        html_escape(entity_type_short_label(&node.entity_type)),
        html_escape(&node.title)
    );
    if child_ids.is_empty() {
        return format!("<li class=\"tree-item leaf\">{}</li>", label_html);
    }

    let child_html = child_ids
        .iter()
        .map(|child_id| {
            render_tree_node_html(
                child_id,
                children_map,
                nodes_by_id,
                current_node_id,
                node_href_prefix,
                expanded_nodes,
            )
        })
        .collect::<String>();
    let open_attr = if expanded_nodes.contains(node_id) {
        " open"
    } else {
        ""
    };
    format!(
        "<li class=\"tree-item branch\"><details{}><summary>{}</summary><ul class=\"tree-children\">{}</ul></details></li>",
        open_attr, label_html, child_html
    )
}

fn render_project_tree_html(
    project_code: &str,
    root_ids: &[String],
    children_map: &HashMap<String, Vec<String>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
    current_node_id: Option<&str>,
    node_href_prefix: &str,
    project_root_href: &str,
    default_open: bool,
    expanded_nodes: &HashSet<String>,
) -> String {
    let root_children_html = root_ids
        .iter()
        .map(|root_id| {
            render_tree_node_html(
                root_id,
                children_map,
                nodes_by_id,
                current_node_id,
                node_href_prefix,
                expanded_nodes,
            )
        })
        .collect::<String>();
    let open_attr = if default_open { " open" } else { "" };
    format!(
        "<nav class=\"tree-shell\" aria-label=\"Project hierarchy\"><ul class=\"tree-root\">\
           <li class=\"tree-item branch root\"><details{}>\
             <summary><a class=\"tree-link{}\" href=\"{}\"><span class=\"tree-tag\">PRJ</span><span>{}</span></a></summary>\
             <ul class=\"tree-children\">{}</ul>\
           </details></li>\
         </ul></nav>",
        open_attr,
        if current_node_id.is_none() { " current" } else { "" },
        html_escape(project_root_href),
        html_escape(project_code),
        root_children_html
    )
}

fn relation_line_html(edges: &[SollDocEdge], nodes_by_id: &HashMap<String, SollDocNode>) -> String {
    if edges.is_empty() {
        return "<p class=\"muted\">No relations in this scope.</p>".to_string();
    }

    let mut items = edges.to_vec();
    items.sort_by(|left, right| {
        (&left.relation_type, &left.source_id, &left.target_id).cmp(&(
            &right.relation_type,
            &right.source_id,
            &right.target_id,
        ))
    });

    let mut html = String::from("<ul class=\"relation-list\">");
    for edge in items {
        let source_label = nodes_by_id
            .get(&edge.source_id)
            .map(|node| {
                format!(
                    "{} · {}",
                    entity_type_short_label(&node.entity_type),
                    node.title
                )
            })
            .unwrap_or_else(|| edge.source_id.clone());
        let target_label = nodes_by_id
            .get(&edge.target_id)
            .map(|node| {
                format!(
                    "{} · {}",
                    entity_type_short_label(&node.entity_type),
                    node.title
                )
            })
            .unwrap_or_else(|| edge.target_id.clone());
        html.push_str(&format!(
            "<li><code>{}</code> <span class=\"rel\">{}</span> <code>{}</code><div class=\"muted\">{} -> {}</div></li>",
            html_escape(&edge.source_id),
            html_escape(&edge.relation_type),
            html_escape(&edge.target_id),
            html_escape(&source_label),
            html_escape(&target_label)
        ));
    }
    html.push_str("</ul>");
    html
}

fn linked_node_list_html(
    title: &str,
    node_ids: &[String],
    nodes_by_id: &HashMap<String, SollDocNode>,
    page_prefix: &str,
) -> String {
    if node_ids.is_empty() {
        return format!(
            "<section class=\"card\"><h3>{}</h3><p class=\"muted\">None.</p></section>",
            html_escape(title)
        );
    }

    let mut ids = node_ids.to_vec();
    ids.sort();
    ids.dedup();

    let mut html = format!(
        "<section class=\"card\"><h3>{}</h3><ul class=\"node-list\">",
        html_escape(title)
    );
    for node_id in ids {
        let Some(node) = nodes_by_id.get(&node_id) else {
            continue;
        };
        html.push_str(&format!(
            "<li><a href=\"{}{}\">{} · {}</a><span class=\"muted\">{}</span></li>",
            page_prefix,
            html_escape(&node_file_name(&node.id)),
            html_escape(entity_type_short_label(&node.entity_type)),
            html_escape(&node.title),
            html_escape(&node.id)
        ));
    }
    html.push_str("</ul></section>");
    html
}

fn linked_page_list_html(title: &str, items: &[(String, String, String)]) -> String {
    if items.is_empty() {
        return format!(
            "<section class=\"card\"><h3>{}</h3><p class=\"muted\">None.</p></section>",
            html_escape(title)
        );
    }

    let mut ordered = items.to_vec();
    ordered.sort_by(|left, right| (&left.1, &left.0).cmp(&(&right.1, &right.0)));
    ordered.dedup_by(|left, right| left.0 == right.0);

    let mut html = format!(
        "<section class=\"card\"><h3>{}</h3><ul class=\"node-list\">",
        html_escape(title)
    );
    for (href, label, meta) in ordered {
        html.push_str(&format!(
            "<li><a href=\"{}\">{}</a><span class=\"muted\">{}</span></li>",
            html_escape(&href),
            html_escape(&label),
            html_escape(&meta)
        ));
    }
    html.push_str("</ul></section>");
    html
}

struct RenderedMermaidGraph {
    definition: String,
    link_map_json: String,
}

fn render_mermaid_graph(
    nodes: &[SollDocNode],
    edges: &[SollDocEdge],
    links: &HashMap<String, String>,
) -> RenderedMermaidGraph {
    let mut ordered_nodes = nodes.to_vec();
    ordered_nodes.sort_by(|left, right| left.id.cmp(&right.id));
    let mermaid_ids = ordered_nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.id.clone(), format!("N{}", idx)))
        .collect::<HashMap<_, _>>();

    let mut ordered_edges = edges.to_vec();
    ordered_edges.sort_by(|left, right| {
        (&left.source_id, &left.relation_type, &left.target_id).cmp(&(
            &right.source_id,
            &right.relation_type,
            &right.target_id,
        ))
    });

    let mut graph = String::from("flowchart LR\n");
    for node in ordered_nodes {
        let label = format!(
            "{} {}: {}",
            entity_type_short_label(&node.entity_type),
            node.id,
            summarize_for_label(&node.title, 42)
        );
        graph.push_str(&format!(
            "  {}[\"{}\"]\n",
            mermaid_ids
                .get(&node.id)
                .map(String::as_str)
                .unwrap_or("NODE"),
            mermaid_escape_label(&label)
        ));
    }
    for edge in ordered_edges {
        let source_id = mermaid_ids
            .get(&edge.source_id)
            .map(String::as_str)
            .unwrap_or("NODE");
        let target_id = mermaid_ids
            .get(&edge.target_id)
            .map(String::as_str)
            .unwrap_or("NODE");
        graph.push_str(&format!(
            "  {} -- {} --> {}\n",
            source_id,
            mermaid_escape_label(&edge.relation_type),
            target_id
        ));
    }

    let mut link_pairs = links.iter().collect::<Vec<_>>();
    link_pairs.sort_by(|left, right| left.0.cmp(right.0));
    for (canonical_node_id, href) in link_pairs {
        let Some(mermaid_id) = mermaid_ids.get(canonical_node_id) else {
            continue;
        };
        graph.push_str(&format!(
            "  click {} href \"{}\" \"Open {}\"\n",
            mermaid_id,
            href,
            mermaid_escape_label(canonical_node_id)
        ));
    }

    let link_map_json = serde_json::to_string(
        &mermaid_ids
            .iter()
            .filter_map(|(canonical_id, mermaid_id)| {
                links
                    .get(canonical_id)
                    .map(|href| (mermaid_id.clone(), href.clone()))
            })
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".to_string());

    RenderedMermaidGraph {
        definition: graph,
        link_map_json,
    }
}

fn render_site_page(
    page_title: &str,
    eyebrow: &str,
    intro: &str,
    breadcrumb_html: &str,
    left_title: &str,
    left_panel_html: &str,
    center_title: &str,
    graph: &RenderedMermaidGraph,
    right_title: &str,
    right_panel_html: &str,
    summary_html: &str,
) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{page_title}</title>
  <script src="https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.min.js"></script>
  <style>
    :root {{
      --bg: #f5f1e8;
      --surface: rgba(255,255,255,0.92);
      --border: rgba(64, 49, 21, 0.14);
      --text: #22170d;
      --muted: #6f5f49;
      --accent: #1f7a6b;
      --accent-2: #b55c2f;
      --shadow: 0 20px 60px rgba(48, 34, 12, 0.12);
      --radius: 22px;
      --left-pane-width: 300px;
      --right-pane-width: 360px;
      --handle-width: 12px;
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      font-family: "Space Grotesk", system-ui, sans-serif;
      background:
        radial-gradient(circle at top right, rgba(31,122,107,0.14), transparent 20%),
        radial-gradient(circle at top left, rgba(181,92,47,0.14), transparent 22%),
        var(--bg);
      color: var(--text);
    }}
    .page {{ width: calc(100vw - 24px); margin: 12px auto 24px; }}
    .hero, .card {{
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: var(--radius);
      box-shadow: var(--shadow);
    }}
    .hero {{ padding: 24px 26px; }}
    .eyebrow {{
      font-size: 12px;
      font-weight: 700;
      letter-spacing: 0.12em;
      text-transform: uppercase;
      color: var(--accent);
      margin-bottom: 10px;
    }}
    h1 {{ margin: 0 0 10px; font-size: clamp(2rem, 4vw, 3.6rem); line-height: 0.95; }}
    .lede {{ margin: 0; color: var(--muted); max-width: 70ch; }}
    .breadcrumb {{
      margin: 14px 0 0;
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      font-size: 0.94rem;
      color: var(--muted);
    }}
    .breadcrumb a {{ color: var(--accent); text-decoration: none; }}
    .toolbar {{
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      margin-top: 20px;
    }}
    .toolbar button {{
      border: 1px solid var(--border);
      border-radius: 999px;
      background: rgba(255,255,255,0.78);
      padding: 10px 14px;
      font: inherit;
      color: var(--text);
      cursor: pointer;
    }}
    .workspace {{
      display: grid;
      grid-template-columns: var(--left-pane-width) var(--handle-width) minmax(0, 1fr) var(--handle-width) var(--right-pane-width);
      gap: 0;
      align-items: stretch;
      min-height: calc(100vh - 220px);
      margin-top: 18px;
    }}
    body.left-collapsed .workspace {{
      grid-template-columns: 0px 0px minmax(0, 1fr) var(--handle-width) var(--right-pane-width);
    }}
    body.right-collapsed .workspace {{
      grid-template-columns: var(--left-pane-width) var(--handle-width) minmax(0, 1fr) 0px 0px;
    }}
    body.left-collapsed.right-collapsed .workspace {{
      grid-template-columns: 0px 0px minmax(0, 1fr) 0px 0px;
    }}
    .pane, .center-pane {{
      min-width: 0;
    }}
    .pane-inner, .center-pane {{
      height: 100%;
      overflow: auto;
      padding: 18px;
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: var(--radius);
      box-shadow: var(--shadow);
    }}
    .center-pane h2, .pane h2 {{ margin-top: 0; }}
    .resize-handle {{
      position: relative;
      width: var(--handle-width);
      cursor: col-resize;
      background: transparent;
    }}
    .resize-handle::before {{
      content: "";
      position: absolute;
      top: 14px;
      bottom: 14px;
      left: calc(50% - 1px);
      width: 2px;
      border-radius: 999px;
      background: rgba(64, 49, 21, 0.16);
    }}
    body.left-collapsed .resize-left,
    body.right-collapsed .resize-right {{
      display: none;
    }}
    .card {{ padding: 18px 18px 16px; }}
    .card h2, .card h3 {{ margin-top: 0; }}
    .mermaid {{
      background: rgba(255,255,255,0.62);
      border-radius: 16px;
      padding: 8px;
      overflow: auto;
    }}
    .node-list, .relation-list {{
      margin: 0;
      padding-left: 18px;
    }}
    .node-list li, .relation-list li {{ margin: 8px 0; }}
    a {{ color: var(--accent-2); }}
    code {{
      background: rgba(31,122,107,0.08);
      border-radius: 8px;
      padding: 0 6px;
      font-size: 0.92em;
    }}
    .muted {{ color: var(--muted); }}
    .rel {{ font-weight: 700; color: var(--accent); }}
    .summary-grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
      gap: 12px;
      margin-top: 14px;
    }}
    .summary-grid .cell {{
      padding: 12px 14px;
      border: 1px solid var(--border);
      border-radius: 14px;
      background: rgba(181,92,47,0.05);
    }}
    .tree-shell, .tree-root, .tree-children {{
      margin: 0;
      padding-left: 0;
      list-style: none;
    }}
    .tree-item {{ margin: 4px 0; }}
    .tree-item > details > summary {{
      list-style: none;
      cursor: pointer;
    }}
    .tree-item > details > summary::-webkit-details-marker {{ display: none; }}
    .tree-link {{
      display: inline-flex;
      align-items: center;
      gap: 8px;
      width: calc(100% - 18px);
      padding: 8px 10px;
      border-radius: 12px;
      text-decoration: none;
      color: var(--text);
    }}
    .tree-link:hover {{ background: rgba(31,122,107,0.08); }}
    .tree-link.current {{
      background: rgba(31,122,107,0.14);
      font-weight: 700;
    }}
    .tree-tag {{
      display: inline-flex;
      align-items: center;
      justify-content: center;
      min-width: 42px;
      padding: 4px 8px;
      border-radius: 999px;
      background: rgba(181,92,47,0.12);
      font-size: 0.76rem;
      letter-spacing: 0.04em;
      text-transform: uppercase;
      color: var(--accent-2);
    }}
    .tree-children {{
      margin-left: 18px;
      padding-left: 12px;
      border-left: 1px dashed rgba(64, 49, 21, 0.18);
    }}
    .panel-meta {{
      margin: 0 0 12px;
      color: var(--muted);
      font-size: 0.95rem;
    }}
    @media (max-width: 960px) {{
      .page {{ width: calc(100vw - 12px); }}
      .workspace {{
        grid-template-columns: 1fr;
        min-height: auto;
      }}
      .resize-handle {{ display: none; }}
      body.left-collapsed .workspace,
      body.right-collapsed .workspace,
      body.left-collapsed.right-collapsed .workspace {{
        grid-template-columns: 1fr;
      }}
    }}
  </style>
</head>
<body>
  <div class="page">
    <section class="hero">
      <div class="eyebrow">{eyebrow}</div>
      <h1>{page_title}</h1>
      <p class="lede">{intro}</p>
      <div class="breadcrumb">{breadcrumb_html}</div>
      <div class="summary-grid">{summary_html}</div>
    </section>
    <section class="toolbar" aria-label="Pane controls">
      <button id="toggle-left" type="button" aria-expanded="true" aria-controls="left-pane">Toggle tree</button>
      <button id="toggle-right" type="button" aria-expanded="true" aria-controls="right-pane">Toggle details</button>
    </section>
    <section class="workspace">
      <aside class="pane pane-left" id="left-pane" aria-label="Hierarchy tree">
        <div class="pane-inner">
          <h2>{left_title}</h2>
          <p class="panel-meta">Primary navigation path. HTML links and tree links are canonical; Mermaid clicks are enhancement only.</p>
          {left_panel_html}
        </div>
      </aside>
      <div class="resize-handle resize-left" data-side="left" aria-hidden="true"></div>
      <article class="center-pane">
        <h2>{center_title}</h2>
        <div class="mermaid" data-link-map='{graph_link_map_json}'>
{graph_definition}
        </div>
        <details>
          <summary>Graph source</summary>
          <pre>{graph_source_html}</pre>
        </details>
      </article>
      <div class="resize-handle resize-right" data-side="right" aria-hidden="true"></div>
      <aside class="pane pane-right" id="right-pane" aria-label="Details">
        <div class="pane-inner">
          <h2>{right_title}</h2>
          {right_panel_html}
        </div>
      </aside>
    </section>
  </div>
  <script>
    mermaid.initialize({{
      startOnLoad: true,
      securityLevel: "loose",
      theme: "base",
      flowchart: {{
        useMaxWidth: true,
        htmlLabels: true
      }},
      themeVariables: {{
        primaryColor: "#eae3d3",
        primaryTextColor: "#22170d",
        primaryBorderColor: "#83633f",
        lineColor: "#4f6f67",
        tertiaryColor: "#f7f3eb"
      }}
    }});

    function safeStorage() {{
      try {{
        const key = "__axon_docs_probe__";
        window.localStorage.setItem(key, "1");
        window.localStorage.removeItem(key);
        return window.localStorage;
      }} catch (_error) {{
        return null;
      }}
    }}

    const storage = safeStorage();

    function applyPaneState() {{
      if (!storage) {{
        return;
      }}
      const leftWidth = storage.getItem("axon-docs-left-width");
      const rightWidth = storage.getItem("axon-docs-right-width");
      const leftCollapsed = storage.getItem("axon-docs-left-collapsed") === "1";
      const rightCollapsed = storage.getItem("axon-docs-right-collapsed") === "1";
      if (leftWidth) {{
        document.documentElement.style.setProperty("--left-pane-width", leftWidth);
      }}
      if (rightWidth) {{
        document.documentElement.style.setProperty("--right-pane-width", rightWidth);
      }}
      document.body.classList.toggle("left-collapsed", leftCollapsed);
      document.body.classList.toggle("right-collapsed", rightCollapsed);
      const leftButton = document.getElementById("toggle-left");
      const rightButton = document.getElementById("toggle-right");
      if (leftButton) {{
        leftButton.setAttribute("aria-expanded", String(!leftCollapsed));
      }}
      if (rightButton) {{
        rightButton.setAttribute("aria-expanded", String(!rightCollapsed));
      }}
    }}

    function persistPaneState() {{
      if (!storage) {{
        return;
      }}
      storage.setItem("axon-docs-left-collapsed", document.body.classList.contains("left-collapsed") ? "1" : "0");
      storage.setItem("axon-docs-right-collapsed", document.body.classList.contains("right-collapsed") ? "1" : "0");
      storage.setItem("axon-docs-left-width", getComputedStyle(document.documentElement).getPropertyValue("--left-pane-width").trim() || "300px");
      storage.setItem("axon-docs-right-width", getComputedStyle(document.documentElement).getPropertyValue("--right-pane-width").trim() || "360px");
    }}

    function togglePane(side) {{
      const className = side === "left" ? "left-collapsed" : "right-collapsed";
      document.body.classList.toggle(className);
      const button = document.getElementById(side === "left" ? "toggle-left" : "toggle-right");
      if (button) {{
        button.setAttribute("aria-expanded", String(!document.body.classList.contains(className)));
      }}
      persistPaneState();
    }}

    function installPaneControls() {{
      const leftButton = document.getElementById("toggle-left");
      const rightButton = document.getElementById("toggle-right");
      if (leftButton) {{
        leftButton.addEventListener("click", () => togglePane("left"));
      }}
      if (rightButton) {{
        rightButton.addEventListener("click", () => togglePane("right"));
      }}

      document.querySelectorAll(".resize-handle[data-side]").forEach((handle) => {{
        handle.addEventListener("pointerdown", (event) => {{
          const side = handle.dataset.side;
          const startX = event.clientX;
          const startLeft = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--left-pane-width")) || 300;
          const startRight = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--right-pane-width")) || 360;
          const onMove = (moveEvent) => {{
            if (side === "left") {{
              const next = Math.max(180, Math.min(520, startLeft + (moveEvent.clientX - startX)));
              document.documentElement.style.setProperty("--left-pane-width", `${{next}}px`);
            }} else {{
              const next = Math.max(220, Math.min(620, startRight - (moveEvent.clientX - startX)));
              document.documentElement.style.setProperty("--right-pane-width", `${{next}}px`);
            }}
          }};
          const onUp = () => {{
            window.removeEventListener("pointermove", onMove);
            window.removeEventListener("pointerup", onUp);
            persistPaneState();
          }};
          window.addEventListener("pointermove", onMove);
          window.addEventListener("pointerup", onUp);
        }});
      }});
    }}

    function bindMermaidNodeLinks() {{
      document.querySelectorAll('.mermaid[data-link-map]').forEach((container) => {{
        let linkMap = {{}};
        try {{
          linkMap = JSON.parse(container.dataset.linkMap || '{{}}');
        }} catch (_error) {{
          linkMap = {{}};
        }}
        const svg = container.querySelector('svg');
        if (!svg) {{
          return;
        }}
        Object.entries(linkMap).forEach(([nodeId, href]) => {{
          const node = svg.querySelector(`g.node[id*="flowchart-${{nodeId}}-"]`);
          if (!node || node.dataset.axonBound === '1') {{
            return;
          }}
          node.dataset.axonBound = '1';
          node.style.cursor = 'pointer';
          node.addEventListener('click', () => {{
            window.location.href = href;
          }});
        }});
      }});
    }}

    window.addEventListener('load', () => {{
      applyPaneState();
      installPaneControls();
      let attempts = 0;
      const timer = window.setInterval(() => {{
        bindMermaidNodeLinks();
        attempts += 1;
        if (document.querySelector('.mermaid svg') || attempts >= 20) {{
          window.clearInterval(timer);
        }}
      }}, 150);
    }});
  </script>
</body>
</html>
"##,
        page_title = html_escape(page_title),
        eyebrow = html_escape(eyebrow),
        intro = html_escape(intro),
        breadcrumb_html = breadcrumb_html,
        left_title = html_escape(left_title),
        left_panel_html = left_panel_html,
        center_title = html_escape(center_title),
        summary_html = summary_html,
        graph_definition = graph.definition,
        graph_link_map_json = html_escape(&graph.link_map_json),
        graph_source_html = html_escape(&graph.definition),
        right_title = html_escape(right_title),
        right_panel_html = right_panel_html,
    )
}

fn project_scope_clause_for_table(id_column: &str, project_code: Option<&str>) -> String {
    project_code
        .map(|code| format!(" WHERE {} LIKE '%-{}-%'", id_column, escape_sql(code)))
        .unwrap_or_default()
}

fn project_scope_clause_for_relation(project_code: Option<&str>) -> String {
    project_code
        .map(|code| {
            let escaped = escape_sql(code);
            format!(
                " WHERE source_id LIKE '%-{}-%' OR target_id LIKE '%-{}-%'",
                escaped, escaped
            )
        })
        .unwrap_or_default()
}

impl McpServer {
    pub(crate) fn axon_soll_commit_revision(&self, args: &Value) -> Option<Value> {
        let preview_id = match args.get("preview_id").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                return Some(json!({
                    "content": [{"type":"text","text":"Missing required argument: preview_id"}],
                    "isError": true
                }));
            }
        };
        let author = args
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let preview_raw = match query_first_sql_cell(
            self,
            &format!(
                "SELECT payload FROM soll.RevisionPreview WHERE preview_id = '{}'",
                escape_sql(preview_id)
            ),
        ) {
            Some(v) => v,
            None => {
                return Some(json!({
                    "content": [{"type":"text","text": format!("Preview not found: {}", preview_id)}],
                    "isError": true
                }));
            }
        };
        let payload: Value = match serde_json::from_str(&preview_raw) {
            Ok(v) => v,
            Err(e) => {
                return Some(json!({
                    "content": [{"type":"text","text": format!("Invalid preview payload JSON: {}", e)}],
                    "isError": true
                }));
            }
        };
        let operations = payload
            .get("operations")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let project_code = payload
            .get("project_code")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");

        let revision_id = if let Some(reserved_revision_id) = args
            .get("reserved_revision_id")
            .and_then(|value| value.as_str())
        {
            reserved_revision_id.to_string()
        } else {
            let (_, project_code, _, next_revision) = match self
                .next_server_numeric_id(project_code, "revision")
            {
                Ok(parts) => parts,
                Err(e) => {
                    return Some(json!({
                        "content": [{"type":"text","text": format!("SOLL commit error (revision id): {}", e)}],
                        "isError": true
                    }))
                }
            };
            format!("REV-{}-{:03}", project_code, next_revision)
        };
        let now = now_unix_ms();
        let _ = self.graph_store.execute("BEGIN TRANSACTION");

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.Revision (revision_id, author, source, summary, status, created_at, committed_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            &json!([revision_id, author, "mcp", "SOLL plan commit", "committed", now, now]),
        ) {
            let _ = self.graph_store.execute("ROLLBACK");
            return Some(json!({"content":[{"type":"text","text": format!("SOLL commit error (revision row): {}", e)}],"isError": true}));
        }

        let mut identity_mapping = std::collections::HashMap::new();
        for op in &operations {
            match self.apply_operation_with_audit(&revision_id, op, &mut identity_mapping) {
                Ok(generated_id) => {
                    if !generated_id.is_empty() {
                        if let Some(lk) = op.get("logical_key").and_then(|v| v.as_str()) {
                            identity_mapping.insert(lk.to_string(), generated_id);
                        }
                    }
                }
                Err(e) => {
                    let _ = self.graph_store.execute("ROLLBACK");
                    return Some(
                        json!({"content":[{"type":"text","text": format!("SOLL commit error (operation): {}", e)}],"isError": true}),
                    );
                }
            }
        }

        let _ = self.graph_store.execute("COMMIT");
        let _ = self.graph_store.execute(&format!(
            "DELETE FROM soll.RevisionPreview WHERE preview_id = '{}'",
            escape_sql(preview_id)
        ));

        Some(json!({
            "content": [{"type":"text","text": format!("SOLL revision committed: {} ({} operations)", revision_id, operations.len())}],
            "data": {
                "revision_id": revision_id,
                "operations": operations.len(),
                "identity_mapping": identity_mapping
            }
        }))
    }

    pub(crate) fn axon_soll_query_context(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let project_code = self.resolve_project_code(project_code).ok()?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(25)
            .max(1);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let cache_key = format!("{}|{}", project_code, limit);
        if let Some(cached) = Self::read_soll_context_cache(&cache_key, now_ms) {
            return Some(cached);
        }

        let escaped_project = escape_sql(&project_code);
        let reqs = self
            .query_single_column(&format!(
                "SELECT id || '|' || title || '|' || COALESCE(status,'')
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Requirement'
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                limit = limit
            ))
            .unwrap_or_default();
        let visions = self
            .query_single_column(&format!(
                "SELECT id || '|' || title || '|' || COALESCE(status,'') || '|' || COALESCE(description,'')
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Vision'
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                limit = limit
            ))
            .unwrap_or_default();
        let decisions = self
            .query_single_column(&format!(
                "SELECT id || '|' || title || '|' || COALESCE(status,'')
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Decision'
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                limit = limit
            ))
            .unwrap_or_default();
        let revisions = self.query_single_column(&format!(
            "SELECT revision_id || '|' || COALESCE(summary,'') || '|' || COALESCE(author,'') FROM soll.Revision ORDER BY committed_at DESC LIMIT {}",
            limit
        )).unwrap_or_default();

        let response = json!({
            "content": [{"type":"text","text": format!("SOLL context for {} loaded.", project_code)}],
            "data": {
                "project_code": project_code,
                "visions": visions,
                "requirements": reqs,
                "decisions": decisions,
                "revisions": revisions
            }
        });
        Self::write_soll_context_cache(cache_key, now_ms, &response);
        Some(response)
    }

    pub(crate) fn axon_soll_work_plan(&self, args: &Value) -> Option<Value> {
        let project_code = args.get("project_code")?.as_str()?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .max(1) as usize;
        let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(5).max(1) as usize;
        let include_ist = args
            .get("include_ist")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("brief");

        let mut nodes = self.load_work_plan_nodes(project_code);
        let edges = self.load_work_plan_edges(project_code);
        let adjacency = build_adjacency_map(&edges);
        let cycle_sets = detect_cycle_sets(nodes.keys(), &adjacency);
        let cycle_node_ids = cycle_sets
            .iter()
            .flat_map(|set| set.iter().cloned())
            .collect::<HashSet<_>>();
        let blocked_by_cycles = collect_blocked_by_cycles(&adjacency, &cycle_node_ids);
        let backlog_visible = self
            .project_scope_summary(Some(project_code))
            .map(|summary| summary.backlog_files > 0)
            .unwrap_or(false);

        for node in nodes.values_mut() {
            node.backlog_visible = backlog_visible;
            if include_ist {
                node.ist_degraded_links = self.count_degraded_links_for_node(&node.id);
                if node.ist_degraded_links > 0 {
                    node.ist_signals.push(format!(
                        "{} lien(s) vers un scope `indexed_degraded`",
                        node.ist_degraded_links
                    ));
                }
            }
        }

        let schedulable_ids = nodes
            .keys()
            .filter(|id| !cycle_node_ids.contains(*id) && !blocked_by_cycles.contains(*id))
            .cloned()
            .collect::<HashSet<_>>();
        let schedulable_adj = filter_adjacency(&adjacency, &schedulable_ids);
        let descendants = compute_descendant_counts(&schedulable_ids, &schedulable_adj);

        for node in nodes.values_mut() {
            node.descendants = *descendants.get(&node.id).unwrap_or(&0);
            let (score, reasons, gates) = score_node(node, include_ist);
            node.score = score;
            node.reasons = reasons;
            node.validation_gates = gates;
        }

        let waves = build_waves(&nodes, &edges, &schedulable_ids);
        let cycles = cycle_sets
            .into_iter()
            .map(|set| {
                let mut node_ids = set.into_iter().collect::<Vec<_>>();
                node_ids.sort();
                WorkPlanCycle { node_ids }
            })
            .collect::<Vec<_>>();

        let mut blockers = cycle_node_ids
            .iter()
            .filter_map(|id| nodes.get(id))
            .map(|node| WorkPlanBlocker {
                id: node.id.clone(),
                entity_type: node.entity_type.label().to_string(),
                reason: "in_cycle".to_string(),
            })
            .collect::<Vec<_>>();
        blockers.extend(
            blocked_by_cycles
                .iter()
                .filter_map(|id| nodes.get(id))
                .map(|node| WorkPlanBlocker {
                    id: node.id.clone(),
                    entity_type: node.entity_type.label().to_string(),
                    reason: "depends_on_cycle".to_string(),
                }),
        );
        blockers.sort_by(|a, b| a.id.cmp(&b.id));

        let (limited_waves, returned_items, truncated) = apply_wave_limit(&waves, limit);
        let top_recommendations = build_top_recommendations(&limited_waves, top);
        let global_validation =
            self.axon_soll_verify_requirements(&json!({ "project_code": project_code }));
        let soll_validation = self.axon_validate_soll(&json!({ "project_code": project_code }));
        let completeness_snapshot = self.soll_completeness_snapshot(Some(project_code)).ok();
        let validation_gates = json!({
            "requirement_verification": global_validation
                .as_ref()
                .and_then(|resp| resp.get("data"))
                .cloned()
                .unwrap_or(json!({})),
            "soll_validation": soll_validation
                .as_ref()
                .and_then(|resp| resp.get("data"))
                .cloned()
                .unwrap_or(json!({})),
            "completeness_axes": completeness_snapshot
                .map(|snapshot| json!({
                    "concept_completeness": snapshot.concept_complete(),
                    "implementation_completeness": snapshot.implementation_complete(),
                    "evidence_ready": snapshot.evidence_ready()
                }))
                .unwrap_or_else(|| json!({})),
            "backlog_visible": backlog_visible
        });
        let data = json!({
            "summary": {
                "project_code": project_code,
                "total_nodes": nodes.len(),
                "schedulable_nodes": schedulable_ids.len(),
                "blocked_nodes": blockers.len(),
                "cycle_count": cycles.len(),
                "wave_count": waves.len(),
                "returned_items": returned_items,
                "top_count": top_recommendations.len()
            },
            "blockers": blockers.iter().map(blocker_to_json).collect::<Vec<_>>(),
            "cycles": cycles.iter().map(cycle_to_json).collect::<Vec<_>>(),
            "ordered_waves": limited_waves.iter().map(wave_to_json).collect::<Vec<_>>(),
            "top_recommendations": top_recommendations,
            "validation_gates": validation_gates,
            "metadata": {
                "algorithm_version": "v1",
                "include_ist": include_ist,
                "generated_at": now_unix_ms(),
                "truncated": truncated,
                "limit": limit,
                "top": top
            }
        });

        let text = if format == "json" {
            format!("SOLL work plan generated for {}.", project_code)
        } else {
            self.render_work_plan_text(
                project_code,
                &limited_waves,
                &blockers,
                &cycles,
                &top_recommendations,
                truncated,
            )
        };

        Some(json!({
            "content": [{"type":"text","text": text}],
            "data": data
        }))
    }

    pub(crate) fn axon_soll_attach_evidence(&self, args: &Value) -> Option<Value> {
        let entity_type = args.get("entity_type")?.as_str()?;
        let entity_id = args.get("entity_id")?.as_str()?;
        let artifacts = args.get("artifacts")?.as_array()?;
        let mut attached = 0usize;
        let now = now_unix_ms();
        let normalized_entity_type = normalize_traceability_entity_type(entity_type);

        for (idx, art) in artifacts.iter().enumerate() {
            let artifact_type = art
                .get("artifact_type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let artifact_ref = art
                .get("artifact_ref")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if artifact_ref.is_empty() {
                continue;
            }
            let confidence = art
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.8);
            let metadata = art
                .get("metadata")
                .cloned()
                .unwrap_or(json!({}))
                .to_string();
            let trace_id = format!("TRC-{}-{}-{}", entity_id, now, idx);

            if self.graph_store.execute_param(
                "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &json!([trace_id, normalized_entity_type, entity_id, artifact_type, artifact_ref, confidence, metadata, now]),
            ).is_ok() {
                attached += 1;
            }
        }

        Some(json!({
            "content": [{"type":"text","text": format!("Attached {} evidence item(s) to {}:{}", attached, entity_type, entity_id)}],
            "data": {"attached": attached, "normalized_entity_type": normalize_traceability_entity_type(entity_type)}
        }))
    }

    fn load_work_plan_nodes(&self, project_code: &str) -> HashMap<String, WorkPlanNode> {
        let Ok(project_code) = self.resolve_project_code(project_code) else {
            return HashMap::new();
        };
        let mut nodes = HashMap::new();
        let requirement_coverage = self
            .requirement_coverage_summary(&project_code)
            .unwrap_or_default();
        let requirement_coverage_by_id = requirement_coverage
            .entries
            .iter()
            .map(|entry| (entry.id.clone(), entry.clone()))
            .collect::<HashMap<_, _>>();
        let req_query = format!(
            "SELECT r.id, r.title, COALESCE(r.status,''), COALESCE(r.metadata,'{{}}')
             FROM soll.Node r
             WHERE r.type = 'Requirement' AND r.id LIKE 'REQ-{}-%'
             ORDER BY r.id",
            escape_sql(&project_code)
        );
        if let Ok(raw) = self.graph_store.query_json(&req_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() < 4 {
                    continue;
                }
                let meta: serde_json::Value =
                    serde_json::from_str(&row[3]).unwrap_or(serde_json::json!({}));
                let priority = meta
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let status = row[2].clone();
                let id = row[0].clone();
                let coverage_entry = requirement_coverage_by_id.get(&id);
                nodes.insert(
                    id.clone(),
                    WorkPlanNode {
                        id,
                        title: row[1].clone(),
                        entity_type: WorkPlanEntityType::Requirement,
                        status,
                        priority,
                        requirement_state: Some(
                            coverage_entry
                                .map(|entry| entry.state.clone())
                                .unwrap_or_else(|| "missing".to_string()),
                        ),
                        evidence_count: coverage_entry
                            .map(|entry| entry.evidence_count)
                            .unwrap_or(0),
                        descendants: 0,
                        ist_degraded_links: 0,
                        backlog_visible: false,
                        score: 0,
                        reasons: Vec::new(),
                        validation_gates: Vec::new(),
                        ist_signals: Vec::new(),
                    },
                );
            }
        }

        let dec_query = format!(
            "SELECT id, title, COALESCE(status,'') FROM soll.Node WHERE type='Decision' AND id LIKE 'DEC-{}-%' ORDER BY id",
            escape_sql(&project_code)
        );
        if let Ok(raw) = self.graph_store.query_json(&dec_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() < 3 {
                    continue;
                }
                let id = row[0].clone();
                nodes.insert(
                    id.clone(),
                    WorkPlanNode {
                        id,
                        title: row[1].clone(),
                        entity_type: WorkPlanEntityType::Decision,
                        status: row[2].clone(),
                        priority: String::new(),
                        requirement_state: None,
                        evidence_count: 0,
                        descendants: 0,
                        ist_degraded_links: 0,
                        backlog_visible: false,
                        score: 0,
                        reasons: Vec::new(),
                        validation_gates: Vec::new(),
                        ist_signals: Vec::new(),
                    },
                );
            }
        }

        let mil_query = format!(
            "SELECT id, title, COALESCE(status,'') FROM soll.Node WHERE type='Milestone' AND id LIKE 'MIL-{}-%' ORDER BY id",
            escape_sql(&project_code)
        );
        if let Ok(raw) = self.graph_store.query_json(&mil_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() < 3 {
                    continue;
                }
                let id = row[0].clone();
                nodes.insert(
                    id.clone(),
                    WorkPlanNode {
                        id,
                        title: row[1].clone(),
                        entity_type: WorkPlanEntityType::Milestone,
                        status: row[2].clone(),
                        priority: String::new(),
                        requirement_state: None,
                        evidence_count: 0,
                        descendants: 0,
                        ist_degraded_links: 0,
                        backlog_visible: false,
                        score: 0,
                        reasons: Vec::new(),
                        validation_gates: Vec::new(),
                        ist_signals: Vec::new(),
                    },
                );
            }
        }

        nodes
    }

    fn load_work_plan_edges(&self, project_code: &str) -> Vec<(String, String)> {
        let Ok(project_code) = self.resolve_project_code(project_code) else {
            return Vec::new();
        };
        let mut edges = Vec::new();
        let solves_query = format!(
            "SELECT source_id, target_id FROM soll.Edge WHERE relation_type='SOLVES' AND source_id LIKE 'DEC-{}-%' AND target_id LIKE 'REQ-{}-%'",
            escape_sql(&project_code),
            escape_sql(&project_code)
        );
        if let Ok(raw) = self.graph_store.query_json(&solves_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() >= 2 {
                    edges.push((row[0].clone(), row[1].clone()));
                }
            }
        }

        let belongs_query = format!(
            "SELECT source_id, target_id FROM soll.Edge WHERE relation_type='BELONGS_TO' AND source_id LIKE 'REQ-{}-%' AND (target_id LIKE 'REQ-{}-%' OR target_id LIKE 'MIL-{}-%')",
            escape_sql(&project_code),
            escape_sql(&project_code),
            escape_sql(&project_code)
        );
        if let Ok(raw) = self.graph_store.query_json(&belongs_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() >= 2 {
                    edges.push((row[0].clone(), row[1].clone()));
                }
            }
        }

        edges.sort();
        edges.dedup();
        edges
    }

    fn count_degraded_links_for_node(&self, node_id: &str) -> usize {
        let degraded_file_query = format!(
            "SELECT count(*) FROM (
                SELECT DISTINCT f.path
                FROM SUBSTANTIATES rel
                JOIN File f ON (
                    (rel.source_id = '{id}' AND rel.target_id = f.path)
                    OR (rel.target_id = '{id}' AND rel.source_id = f.path)
                )
                WHERE f.status = 'indexed_degraded'
                UNION
                SELECT DISTINCT f.path
                FROM IMPACTS rel
                JOIN File f ON (
                    (rel.source_id = '{id}' AND rel.target_id = f.path)
                    OR (rel.target_id = '{id}' AND rel.source_id = f.path)
                )
                WHERE f.status = 'indexed_degraded'
                UNION
                SELECT DISTINCT f.path
                FROM SUBSTANTIATES rel
                JOIN CONTAINS c ON (
                    (rel.source_id = '{id}' AND rel.target_id = c.target_id)
                    OR (rel.target_id = '{id}' AND rel.source_id = c.target_id)
                )
                JOIN File f ON f.path = c.source_id
                WHERE f.status = 'indexed_degraded'
                UNION
                SELECT DISTINCT f.path
                FROM IMPACTS rel
                JOIN CONTAINS c ON (
                    (rel.source_id = '{id}' AND rel.target_id = c.target_id)
                    OR (rel.target_id = '{id}' AND rel.source_id = c.target_id)
                )
                JOIN File f ON f.path = c.source_id
                WHERE f.status = 'indexed_degraded'
            ) t",
            id = escape_sql(node_id)
        );
        self.graph_store
            .query_count(&degraded_file_query)
            .unwrap_or(0)
            .max(0) as usize
    }

    fn render_work_plan_text(
        &self,
        project_code: &str,
        waves: &[WorkPlanWave],
        blockers: &[WorkPlanBlocker],
        cycles: &[WorkPlanCycle],
        top_recommendations: &[Value],
        truncated: bool,
    ) -> String {
        let mut evidence = String::new();
        if !top_recommendations.is_empty() {
            evidence.push_str("Immediate actions:\n");
            for rec in top_recommendations {
                let id = rec.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
                let kind = rec.get("kind").and_then(|v| v.as_str()).unwrap_or("task");
                let reason = rec
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("action immediate");
                evidence.push_str(&format!("- {} [{}] : {}\n", id, kind, reason));
            }
            evidence.push('\n');
        }
        if !blockers.is_empty() {
            evidence.push_str("Blockers:\n");
            for blocker in blockers {
                evidence.push_str(&format!(
                    "- {} ({}) : {}\n",
                    blocker.id, blocker.entity_type, blocker.reason
                ));
            }
            evidence.push('\n');
        }
        if !cycles.is_empty() {
            evidence.push_str("Cycles:\n");
            for cycle in cycles {
                evidence.push_str(&format!("- {}\n", cycle.node_ids.join(" -> ")));
            }
            evidence.push('\n');
        }
        for wave in waves {
            evidence.push_str(&format!("Wave {}:\n", wave.wave_index));
            for item in &wave.items {
                evidence.push_str(&format!(
                    "- {} [{}] score={} :: {}\n",
                    item.id,
                    item.entity_type.label(),
                    item.score,
                    item.reasons.join(", ")
                ));
            }
            evidence.push('\n');
        }
        if truncated {
            evidence.push_str("[truncated=true]\n");
        }
        format!(
            "### 🗺️ SOLL Work Plan: {}\n\n{}",
            project_code,
            format_standard_contract(
                "ok",
                "work plan computed from SOLL",
                &format!("project:{}", project_code),
                &evidence,
                &[
                    "review blockers before execution",
                    "use `format=json` for machine consumption"
                ],
                "medium",
            )
        )
    }

    pub(crate) fn axon_soll_verify_requirements(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let project_code = self.resolve_project_code(project_code).ok()?;
        let summary = self.requirement_coverage_summary(&project_code).ok()?;
        let snapshot = self.soll_completeness_snapshot(Some(&project_code)).ok()?;
        let details = summary
            .entries
            .iter()
            .map(|entry| {
                json!({
                    "id": entry.id,
                    "state": entry.state,
                    "status": entry.status,
                    "evidence_count": entry.evidence_count
                })
            })
            .collect::<Vec<_>>();

        Some(json!({
            "content": [{"type":"text","text": format!("Requirement verification: done={}, partial={}, missing={}", summary.done, summary.partial, summary.missing)}],
            "data": {
                "project_code": project_code,
                "done": summary.done,
                "partial": summary.partial,
                "missing": summary.missing,
                "details": details,
                "completeness_axes": {
                    "concept_completeness": snapshot.concept_complete(),
                    "implementation_completeness": snapshot.implementation_complete(),
                    "evidence_ready": snapshot.evidence_ready()
                },
                "guidance_source": "server-side canonical soll completeness evaluator"
            }
        }))
    }

    pub(crate) fn axon_soll_rollback_revision(&self, args: &Value) -> Option<Value> {
        let revision_id = args.get("revision_id")?.as_str()?;
        let query = format!(
            "SELECT entity_type, entity_id, action, before_json, after_json
             FROM soll.RevisionChange
             WHERE revision_id = '{}'
             ORDER BY created_at DESC",
            escape_sql(revision_id)
        );
        let rows_raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();

        let _ = self.graph_store.execute("BEGIN TRANSACTION");
        for row in rows {
            if row.len() < 5 {
                continue;
            }
            let entity_type = &row[0];
            let entity_id = &row[1];
            let action = &row[2];
            let before_json = &row[3];

            let op = if action == "create" {
                json!({"kind":"delete", "entity": entity_type, "entity_id": entity_id})
            } else {
                let before_val: Value = serde_json::from_str(before_json).unwrap_or(json!({}));
                json!({"kind":"restore", "entity": entity_type, "entity_id": entity_id, "before": before_val})
            };

            if let Err(e) = self.apply_rollback_operation(&op) {
                let _ = self.graph_store.execute("ROLLBACK");
                return Some(
                    json!({"content":[{"type":"text","text": format!("Rollback failed: {}", e)}],"isError": true}),
                );
            }
        }

        let _ = self.graph_store.execute("COMMIT");
        let _ = self.graph_store.execute(&format!(
            "UPDATE soll.Revision SET status = 'rolled_back' WHERE revision_id = '{}'",
            escape_sql(revision_id)
        ));
        Some(
            json!({"content":[{"type":"text","text": format!("Revision rolled back: {}", revision_id)}]}),
        )
    }

    fn build_plan_operations(&self, project_code: &str, args: &Value) -> Vec<Value> {
        let mut operations = Vec::new();

        // 1. Entities
        if let Some(plan) = args.get("plan") {
            for entity in [
                "pillar",
                "requirement",
                "decision",
                "milestone",
                "vision",
                "concept",
            ] {
                if let Some(items) = plan.get(format!("{}s", entity)).and_then(|v| v.as_array()) {
                    for item in items {
                        if let Some(obj) = item.as_object() {
                            let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("");
                            let logical_key = obj
                                .get("logical_key")
                                .and_then(|v| v.as_str())
                                .unwrap_or(title);
                            if logical_key.is_empty() {
                                continue;
                            }
                            let existing_id =
                                self.resolve_soll_id(entity, project_code, title, logical_key);
                            let kind = if existing_id.is_some() {
                                "update"
                            } else {
                                "create"
                            };
                            operations.push(json!({
                                "kind": kind,
                                "entity": entity,
                                "project_code": project_code,
                                "logical_key": logical_key,
                                "entity_id": existing_id,
                                "payload": Value::Object(obj.clone())
                            }));
                        }
                    }
                }
            }
        }

        // 2. Relations
        if let Some(relations) = args.get("relations").and_then(|v| v.as_array()) {
            for rel in relations {
                if let Some(obj) = rel.as_object() {
                    operations.push(json!({
                        "kind": "link",
                        "entity": "relation",
                        "project_code": project_code,
                        "payload": Value::Object(obj.clone())
                    }));
                }
            }
        }

        operations
    }

    fn apply_operation_with_audit(
        &self,
        revision_id: &str,
        op: &Value,
        identity_mapping: &mut std::collections::HashMap<String, String>,
    ) -> anyhow::Result<String> {
        let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("create");
        let entity = op
            .get("entity")
            .and_then(|v| v.as_str())
            .unwrap_or("requirement");
        let mut payload = op.get("payload").cloned().unwrap_or(serde_json::json!({}));
        let project_code = op
            .get("project_code")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");

        if kind == "link" {
            if let Some(obj) = payload.as_object_mut() {
                if let Some(sid) = obj.get("source_id").and_then(|v| v.as_str()) {
                    if let Some(canon) = identity_mapping.get(sid) {
                        obj.insert("source_id".to_string(), serde_json::json!(canon));
                    }
                }
                if let Some(tid) = obj.get("target_id").and_then(|v| v.as_str()) {
                    if let Some(canon) = identity_mapping.get(tid) {
                        obj.insert("target_id".to_string(), serde_json::json!(canon));
                    }
                }
            }

            let result = self.axon_soll_manager(
                &serde_json::json!({"action":"link","entity":"relation","data":payload}),
            );
            if soll_tool_is_error(result.as_ref()) {
                return Err(anyhow::anyhow!(
                    "{}",
                    soll_tool_text(result.as_ref()).unwrap_or_else(|| "link error".to_string())
                ));
            }
            return Ok("".to_string());
        }

        let entity_id_hint = op
            .get("entity_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let before = if let Some(id) = entity_id_hint.clone() {
            self.snapshot_entity(entity, &id)
                .unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let result = if kind == "update" && entity_id_hint.is_some() {
            let mut data = payload.clone();
            data["id"] = serde_json::json!(entity_id_hint.clone().unwrap_or_default());
            self.axon_soll_manager(
                &serde_json::json!({"action":"update","entity":entity,"data":data}),
            )
        } else {
            let mut data = payload.clone();
            data["project_code"] = serde_json::json!(project_code);
            self.axon_soll_manager(
                &serde_json::json!({"action":"create","entity":entity,"data":data}),
            )
        };

        if soll_tool_is_error(result.as_ref()) {
            return Err(anyhow::anyhow!(
                "{}",
                soll_tool_text(result.as_ref()).unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        let entity_id = if let Some(id) = entity_id_hint {
            id
        } else {
            soll_tool_text(result.as_ref())
                .and_then(extract_soll_id_from_message)
                .unwrap_or_else(|| "unknown".to_string())
        };

        let after = self
            .snapshot_entity(entity, &entity_id)
            .unwrap_or(serde_json::json!({}));
        self.graph_store.execute_param(
            "INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, action, before_json, after_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            &serde_json::json!([
                revision_id,
                entity,
                entity_id,
                kind,
                before.to_string(),
                after.to_string(),
                now_unix_ms()
            ]),
        )?;

        Ok(entity_id)
    }

    fn apply_rollback_operation(&self, op: &Value) -> anyhow::Result<()> {
        let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let entity = op.get("entity").and_then(|v| v.as_str()).unwrap_or("");
        let entity_id = op.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");

        match (kind, entity) {
            ("delete", "pillar") => self.graph_store.execute(&format!(
                "DELETE FROM soll.Node WHERE type='Pillar' AND id = '{}'",
                escape_sql(entity_id)
            ))?,
            ("delete", "requirement") => self.graph_store.execute(&format!(
                "DELETE FROM soll.Node WHERE type='Requirement' AND id = '{}'",
                escape_sql(entity_id)
            ))?,
            ("delete", "decision") => self.graph_store.execute(&format!(
                "DELETE FROM soll.Node WHERE type='Decision' AND id = '{}'",
                escape_sql(entity_id)
            ))?,
            ("delete", "milestone") => self.graph_store.execute(&format!(
                "DELETE FROM soll.Node WHERE type='Milestone' AND id = '{}'",
                escape_sql(entity_id)
            ))?,
            ("restore", _) => {
                let before = op.get("before").cloned().unwrap_or(json!({}));
                let mut data = before;
                data["id"] = json!(entity_id);
                let resp =
                    self.axon_soll_manager(&json!({"action":"update","entity":entity,"data":data}));
                if soll_tool_is_error(resp.as_ref()) {
                    return Err(anyhow!(
                        "{}",
                        soll_tool_text(resp.as_ref()).unwrap_or_default()
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot_entity(&self, entity: &str, entity_id: &str) -> Option<Value> {
        let query = match entity {
            "pillar" => format!("SELECT title, description, metadata FROM soll.Node WHERE type='Pillar' AND id = '{}'", escape_sql(entity_id)),
            "requirement" => format!("SELECT title, description, status, metadata FROM soll.Node WHERE type='Requirement' AND id = '{}'", escape_sql(entity_id)),
            "decision" => format!("SELECT title, description, status, metadata FROM soll.Node WHERE type='Decision' AND id = '{}'", escape_sql(entity_id)),
            "milestone" => format!("SELECT title, status, metadata FROM soll.Node WHERE type='Milestone' AND id = '{}'", escape_sql(entity_id)),
            "guideline" => format!("SELECT title, description, status, metadata FROM soll.Node WHERE type='Guideline' AND id = '{}'", escape_sql(entity_id)),
            _ => return None,
        };
        let raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).ok()?;
        let first = rows.first()?;
        match entity {
            "pillar" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "metadata": first.get(2).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            "requirement" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "status": first.get(2).cloned().unwrap_or_default(),
                "priority": first.get(3).cloned().unwrap_or_default(),
                "metadata": first.get(4).cloned().unwrap_or_else(|| "{}".to_string()),
                "owner": first.get(5).cloned().unwrap_or_default(),
                "acceptance_criteria": first.get(6).cloned().unwrap_or_else(|| "[]".to_string()),
                "evidence_refs": first.get(7).cloned().unwrap_or_else(|| "[]".to_string())
            })),
            "decision" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "context": first.get(2).cloned().unwrap_or_default(),
                "rationale": first.get(3).cloned().unwrap_or_default(),
                "status": first.get(4).cloned().unwrap_or_default(),
                "metadata": first.get(5).cloned().unwrap_or_else(|| "{}".to_string()),
                "supersedes_decision_id": first.get(6).cloned().unwrap_or_default(),
                "impact_scope": first.get(7).cloned().unwrap_or_default()
            })),
            "milestone" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "status": first.get(1).cloned().unwrap_or_default(),
                "metadata": first.get(2).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            _ => None,
        }
    }

    pub(crate) fn axon_commit_work(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        let diff_paths = args.get("diff_paths")?.as_array()?;
        let message = args.get("message")?.as_str()?;
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Extract guidelines
        let rows_raw = self.graph_store.query_json(
            "SELECT id, title, description, metadata FROM soll.Node WHERE type='Guideline' AND status='active'"
        ).unwrap_or_else(|_| "[]".to_string());

        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();

        let mut violations = Vec::new();

        for row in rows {
            if row.len() < 4 {
                continue;
            }
            let id = &row[0];
            let meta: serde_json::Value =
                serde_json::from_str(&row[3]).unwrap_or_else(|_| serde_json::json!({}));

            let trigger_path = meta
                .get("trigger_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let required_path = meta
                .get("required_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let enforcement = meta
                .get("enforcement")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if trigger_path.is_empty() || required_path.is_empty() || enforcement != "strict" {
                continue;
            }

            // Check if any diff_path matches trigger_path
            let trigger_clean = trigger_path.replace("*", "");
            let triggered = diff_paths.iter().any(|p| {
                if let Some(path_str) = p.as_str() {
                    path_str.contains(&trigger_clean)
                } else {
                    false
                }
            });

            if triggered {
                // Check if any diff_path matches required_path
                let satisfied = diff_paths.iter().any(|p| {
                    if let Some(path_str) = p.as_str() {
                        path_str.contains(required_path)
                    } else {
                        false
                    }
                });

                if !satisfied {
                    let phase = meta.get("phase").and_then(|v| v.as_str()).unwrap_or("");
                    let phase_str = if phase.is_empty() {
                        "".to_string()
                    } else {
                        format!(" [Phase: {}]", phase)
                    };
                    violations.push(serde_json::json!({
                        "rule": format!("{} - {}", id, row[1]),
                        "diagnostic": format!("Le chemin modifié déclenche la règle {}{}, qui exige que le fichier requis '{}' soit modifié.", id, phase_str, required_path),
                        "remediation_plan": format!("1. Mettez à jour le fichier '{}'.\n2. Rappelez axon_commit_work.", required_path)
                    }));
                }
            }
        }

        if !violations.is_empty() {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Violation de {}\n\nVoici le remediation_plan:\n{}", violations[0]["rule"], violations[0]["remediation_plan"]) }],
                "isError": true,
                "data": { "violations": violations }
            }));
        }

        if dry_run {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Validation réussie (Dry Run). Aucun commit effectué. Le message '{}' est valide.", message) }]
            }));
        }

        // 1. Execute SOLL export
        let export_args = serde_json::json!({});
        let export_res = self.axon_export_soll(&export_args);
        let mut export_report = String::new();
        if let Some(res) = export_res {
            if soll_tool_is_error(Some(&res)) {
                return Some(res); // Early return if export fails
            }
            if let Some(txt) = soll_tool_text(Some(&res)) {
                export_report = txt;
            }
        }

        // 2. Perform Git Commit
        let mut add_cmd = std::process::Command::new("git");
        add_cmd.arg("add");
        for p in diff_paths {
            if let Some(path_str) = p.as_str() {
                add_cmd.arg(path_str);
            }
        }
        let add_out = add_cmd.output();
        if let Err(e) = add_out {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Git add failed: {}", e) }],
                "isError": true
            }));
        }

        let commit_out = std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg(message)
            .output();

        match commit_out {
            Ok(output) => {
                let status = if output.status.success() {
                    format!(
                        "Commit effectué avec succès.\n{}",
                        String::from_utf8_lossy(&output.stdout)
                    )
                } else {
                    format!(
                        "Commit échoué.\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    )
                };
                Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Validation réussie.\n\n{}\n\nExport Report (not auto-staged):\n{}", status, export_report) }]
                }))
            }
            Err(e) => Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Git commit failed: {}", e) }],
                "isError": true
            })),
        }
    }

    pub(crate) fn axon_init_project(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        let project_path = match args.get("project_path").and_then(|value| value.as_str()) {
            Some(path) if !path.trim().is_empty() => path.trim(),
            _ => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": "`project_path` est obligatoire pour `axon_init_project`." }],
                    "isError": true
                }))
            }
        };
        let project_name = match self.derive_project_name_from_path(project_path) {
            Ok(name) => name,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet: {}", e) }],
                    "isError": true
                }))
            }
        };
        let project_code = match self.assign_project_code_for_init(&project_name, project_path) {
            Ok(code) => code,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                    "isError": true
                }))
            }
        };
        if let Some(requested_code) = args.get("project_code").and_then(|value| value.as_str()) {
            let requested = match self
                .validate_explicit_canonical_project_code(Some(requested_code), "axon_init_project")
            {
                Ok(code) => code,
                Err(e) => {
                    return Some(serde_json::json!({
                        "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                        "isError": true
                    }))
                }
            };
            if requested != project_code {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: `project_code` est attribué par le serveur. Omettez-le ou utilisez `{}` pour ce projet.", project_code) }],
                    "isError": true
                }));
            }
        }
        let concept_text = args
            .get("concept_document_url_or_text")
            .and_then(|v| v.as_str());

        // 1. Register project
        if let Err(e) = self.graph_store.sync_project_registry_entry(
            &project_code,
            Some(&project_name),
            Some(project_path),
        ) {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Erreur lors de l'enregistrement du projet: {}", e) }],
                "isError": true
            }));
        }
        if let Err(e) = self.ensure_soll_registry_row(&project_code) {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Erreur lors de l'initialisation SOLL du projet: {}", e) }],
                "isError": true
            }));
        }

        // 2. Fetch global guidelines
        let rows_raw = self.graph_store.query_json(
            "SELECT id, title, description, metadata FROM soll.Node WHERE type='Guideline' AND project_code='PRO'"
        ).unwrap_or_else(|_| "[]".to_string());

        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();

        let mut rules_text = String::new();
        for row in rows {
            if row.len() >= 3 {
                rules_text.push_str(&format!("- **{}**: {} ({})\n", row[0], row[1], row[2]));
            }
        }

        // 3. Prepare response
        let mut response_text = format!(
            "Projet '{}' ({}) initialisé avec succès dans Axon.\n\n",
            project_name, project_code
        );

        if concept_text.is_some() {
            response_text.push_str(&format!(
                "📄 Un document de concept a été détecté. Extrayez-en la Vision et les Piliers, et utilisez `soll_manager` pour les créer sous le projet {}.\n\n",
                project_code
            ));
        }

        response_text.push_str(&format!(
            "Code projet attribué par le serveur: `{}`.\n\n",
            project_code
        ));
        response_text.push_str("Voici les règles globales disponibles. Lesquelles souhaitez-vous activer, ignorer ou spécialiser pour ce projet ?\n");
        response_text.push_str(&rules_text);
        response_text
            .push_str("\n(Utilisez l'outil `axon_apply_guidelines` pour appliquer ces choix).");

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": response_text }],
            "data": {
                "project_code": project_code,
                "project_name": project_name,
                "project_path": project_path
            }
        }))
    }

    pub(crate) fn axon_apply_guidelines(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let project_code = match self.require_registered_mutation_project_code(
            args.get("project_code").and_then(|value| value.as_str()),
            "axon_apply_guidelines",
        ) {
            Ok(code) => code,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                    "isError": true
                }))
            }
        };
        let accepted_ids = args.get("accepted_global_rule_ids")?.as_array()?;

        let mut applied = Vec::new();
        for id_val in accepted_ids {
            let global_id = id_val.as_str().unwrap_or("");

            // Fetch global rule
            let row_raw = self.graph_store.query_json(&format!(
                "SELECT title, description, metadata FROM soll.Node WHERE id = '{}' AND type='Guideline'",
                escape_sql(global_id)
            )).unwrap_or_else(|_| "[]".to_string());

            let rows: Vec<Vec<String>> = serde_json::from_str(&row_raw).unwrap_or_default();
            if let Some(row) = rows.first() {
                if row.len() < 3 {
                    continue;
                }
                let title = &row[0];
                let desc = &row[1];
                let meta = &row[2];

                // Create local rule
                let (_scope_code, p_code, prefix, num) = match self
                    .next_soll_numeric_id(&project_code, "guideline")
                {
                    Ok(parts) => parts,
                    Err(e) => {
                        return Some(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Erreur registre SOLL: {}", e) }],
                            "isError": true
                        }))
                    }
                };
                let local_id = format!("{}-{}-{:03}", prefix, p_code, num);

                // Insert local rule
                let _ = self.graph_store.execute_param(
                    "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) 
                     VALUES (?, 'Guideline', ?, ?, ?, 'active', ?)",
                    &serde_json::json!([local_id, p_code, title, desc, meta])
                );

                // Insert edge
                let _ = self.graph_store.execute_param(
                    "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata) VALUES (?, ?, 'INHERITS_FROM', '{}')",
                    &serde_json::json!([local_id, global_id])
                );

                applied.push(local_id);
            }
        }

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": format!("Héritage appliqué. Nouvelles règles locales créées: {:?}", applied) }]
        }))
    }
}

fn build_top_recommendations(waves: &[WorkPlanWave], top: usize) -> Vec<Value> {
    let mut recommendations = Vec::new();
    for wave in waves {
        for item in &wave.items {
            recommendations.push(json!({
                "id": item.id,
                "entity_type": item.entity_type.label(),
                "title": item.title,
                "score": item.score,
                "wave_index": wave.wave_index,
                "kind": recommendation_kind(item),
                "reason": recommendation_reason(item),
                "validation_gates": item.validation_gates
            }));
            if recommendations.len() >= top {
                return recommendations;
            }
        }
    }
    recommendations
}

fn summarize_ops(ops: &[Value]) -> (usize, usize) {
    let mut creates = 0usize;
    let mut updates = 0usize;
    for op in ops {
        match op.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
            "create" => creates += 1,
            "update" => updates += 1,
            _ => {}
        }
    }
    (creates, updates)
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_numeric_suffix(value: &str) -> Option<u64> {
    let head = value.split(':').next()?.trim();
    head.rsplit('-').next()?.parse::<u64>().ok()
}

fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}
