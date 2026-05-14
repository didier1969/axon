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

    match node.priority.as_str() {
        "P0" => {
            score += 20;
            reasons.push("priority P0".to_string());
        }
        "P1" => {
            score += 15;
            reasons.push("priority P1".to_string());
        }
        "P2" => {
            score += 8;
            reasons.push("priority P2".to_string());
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

pub(super) fn build_waves(
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
                "ist_signals": item.ist_signals
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
    use super::is_terminal_status;

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
            assert!(
                is_terminal_status(status),
                "`{status}` must be terminal"
            );
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
}
