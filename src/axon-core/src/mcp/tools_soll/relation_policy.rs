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
    "VIS", "PIL", "REQ", "CPT", "DEC", "MIL", "VAL", "STK", "GUI", "ART",
];

pub(super) fn relation_table_name(_relation_type: &str) -> Option<&'static str> {
    Some("soll.Edge")
}

pub(super) fn soll_entity_table_name(prefix: &str) -> Option<&'static str> {
    match prefix {
        "VIS" | "PIL" | "REQ" | "CPT" | "DEC" | "MIL" | "VAL" | "STK" | "GUI" => Some("soll.Node"),
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
