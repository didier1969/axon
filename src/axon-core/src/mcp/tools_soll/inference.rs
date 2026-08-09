use super::*;

mod mutation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum WorkPlanEntityType {
    Decision,
    Requirement,
    Milestone,
}

impl WorkPlanEntityType {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Decision => "Decision",
            Self::Requirement => "Requirement",
            Self::Milestone => "Milestone",
        }
    }

    pub(super) fn sort_rank(&self) -> usize {
        match self {
            Self::Decision => 0,
            Self::Requirement => 1,
            Self::Milestone => 2,
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(super) struct WorkPlanNode {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) entity_type: WorkPlanEntityType,
    pub(super) status: String,
    pub(super) priority: String,
    pub(super) requirement_state: Option<String>,
    pub(super) evidence_count: usize,
    pub(super) descendants: usize,
    pub(super) ist_degraded_links: usize,
    pub(super) backlog_visible: bool,
    pub(super) score: i64,
    pub(super) reasons: Vec<String>,
    pub(super) validation_gates: Vec<String>,
    pub(super) ist_signals: Vec<String>,
    /// REQ-AXO-144 — last-update timestamp (ms since epoch) read from
    /// node metadata. `None` when the node has no `updated_at` field
    /// (older fixtures, hand-inserted rows). Drives temporal score decay.
    pub(super) updated_at_ms: Option<i64>,
    /// REQ-AXO-91501 — PageRank centrality score on the schedulable
    /// sub-graph (filtered to non-terminal nodes). `None` when the
    /// caller did not request centrality scoring (via the
    /// `include_centrality` arg, default false). When `Some`, the
    /// value is in [0.0, 1.0]; integrated into `score` as
    /// `+round(centrality * 100)` by `score_node`.
    pub(super) centrality: Option<f32>,
    /// REQ-AXO-902079 — strategic breadcrumb of the leaf's parents
    /// (`MIL ‹title› → DEC ‹title›`) so an actionable Requirement carries its
    /// WHY-chain inline. `None` for non-leaf nodes / when no strategic parent
    /// is on the schedulable graph. Populated by `build_actionable_leaves_wave`.
    pub(super) breadcrumb: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkPlanWave {
    pub(super) wave_index: usize,
    pub(super) items: Vec<WorkPlanNode>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkPlanCycle {
    pub(super) node_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkPlanBlocker {
    pub(super) id: String,
    pub(super) entity_type: String,
    pub(super) reason: String,
}

/// Returns true when a SOLL Node status represents a terminal lifecycle
/// state. Terminal nodes are excluded from `soll_work_plan` scheduling and
/// from descendant counting.
///
/// Recognized terminal states per DEC-PRO-100 canonical vocabulary
/// `[current, planned, delivered, superseded, rejected]` + legacy values
/// still present in older nodes :
/// - `delivered` / `superseded` (Decision)
/// - `completed` / `superseded` (Requirement, Milestone — legacy `completed`
///    retained for historical nodes; new ones use `delivered`)
/// - `archived` (any type)
/// - `rejected` (REQ-AXO-346) — explicit operator/LLM rejection. Pre-fix,
///    rejected nodes leaked into `soll_work_plan` Wave 1 with inflated
///    `unblocks N` scores pointing at their rejected descendants
///    (DEC-AXO-077 / 078 / 084 lit. observed session 32). Adding `rejected`
///    here closes Bug 1+2+3 of REQ-AXO-346.
pub(super) fn is_terminal_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "delivered" | "superseded" | "completed" | "archived" | "rejected"
    )
}

/// REQ-AXO-902016 — single source of truth for the canonical `node.status`
/// vocabulary (DEC-PRO-100). `blocked`/`deferred` join the original five so a
/// REQ parked on an external factor has an honest status. Server-side
/// validation (`soll_manager`) and the DB CHECK constraint
/// (`soll_node_status_canonical`, 01_soll_schema.sql) must agree with THIS list.
pub(super) const CANONICAL_NODE_STATUSES: &[&str] = &[
    "current",
    "planned",
    "delivered",
    "superseded",
    "rejected",
    "blocked",
    "deferred",
];

/// REQ-AXO-902016 — a node that is neither delivered nor rejected but parked
/// on an EXTERNAL factor (infra / keys / a pending human decision). Distinct
/// from terminal (it is NOT done) AND from active (it is NOT schedulable):
/// `soll_work_plan` must keep it OUT of the actionable wave (it was the
/// false-actionable noise that made gated REQs reappear) and instead surface it
/// in the blockers section. Coherence audits (`soll_validate`) already skip it
/// via the default `['current','planned']` status filter, so it is not flagged
/// for missing criteria/links while parked. `deferred` = self-blocked (we chose
/// to wait); `blocked` = blocked by a named factor (ideally a BLOCKED_BY edge).
pub(super) fn is_blocked_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "blocked" | "deferred"
    )
}

// REQ-AXO-346 Slice 3 — the hand-rolled adjacency map, Tarjan SCC,
// blocked-by-cycle BFS, filtered-adjacency view, and descendant counter
// previously living here are replaced by the petgraph-native helpers in
// `planning_work_plan.rs`. petgraph already powers `SollSnapshot`
// (REQ-AXO-322 / DEC-AXO-091) and `petgraph::algo::tarjan_scc` is
// O(V+E) by contract — no need to maintain a second implementation.
// `build_waves` likewise moved to a petgraph Kahn variant.

/// REQ-AXO-144 — half-life used when no override is supplied via args.
pub(super) const DEFAULT_DECAY_HALF_LIFE_DAYS: f64 = 30.0;

/// REQ-AXO-144 — temporal decay multiplier `exp(-age_days / half_life_days)`.
/// Returns 1.0 when decay is disabled, when the node has no `updated_at`
/// metadata, or when `half_life_days` is non-positive (guard against
/// misconfiguration).
pub(super) fn decay_factor_for_node(
    node: &WorkPlanNode,
    include_decay: bool,
    half_life_days: f64,
    now_ms: i64,
) -> f64 {
    if !include_decay {
        return 1.0;
    }
    if half_life_days <= 0.0 {
        return 1.0;
    }
    let Some(updated_ms) = node.updated_at_ms else {
        return 1.0;
    };
    let age_ms = (now_ms - updated_ms).max(0);
    let age_days = (age_ms as f64) / (1000.0 * 60.0 * 60.0 * 24.0);
    (-age_days / half_life_days).exp()
}

/// REQ-AXO-902282 (feedback #47, FSF) — the ONE canonical priority vocabulary for the work
/// plan. Maps canonical `P0..P3` AND legacy `critical/high/medium/low` to a level where
/// 0 = highest priority. `None` for unrecognised/empty values (fixture rows, unset priority).
/// Single source of truth the three former interpreters now share (score_node bonus,
/// actionable_priority_rank sort key, kickoff priority_rank) so P2/P3 stop collapsing into P1.
pub(super) fn priority_level(priority: &str) -> Option<u8> {
    match priority.trim().to_ascii_lowercase().as_str() {
        "p0" | "critical" => Some(0),
        "p1" | "high" => Some(1),
        "p2" | "medium" => Some(2),
        "p3" | "low" => Some(3),
        _ => None,
    }
}

pub(super) fn score_node(
    node: &WorkPlanNode,
    include_ist: bool,
    include_decay: bool,
    half_life_days: f64,
    now_ms: i64,
) -> (i64, Vec<String>, Vec<String>) {
    let mut score = (node.descendants as i64) * 40;
    let mut reasons = vec![format!("unblocks {} descendant(s)", node.descendants)];
    let mut validation_gates = Vec::new();

    // REQ-AXO-902282 (feedback #47) — score EVERY priority level through the shared
    // `priority_level` vocabulary, not just P0/P1/P2. The old `_ => {}` gave P3 (and any
    // legacy value) +0 and NO reason, so a P3 backlog was invisible and unranked. P0/P1/P2
    // bonuses are preserved exactly; P3 now earns a monotone +4 and its own reason.
    if let Some(level) = priority_level(&node.priority) {
        let bonus = match level {
            0 => 20,
            1 => 15,
            2 => 8,
            _ => 4,
        };
        score += bonus;
        reasons.push(format!("priority P{level}"));
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
        reasons.push("no evidence attached".to_string());
        validation_gates.push("attach evidence".to_string());
    }

    if include_ist && node.ist_degraded_links > 0 {
        score += 8;
        reasons.push("IST scope degraded".to_string());
        validation_gates.push("reindex degraded scope".to_string());
    }

    if node.backlog_visible {
        score += 5;
        reasons.push("project backlog visible".to_string());
        validation_gates.push("reduce project backlog before closure".to_string());
    }

    if matches!(node.entity_type, WorkPlanEntityType::Milestone) && node.descendants == 0 {
        score -= 10;
        reasons.push("isolated milestone".to_string());
    }

    // REQ-AXO-91501 — PageRank centrality on the schedulable sub-graph.
    // Surfaces hub nodes whose absolute descendant count is modest but
    // whose graph position concentrates many indirect dependencies
    // (Wave 1 hubs that the naïve scorer under-ranks). Range [0.0, 1.0],
    // multiplied by 100 to align with the other integer score buckets.
    // Opt-in via `include_centrality=true` arg.
    if let Some(centrality) = node.centrality {
        let bonus = (centrality * 100.0).round() as i64;
        if bonus > 0 {
            score += bonus;
            reasons.push(format!(
                "centrality bonus +{bonus} (PageRank {centrality:.3})"
            ));
        }
    }

    // REQ-AXO-144 — apply temporal decay so accepted Decisions and other
    // mature nodes without recent activity fall naturally out of wave 1
    // even when their structural score (descendants, evidence gaps, …)
    // would still rank them on top. Only nodes carrying an `updated_at`
    // timestamp are affected (back-compat: hand-inserted fixtures stay
    // unchanged). The reasons[] line surfaces the decay only when it is
    // material (factor < 0.5, i.e. the node is older than ~1 half-life)
    // so noise stays low for fresh nodes.
    let decay = decay_factor_for_node(node, include_decay, half_life_days, now_ms);
    if (decay - 1.0).abs() > f64::EPSILON {
        score = (score as f64 * decay).round() as i64;
        if decay < 0.5 {
            reasons.push(format!("decayed by age (factor {:.2})", decay));
        }
    }

    (score, reasons, validation_gates)
}

pub(super) fn apply_wave_limit(
    waves: &[WorkPlanWave],
    limit: usize,
) -> (Vec<WorkPlanWave>, usize, bool) {
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

pub(super) fn blocker_to_json(blocker: &WorkPlanBlocker) -> Value {
    json!({
        "id": blocker.id,
        "entity_type": blocker.entity_type,
        "reason": blocker.reason
    })
}

pub(super) fn cycle_to_json(cycle: &WorkPlanCycle) -> Value {
    json!({ "node_ids": cycle.node_ids })
}

pub(super) fn wave_to_json(wave: &WorkPlanWave) -> Value {
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
                "ist_signals": item.ist_signals,
                "breadcrumb": item.breadcrumb
            })
        }).collect::<Vec<_>>()
    })
}

pub(super) fn recommendation_kind(node: &WorkPlanNode) -> &'static str {
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

pub(super) fn recommendation_reason(node: &WorkPlanNode) -> String {
    if node.descendants > 0 {
        format!("unblocks {} descendant(s)", node.descendants)
    } else if node
        .requirement_state
        .as_deref()
        .is_some_and(|state| matches!(state, "missing" | "partial"))
    {
        format!(
            "close proof gap ({})",
            node.requirement_state.as_deref().unwrap_or("unknown")
        )
    } else if matches!(node.entity_type, WorkPlanEntityType::Milestone) {
        "milestone to scope or attach".to_string()
    } else {
        node.reasons
            .first()
            .cloned()
            .unwrap_or_else(|| "immediate action".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{is_blocked_status, is_terminal_status};

    /// REQ-AXO-346 Slice 1 — lock the terminal-status contract.
    /// `rejected` must be terminal (DEC-PRO-100 canonical vocabulary)
    /// so `soll_work_plan` excludes rejected DECs from Wave 1.
    #[test]
    fn rejected_status_is_terminal() {
        assert!(is_terminal_status("rejected"));
        assert!(is_terminal_status("REJECTED"));
        assert!(is_terminal_status("  rejected  "));
    }

    #[test]
    fn delivered_superseded_completed_archived_are_terminal() {
        for status in ["delivered", "superseded", "completed", "archived"] {
            assert!(is_terminal_status(status), "`{status}` must be terminal");
        }
    }

    #[test]
    fn active_statuses_are_not_terminal() {
        for status in ["current", "planned", "in_progress", "draft", "proposed", ""] {
            assert!(
                !is_terminal_status(status),
                "`{status}` must NOT be terminal"
            );
        }
    }

    /// REQ-AXO-902016 — `blocked`/`deferred` are a third category: parked on an
    /// external factor, neither terminal (done) nor schedulable.
    #[test]
    fn blocked_and_deferred_are_blocked_not_terminal() {
        for status in ["blocked", "deferred", "BLOCKED", "  deferred  "] {
            assert!(is_blocked_status(status), "`{status}` must be blocked");
            assert!(
                !is_terminal_status(status),
                "`{status}` must NOT be terminal (it is not done)"
            );
        }
    }

    #[test]
    fn active_and_terminal_statuses_are_not_blocked() {
        for status in [
            "current",
            "planned",
            "delivered",
            "rejected",
            "superseded",
            "",
        ] {
            assert!(!is_blocked_status(status), "`{status}` must NOT be blocked");
        }
    }

    // --- REQ-AXO-902282 canonical priority vocabulary + score_node (feedback #47) ---------

    #[test]
    fn priority_level_maps_canonical_and_legacy() {
        use super::priority_level;
        assert_eq!(priority_level("P0"), Some(0));
        assert_eq!(priority_level("p3"), Some(3));
        assert_eq!(priority_level("critical"), Some(0));
        assert_eq!(priority_level("HIGH"), Some(1));
        assert_eq!(priority_level("  medium  "), Some(2));
        assert_eq!(priority_level("low"), Some(3));
        assert_eq!(priority_level(""), None, "empty is unranked");
        assert_eq!(priority_level("bogus"), None);
        // Canonical and legacy names are the same level.
        assert_eq!(priority_level("P1"), priority_level("high"));
    }

    // Isolate the priority contribution: 0 descendants, no proof gap, evidence present,
    // no backlog, no decay — so score_node's only non-zero term is the priority bonus.
    fn scored(priority: &str) -> (i64, Vec<String>) {
        let node = super::WorkPlanNode {
            id: "REQ-AXO-TEST".to_string(),
            title: "t".to_string(),
            entity_type: super::WorkPlanEntityType::Requirement,
            status: "planned".to_string(),
            priority: priority.to_string(),
            requirement_state: None,
            evidence_count: 1,
            descendants: 0,
            ist_degraded_links: 0,
            backlog_visible: false,
            score: 0,
            reasons: Vec::new(),
            validation_gates: Vec::new(),
            ist_signals: Vec::new(),
            updated_at_ms: None,
            centrality: None,
            breadcrumb: None,
        };
        let (score, reasons, _gates) = super::score_node(&node, false, false, 30.0, 0);
        (score, reasons)
    }

    #[test]
    fn score_node_honours_p3_and_ranks_priorities_monotonically() {
        let p0 = scored("P0").0;
        let p1 = scored("P1").0;
        let p2 = scored("P2").0;
        let (p3_score, p3_reasons) = scored("P3");
        let unset = scored("").0;
        // Every level strictly outranks the next — P3 is no longer flattened to +0.
        assert!(
            p0 > p1 && p1 > p2 && p2 > p3_score && p3_score > unset,
            "monotone P0>P1>P2>P3>unset: {p0},{p1},{p2},{p3_score},{unset}"
        );
        // The #47 regression: a P3 node must earn a bonus AND surface a visible reason.
        assert!(p3_score > 0, "P3 must earn a positive bonus, got {p3_score}");
        assert!(
            p3_reasons.iter().any(|r| r == "priority P3"),
            "P3 must surface its priority reason: {p3_reasons:?}"
        );
        // P0/P1/P2 canonical bonuses are preserved exactly (regression guard on the values).
        assert_eq!(p0, 20);
        assert_eq!(p1, 15);
        assert_eq!(p2, 8);
        // Legacy vocabulary is scored like its canonical twin.
        assert_eq!(scored("high").0, p1, "legacy 'high' scores as P1");
    }
}
