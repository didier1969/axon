use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkEndpointKind {
    Soll(&'static str),
    Artifact,
}

impl LinkEndpointKind {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Soll(prefix) => prefix,
            Self::Artifact => "ART",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ProjectionRole {
    Primary,
    Lateral,
    Supporting,
}

impl ProjectionRole {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Lateral => "lateral",
            Self::Supporting => "supporting",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct KindProjectionPolicy {
    pub(super) breadcrumb_eligible: bool,
    pub(super) root_eligible: bool,
    pub(super) tree_order_rank: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RelationProjectionPolicy {
    pub(super) role: ProjectionRole,
    pub(super) parent_preference_rank: usize,
    pub(super) child_order_rank: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RelationPolicy {
    pub(super) allowed: &'static [&'static str],
    pub(super) default: Option<&'static str>,
    pub(super) allow_multiple_types: bool,
    pub(super) projection: RelationProjectionPolicy,
}

pub(super) const SOLL_RELATION_ENDPOINT_KINDS: &[&str] = &[
    // REQ-AXO-91578/91579 — SKI + PRT added to canonical endpoint kinds.
    // REQ-AXO-901727 — TMG (TechnologyMigration) added (Option A).
    "VIS", "PIL", "REQ", "CPT", "DEC", "MIL", "VAL", "STK", "GUI", "SKI", "PRT", "TMG", "ART",
];

pub(super) fn relation_table_name(_relation_type: &str) -> Option<&'static str> {
    Some("soll.Edge")
}

pub(super) fn soll_entity_table_name(prefix: &str) -> Option<&'static str> {
    match prefix {
        // REQ-AXO-91578/91579 — SKI + PRT added to canonical entity prefix set.
        "VIS" | "PIL" | "REQ" | "CPT" | "DEC" | "MIL" | "VAL" | "STK" | "GUI" | "SKI" | "PRT"
        | "TMG" => Some("soll.Node"),
        _ => None,
    }
}

pub(super) fn relation_policy_for_pair(
    source_type: &str,
    target_type: &str,
) -> Option<RelationPolicy> {
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
        // REQ-AXO-115 — A Concept that formalizes a Pillar-level
        // operational protocol (e.g. CPT-AXO-019 → PIL-AXO-003) needs a
        // canonical edge so the dependency is queryable. BELONGS_TO
        // matches the REQ→PIL semantic ("this node is owned by the
        // pillar"); higher child_order_rank than REQ→PIL (30) puts
        // requirements before concepts in pillar children. Lower
        // parent_preference_rank than CPT→REQ (40) reflects that a
        // Concept's primary parent is its Pillar when one is declared.
        ("CPT", "PIL") => Some(RelationPolicy {
            allowed: &["BELONGS_TO"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 15,
                child_order_rank: 50,
            },
        }),
        // REQ-AXO-274 phase 2 — A project-internal Concept can specialize a
        // cross-project methodology Concept (e.g. CPT-AXO-019 INHERITS_FROM
        // CPT-PRO-004 — "SOLL Operational Protocol" generalized). REFINES is
        // accepted as an alias when the specialization adds material; default
        // INHERITS_FROM matches the GUI→GUI inheritance pattern used to
        // propagate cross-project guidelines (project_code='PRO' parent).
        // REQ-AXO-326 — same-type SUPERSEDES enforces the canonical
        // graph-as-index discipline: a deprecated CPT MUST carry an outgoing
        // SUPERSEDES edge to its replacement (not just metadata.superseded_by).
        // Default stays INHERITS_FROM (cross-project propagation) ; SUPERSEDES
        // is opt-in when explicitly migrating duplicates / dead concepts.
        ("CPT", "CPT") => Some(RelationPolicy {
            allowed: &["INHERITS_FROM", "REFINES", "SUPERSEDES"],
            default: Some("INHERITS_FROM"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 80,
                child_order_rank: 60,
            },
        }),
        // REQ-AXO-274 phase 2 — A project-internal Concept can inherit from a
        // cross-project Decision when the canonical body lives in DEC-PRO
        // (e.g. CPT-AXO-021 INHERITS_FROM DEC-PRO-001 — bootstrap prompt
        // canonical lives in the Decision under PRO).
        ("CPT", "DEC") => Some(RelationPolicy {
            allowed: &["INHERITS_FROM"],
            default: Some("INHERITS_FROM"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 80,
                child_order_rank: 65,
            },
        }),
        // REQ-AXO-274 phase 2 — Methodology Guidelines (GUI-PRO-*) belong to
        // a Pillar (PIL-PRO-*) for theming, queryability via
        // `soll_query_context project_code=PRO`, and `soll_work_plan` scoring.
        // Same semantic as REQ→PIL and CPT→PIL. Supporting role (lower than
        // Concept-level Primary) so a Pillar's primary children remain
        // requirements/concepts; guidelines appear after them in projection.
        ("GUI", "PIL") => Some(RelationPolicy {
            allowed: &["BELONGS_TO"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Supporting,
                parent_preference_rank: 50,
                child_order_rank: 100,
            },
        }),
        // REQ-AXO-901727 (Option A) — TechnologyMigration belongs to a Pillar
        // for theming/queryability (like CPT/GUI→PIL) and REFINES the Decision
        // that mandated the migration (e.g. the DuckDB→PG or AGE-retirement DEC).
        // The HAS_REMNANT TMG→IST cross-graph edge is a follow-up slice (N2).
        ("TMG", "PIL") => Some(RelationPolicy {
            allowed: &["BELONGS_TO"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Supporting,
                parent_preference_rank: 55,
                child_order_rank: 110,
            },
        }),
        ("TMG", "DEC") => Some(RelationPolicy {
            allowed: &["REFINES"],
            default: Some("REFINES"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 90,
                child_order_rank: 120,
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
        // REQ-AXO-326 — SUPERSEDES added to enable canonical replacement
        // of a project-scoped Guideline by the cross-project PRO equivalent
        // (e.g. GUI-AXO-1001 → GUI-PRO-100 after PRO registry fix). Default
        // stays INHERITS_FROM (cross-project propagation).
        ("GUI", "GUI") => Some(RelationPolicy {
            allowed: &["INHERITS_FROM", "SUPERSEDES"],
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
        // REQ-AXO-902016 — BLOCKED_BY to the EXTERNAL factor parking a REQ:
        // a pending Decision (REQ→DEC, e.g. an operator model-swap call), a
        // delivery gate (REQ→MIL), or an infra/key artifact (REQ→ART). These
        // are dependency edges (Supporting role, never a tree parent); the
        // work_plan classifies the source into blockers while the target is
        // non-terminal. The reverse canonical edges (DEC -SOLVES-> REQ,
        // MIL -TARGETS-> REQ) keep their own filiation semantics.
        ("REQ", "DEC") => Some(RelationPolicy {
            allowed: &["BLOCKED_BY"],
            default: Some("BLOCKED_BY"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Supporting,
                parent_preference_rank: 95,
                child_order_rank: 999,
            },
        }),
        ("REQ", "MIL") => Some(RelationPolicy {
            allowed: &["BLOCKED_BY"],
            default: Some("BLOCKED_BY"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Supporting,
                parent_preference_rank: 95,
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
        // REQ-AXO-91578 — SKI (Skill) entity canonical relations.
        // A Skill is a procedure invocable by an LLM via MCP. It anchors
        // organizationally on a Pillar (BELONGS_TO) and methodologically
        // on a Guideline that mandates its invocation (INHERITS_FROM,
        // the canonical "mandated by" semantic per CPT-AXO-90019 triad).
        // Skill composition is via same-type REFINES / COMPOSES_WITH /
        // SUPERSEDES / INHERITS_FROM edges.
        ("SKI", "PIL") => Some(RelationPolicy {
            allowed: &["BELONGS_TO"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 12,
                child_order_rank: 65,
            },
        }),
        ("SKI", "GUI") => Some(RelationPolicy {
            allowed: &["INHERITS_FROM"],
            default: Some("INHERITS_FROM"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 20,
                child_order_rank: 70,
            },
        }),
        ("SKI", "SKI") => Some(RelationPolicy {
            allowed: &["REFINES", "COMPOSES_WITH", "SUPERSEDES", "INHERITS_FROM"],
            default: Some("INHERITS_FROM"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 80,
                child_order_rank: 65,
            },
        }),
        // REQ-AXO-91579 — PRT (PromptTemplate) entity canonical relations.
        // A PromptTemplate is parametrized text rendered by SKI procedures
        // (or directly by an LLM via mcp__axon__prompt_template_get). It
        // BELONGS_TO a Pillar (organizational scope), INHERITS_FROM a
        // Guideline (methodology rule it implements), and is USED_BY skills
        // via the (SKI, PRT) USES edge. PRT composition is via same-type
        // EXTENDS / REFINES / SUPERSEDES edges.
        ("PRT", "PIL") => Some(RelationPolicy {
            allowed: &["BELONGS_TO"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 14,
                child_order_rank: 67,
            },
        }),
        ("PRT", "GUI") => Some(RelationPolicy {
            allowed: &["INHERITS_FROM"],
            default: Some("INHERITS_FROM"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Primary,
                parent_preference_rank: 22,
                child_order_rank: 72,
            },
        }),
        ("PRT", "PRT") => Some(RelationPolicy {
            allowed: &["EXTENDS", "REFINES", "SUPERSEDES", "INHERITS_FROM"],
            default: Some("INHERITS_FROM"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 82,
                child_order_rank: 67,
            },
        }),
        // REQ-AXO-91579 — SKI USES PRT : the canonical edge for a skill
        // that injects a prompt template's rendered output (sub-agent brief,
        // PRD body, error message, etc.).
        ("SKI", "PRT") => Some(RelationPolicy {
            allowed: &["USES"],
            default: Some("USES"),
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 90,
                child_order_rank: 80,
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
        // REQ-AXO-188 — A Decision can REFINE or SUPERSEDE the Concept it
        // governs. CPTs that document architecture-state (e.g. CPT-AXO-030..035)
        // need this canonical edge so:
        //   - soll_work_plan can weight CPTs by recent DECs that refined them,
        //   - soll_validate can detect architecture-state CPTs whose
        //     governing DEC committed >7 days ago without the CPT being
        //     refreshed (drift signal),
        //   - kickoff_bundle can surface "architecture_state CPTs touched by
        //     recent DECs" as a session-start orientation slice.
        // Default=None forces the caller to be explicit (REFINES vs SUPERSEDES
        // carries different semantics; refusing to default avoids miscoding).
        ("DEC", "CPT") => Some(RelationPolicy {
            allowed: &["REFINES", "SUPERSEDES"],
            default: None,
            allow_multiple_types: false,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 95,
                child_order_rank: 999,
            },
        }),
        // REQ-AXO-326 — SUPERSEDES added so a Requirement deduplicated/
        // replaced by a newer REQ carries the canonical edge (e.g.
        // REQ-AXO-207/208 → REQ-AXO-198 — P2 DDL generator duplicates).
        // Default stays REFINES (incremental specialization).
        // REQ-AXO-902016 — BLOCKED_BY models a dependency/blocking edge: this
        // REQ cannot proceed until the target (another REQ) is resolved.
        // soll_work_plan reads it to auto-classify the source out of the
        // actionable wave and into blockers while the target is non-terminal.
        // allow_multiple_types so a REQ can REFINE one REQ and be BLOCKED_BY
        // another in the same graph.
        ("REQ", "REQ") => Some(RelationPolicy {
            allowed: &["REFINES", "BELONGS_TO", "SUPERSEDES", "BLOCKED_BY"],
            default: Some("REFINES"),
            allow_multiple_types: true,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Lateral,
                parent_preference_rank: 95,
                child_order_rank: 999,
            },
        }),
        // REQ-AXO-326 — a Pillar that absorbs another (e.g. PIL-AXO-006
        // "Cohabitation polie live↔dev" fused into PIL-AXO-004) needs the
        // canonical SUPERSEDES edge so the graph reflects the absorption.
        ("PIL", "PIL") => Some(RelationPolicy {
            allowed: &["SUPERSEDES"],
            default: None,
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
        // REQ-AXO-902016 — a Requirement may be SUBSTANTIATES'd by an artifact
        // (evidence) OR BLOCKED_BY one (an external infra/key/dependency artifact
        // that parks the work). allow_multiple_types so both can coexist.
        ("REQ", "ART") => Some(RelationPolicy {
            allowed: &["SUBSTANTIATES", "BLOCKED_BY"],
            default: Some("SUBSTANTIATES"),
            allow_multiple_types: true,
            projection: RelationProjectionPolicy {
                role: ProjectionRole::Supporting,
                parent_preference_rank: 100,
                child_order_rank: 999,
            },
        }),
        ("VAL", "ART") => Some(RelationPolicy {
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

pub(super) fn kind_projection_policy(kind: &str) -> Option<KindProjectionPolicy> {
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

pub(super) fn projection_metadata_payload(
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

pub(super) fn relation_policy_payload(source_type: &str, target_type: &str) -> Value {
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

pub(super) fn relation_schema_summary_for_kind(kind: &str) -> Value {
    let allowed_targets = allowed_relation_targets_from_source(kind);
    let forbidden_targets = SOLL_RELATION_ENDPOINT_KINDS
        .iter()
        .filter(|target_kind| **target_kind != kind)
        .filter(|target_kind| relation_policy_for_pair(kind, target_kind).is_none())
        .map(|target_kind| {
            let reverse_exists = relation_policy_for_pair(target_kind, kind).is_some();
            let reason = if reverse_exists {
                "canonical direction exists in the reverse direction"
            } else if *target_kind == "MIL" || kind == "MIL" {
                "milestones are not part of this canonical edge family"
            } else {
                "no canonical relation policy exists for this pair"
            };
            json!({
                "target_kind": target_kind,
                "reason": reason
            })
        })
        .collect::<Vec<_>>();
    json!({
        "source_kind": kind,
        "allowed_targets": allowed_targets,
        "forbidden_targets": forbidden_targets,
        "incoming_from_source_kinds": incoming_relation_sources_for_target(kind),
        "graph_role": graph_role_for_kind(kind),
        "kind_projection": kind_projection_policy(kind).map(|policy| json!({
            "breadcrumb_eligible": policy.breadcrumb_eligible,
            "root_eligible": policy.root_eligible,
            "tree_order_rank": policy.tree_order_rank
        })),
        "guidance_source": "derived_from_relation_policy"
    })
}

pub(super) fn reverse_relation_hint_payload(source_kind: &str, target_kind: &str) -> Value {
    relation_policy_for_pair(target_kind, source_kind)
        .map(|reverse_policy| {
            let relation = reverse_policy.default.unwrap_or(reverse_policy.allowed[0]);
            json!({
                "source_kind": target_kind,
                "target_kind": source_kind,
                "relation_type": relation,
                "projection": projection_metadata_payload(target_kind, source_kind, reverse_policy),
                "example": relation_example_sentence(target_kind, source_kind, relation)
            })
        })
        .unwrap_or(Value::Null)
}

pub(super) fn graph_role_for_kind(kind: &str) -> &'static str {
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
        "TMG" => "technology migration tracking its incomplete-migration remnants",
        "ART" => "implementation or runtime artifact",
        _ => "soll graph node",
    }
}

pub(super) fn relation_example_sentence(
    source_kind: &str,
    target_kind: &str,
    relation_type: &str,
) -> String {
    format!(
        "A {} node typically uses `{}` to connect to a {} node.",
        source_kind, relation_type, target_kind
    )
}

pub(super) fn allowed_relation_targets_from_source(source_type: &str) -> Vec<Value> {
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

pub(super) fn incoming_relation_sources_for_target(target_type: &str) -> Vec<Value> {
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

/// REQ-AXO-902003 — render a source-only / target-only relation summary as a
/// terse, LLM-visible matrix (mirrors the commit-error guidance), so
/// `soll_relation_schema(source_type=X)` discloses the legal pairs in the
/// rendered TEXT instead of hiding them in `data`. An LLM optimises on the
/// rendered sentence and won't drill into the structured envelope, so the
/// tool's promise ("discover valid links without trial and error") is only
/// kept if the matrix is in the text. Returns `None` for a specific pair
/// (handled by the caller's `canonical_direction` branch) or an empty policy.
pub(super) fn render_kind_schema_matrix_text(data: &Value) -> Option<String> {
    let routes_from = |entries: &[Value], kind_field: &str| -> Vec<String> {
        entries
            .iter()
            .filter_map(|entry| {
                let kind = entry.get(kind_field).and_then(Value::as_str)?;
                let rels = entry
                    .get("allowed_relations")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .filter(|joined| !joined.is_empty())
                    .or_else(|| {
                        entry
                            .get("default_relation")
                            .and_then(Value::as_str)
                            .map(String::from)
                    })?;
                Some(format!("{kind} via {rels}"))
            })
            .collect()
    };

    // Source-only: outgoing matrix — "DEC can legally reach: REQ via SOLVES/REFINES, CPT via REFINES".
    if let (Some(source_kind), Some(targets)) = (
        data.get("source_kind").and_then(Value::as_str),
        data.get("allowed_targets").and_then(Value::as_array),
    ) {
        let routes = routes_from(targets, "target_kind");
        if !routes.is_empty() {
            return Some(format!(
                "{source_kind} can legally reach: {}.",
                routes.join(", ")
            ));
        }
    }

    // Target-only: incoming matrix — "VIS can be legally reached by: PIL via EPITOMIZES".
    if data.get("allowed_targets").is_none() {
        if let (Some(target_kind), Some(sources)) = (
            data.get("target_kind").and_then(Value::as_str),
            data.get("incoming_from_source_kinds").and_then(Value::as_array),
        ) {
            let routes = routes_from(sources, "source_kind");
            if !routes.is_empty() {
                return Some(format!(
                    "{target_kind} can be legally reached by: {}.",
                    routes.join(", ")
                ));
            }
        }
    }

    None
}

pub(super) fn repair_guidance_entry(
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

pub(super) fn relation_scope_matches(
    source_id: &str,
    target_id: &str,
    project_code: Option<&str>,
) -> bool {
    match project_code {
        Some(code) => {
            let marker = format!("-{}-", code);
            source_id.contains(&marker) || target_id.contains(&marker)
        }
        None => true,
    }
}

#[cfg(test)]
mod blocked_by_policy_tests {
    use super::relation_policy_for_pair;

    /// REQ-AXO-902016 — BLOCKED_BY is a canonical Requirement dependency edge to
    /// the realistic blocker kinds (another REQ, a pending DEC, a delivery MIL,
    /// an external ART). soll_work_plan reads it to auto-classify blockers.
    #[test]
    fn blocked_by_allowed_for_requirement_dependency_pairs() {
        for target in ["REQ", "DEC", "MIL", "ART"] {
            let policy = relation_policy_for_pair("REQ", target)
                .unwrap_or_else(|| panic!("REQ -> {target} must have a relation policy"));
            assert!(
                policy.allowed.contains(&"BLOCKED_BY"),
                "REQ -> {target} must allow BLOCKED_BY (allowed = {:?})",
                policy.allowed
            );
        }
    }

    /// BLOCKED_BY must NOT bleed onto unrelated pairs (e.g. VAL -> ART evidence).
    #[test]
    fn blocked_by_not_allowed_on_validation_artifact() {
        let policy = relation_policy_for_pair("VAL", "ART").expect("VAL -> ART policy exists");
        assert!(
            !policy.allowed.contains(&"BLOCKED_BY"),
            "VAL -> ART must stay SUBSTANTIATES-only"
        );
    }

    /// REQ -> ART keeps SUBSTANTIATES (evidence) alongside the new BLOCKED_BY.
    #[test]
    fn requirement_artifact_keeps_substantiates_and_adds_blocked_by() {
        let policy = relation_policy_for_pair("REQ", "ART").expect("REQ -> ART policy exists");
        assert!(policy.allowed.contains(&"SUBSTANTIATES"));
        assert!(policy.allowed.contains(&"BLOCKED_BY"));
        assert!(policy.allow_multiple_types);
    }
}
