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
    /// REQ-AXO-902295 / DEC-AXO-901668 — SOLL hygiene debt, kept STRICTLY
    /// out of `score`. `score` answers "what do I start next?"; this one
    /// answers "where are the proof holes?". Fused into one integer, they
    /// contradict each other: an unproven REQ is urgent for hygiene and NOT
    /// ready for execution — and the fused number made "unproven and
    /// incomplete" worth 69% of a REQ's points, so attaching evidence
    /// LOWERED its rank. Published in `data`/`reasons`/`validation_gates`
    /// (nothing is hidden) but never part of the ordering key.
    pub(super) proof_gap_score: i64,
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
pub(crate) const CANONICAL_NODE_STATUSES: &[&str] = &[
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

/// REQ-AXO-902295 / DEC-AXO-901668 — bonus for work somebody has ALREADY
/// STARTED. Before this, `status` carried NO weight at all: moving a node to
/// `current` — the one unambiguous signal of engagement — changed its rank by
/// exactly zero.
///
/// The value is not arbitrary, it is bracketed by the `descendants * 40` term:
/// **strictly above one descendant (40)** so an engaged terminal leaf beats a
/// not-yet-started single unblocker (finish what is open before opening more),
/// and **below two descendants (80)** so a genuinely structural unblocker still
/// leads. Change either side of that bracket and the rule stops holding.
pub(super) const ENGAGEMENT_BONUS: i64 = 50;

/// REQ-AXO-902295 — `current` is the canonical "work in progress" status
/// (DEC-PRO-100). `planned` is intent, terminal states are done, and
/// `blocked`/`deferred` never reach the actionable wave at all.
pub(super) fn is_engaged_status(status: &str) -> bool {
    status.trim().to_ascii_lowercase() == "current"
}

/// Returns `(score, proof_gap_score, reasons, validation_gates)`.
///
/// REQ-AXO-902295 / DEC-AXO-901668 — the two numbers answer two DIFFERENT
/// questions and must never be summed again:
/// - `score` = execution urgency (descendants, priority, engagement,
///   centrality, temporal decay);
/// - `proof_gap_score` = SOLL hygiene debt (missing proof, missing criteria,
///   degraded IST scope, visible backlog).
///
/// Every reason and every gate is still emitted on the same vectors — the
/// split changes what ORDERS the plan, not what it discloses.
pub(super) fn score_node(
    node: &WorkPlanNode,
    include_ist: bool,
    include_decay: bool,
    half_life_days: f64,
    now_ms: i64,
) -> (i64, i64, Vec<String>, Vec<String>) {
    let mut score = (node.descendants as i64) * 40;
    let mut proof_gap_score = 0i64;
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

    // REQ-AXO-902295 / DEC-AXO-901668 — the four blocks below feed
    // `proof_gap_score`, NOT `score`. They used to be worth up to +38 of
    // execution urgency, which inverted the incentive: attaching evidence
    // cost 10 points and writing acceptance criteria cost 7. Measured on the
    // AXO board of 2026-08-15, they accounted for 18 of REQ-AXO-902295's own
    // 26 points (69%) — the plan was ranking REQs for being unfinished.
    if let Some(state) = node.requirement_state.as_deref() {
        match state {
            "missing" => {
                proof_gap_score += 15;
                reasons.push("requirement missing".to_string());
                validation_gates.push("define acceptance criteria and evidence".to_string());
            }
            "partial" => {
                proof_gap_score += 8;
                reasons.push("requirement partial".to_string());
                validation_gates.push("complete missing proof or acceptance criteria".to_string());
            }
            _ => {}
        }
    }

    if node.evidence_count == 0 {
        proof_gap_score += 10;
        reasons.push("no evidence attached".to_string());
        validation_gates.push("attach evidence".to_string());
    }

    if include_ist && node.ist_degraded_links > 0 {
        proof_gap_score += 8;
        reasons.push("IST scope degraded".to_string());
        validation_gates.push("reindex degraded scope".to_string());
    }

    if node.backlog_visible {
        proof_gap_score += 5;
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

    // REQ-AXO-902295 / DEC-AXO-901668 — engagement is added AFTER the decay
    // multiplier, deliberately. Decay models "nobody has touched this in a
    // long time"; `status=current` is the human assertion of the OPPOSITE.
    // Letting decay erode the very signal that refutes it would be
    // incoherent — and the data says it would also be ineffective:
    // REQ-AXO-902260 is P1 AND `current` yet scored 8, i.e. P1(+15) x 0.53.
    // A bonus applied before the multiplier would be halved on exactly the
    // node it exists to lift.
    //
    // Accepted risk, stated rather than hidden: a `current` abandoned for
    // months keeps its bonus. That is the intended reading — an abandoned
    // WIP is itself a defect worth SEEING, not one to bury under decay.
    if is_engaged_status(&node.status) {
        score += ENGAGEMENT_BONUS;
        reasons.push("work in progress (status=current)".to_string());
    }

    (score, proof_gap_score, reasons, validation_gates)
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
                // REQ-AXO-902295 — hygiene debt, published beside the
                // execution score instead of being summed into it.
                "proof_gap_score": item.proof_gap_score,
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
    // REQ-AXO-902295 — engagement now RANKS the node, so it must also be what
    // the "Immediate actions" line SAYS. Otherwise an engaged P1 would be
    // listed as `[proof_gap] : close proof gap (partial)` — a label describing
    // the hygiene axis that no longer decides the order, i.e. the operator
    // reads a reason that is not the reason.
    } else if is_engaged_status(&node.status) {
        "in_progress"
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
    } else if is_engaged_status(&node.status) {
        // REQ-AXO-902295 — finish what is already open before opening more.
        "work in progress — finish before starting new work".to_string()
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

    /// Build a bare Requirement node; callers tweak only the fields their
    /// assertion is about, so every test states its own inputs explicitly.
    fn node(status: &str, priority: &str) -> super::WorkPlanNode {
        super::WorkPlanNode {
            id: "REQ-AXO-TEST".to_string(),
            title: "t".to_string(),
            entity_type: super::WorkPlanEntityType::Requirement,
            status: status.to_string(),
            priority: priority.to_string(),
            requirement_state: None,
            evidence_count: 1,
            descendants: 0,
            ist_degraded_links: 0,
            backlog_visible: false,
            score: 0,
            proof_gap_score: 0,
            reasons: Vec::new(),
            validation_gates: Vec::new(),
            ist_signals: Vec::new(),
            updated_at_ms: None,
            centrality: None,
            breadcrumb: None,
        }
    }

    // Isolate the priority contribution: 0 descendants, no proof gap, evidence present,
    // no backlog, no decay, status `planned` (no engagement bonus) — so score_node's
    // only non-zero term is the priority bonus.
    fn scored(priority: &str) -> (i64, Vec<String>) {
        let (score, _proof_gap, reasons, _gates) =
            super::score_node(&node("planned", priority), false, false, 30.0, 0);
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

    // --- REQ-AXO-902295 / DEC-AXO-901668 — execution urgency vs proof gap ----------------

    fn exec_score(node: &super::WorkPlanNode, include_decay: bool, now_ms: i64) -> i64 {
        super::score_node(node, false, include_decay, 30.0, now_ms).0
    }

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    /// THE regression this REQ exists for, in its original SWT shape: a leaf that
    /// somebody has STARTED, is P0, carries 7 evidence items and has no descendants
    /// must outrank an untouched P0 that merely unblocks one child.
    ///
    /// Pre-fix this was impossible by construction: the engaged leaf scored 20
    /// (priority only) against 40 + 20 + 10 = 70 — and each of the 7 evidence items
    /// it had EARNED was worth -10 to it in aggregate.
    #[test]
    fn engaged_proven_leaf_outranks_untouched_single_unblocker() {
        let mut engaged = node("current", "P0");
        engaged.evidence_count = 7;
        engaged.descendants = 0;

        let mut untouched = node("planned", "P0");
        untouched.evidence_count = 0;
        untouched.descendants = 1;

        let engaged_score = exec_score(&engaged, false, 0);
        let untouched_score = exec_score(&untouched, false, 0);
        assert!(
            engaged_score > untouched_score,
            "engaged+proven leaf must outrank an untouched unblocker: {engaged_score} vs {untouched_score}"
        );
    }

    /// Proving your work must never cost you rank. Two otherwise identical REQs —
    /// one fully proven, one with neither criteria nor evidence — must score the SAME
    /// execution urgency; only `proof_gap_score` may differ.
    #[test]
    fn attaching_evidence_never_lowers_execution_score() {
        let mut proven = node("current", "P1");
        proven.evidence_count = 7;
        proven.requirement_state = Some("done".to_string());

        let mut unproven = node("current", "P1");
        unproven.evidence_count = 0;
        unproven.requirement_state = Some("missing".to_string());

        let (proven_score, proven_gap, ..) = super::score_node(&proven, false, false, 30.0, 0);
        let (unproven_score, unproven_gap, ..) = super::score_node(&unproven, false, false, 30.0, 0);

        assert_eq!(
            proven_score, unproven_score,
            "proof state must not move execution urgency at all"
        );
        assert_eq!(proven_gap, 0, "a fully proven REQ has no proof gap");
        assert_eq!(
            unproven_gap, 25,
            "missing criteria (15) + no evidence (10) land on the hygiene axis"
        );
    }

    /// REQ-AXO-902295 — the engagement bonus is added AFTER the decay multiplier.
    /// Discriminating case: a low-priority item engaged 60 days ago (decay 0.135)
    /// must still outrank a fresh, untouched P0. Fold the bonus into the decayed
    /// base instead and it collapses to ~7 against 20 — this test goes red.
    #[test]
    fn engagement_bonus_is_not_eroded_by_temporal_decay() {
        let now_ms = 100 * DAY_MS;
        let mut stale_engaged = node("current", "P3");
        stale_engaged.updated_at_ms = Some(now_ms - 60 * DAY_MS);

        let mut fresh_untouched = node("planned", "P0");
        fresh_untouched.updated_at_ms = Some(now_ms);

        let engaged_score = exec_score(&stale_engaged, true, now_ms);
        let untouched_score = exec_score(&fresh_untouched, true, now_ms);
        assert!(
            engaged_score > untouched_score,
            "engagement must survive decay: {engaged_score} vs {untouched_score}"
        );
    }

    /// The bracket that sizes ENGAGEMENT_BONUS is load-bearing (see its doc
    /// comment). Pin it so a later tweak of either term cannot silently break the
    /// "finish what is open" rule or drown genuine structural unblockers.
    #[test]
    fn engagement_bonus_sits_between_one_and_two_descendants() {
        let engaged = node("current", "");
        let mut one_child = node("planned", "");
        one_child.descendants = 1;
        let mut two_children = node("planned", "");
        two_children.descendants = 2;

        let engaged_score = exec_score(&engaged, false, 0);
        assert!(
            engaged_score > exec_score(&one_child, false, 0),
            "engagement must beat a single unblocker"
        );
        assert!(
            engaged_score < exec_score(&two_children, false, 0),
            "engagement must NOT outrank a two-descendant structural unblocker"
        );
    }

    /// Nothing is hidden by the split: every hygiene signal still emits its reason
    /// and its validation gate, exactly as before — only the ordering key changed.
    #[test]
    fn hygiene_signals_still_disclose_reasons_and_gates() {
        let mut gapped = node("planned", "P2");
        gapped.evidence_count = 0;
        gapped.requirement_state = Some("partial".to_string());
        gapped.backlog_visible = true;

        let (_, proof_gap, reasons, gates) = super::score_node(&gapped, false, false, 30.0, 0);
        assert_eq!(proof_gap, 8 + 10 + 5);
        for expected in ["requirement partial", "no evidence attached", "project backlog visible"] {
            assert!(
                reasons.iter().any(|r| r == expected),
                "reason `{expected}` must still be disclosed: {reasons:?}"
            );
        }
        assert_eq!(gates.len(), 3, "one gate per hygiene signal: {gates:?}");
    }
}
