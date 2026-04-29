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

pub(super) fn build_adjacency_map(edges: &[(String, String)]) -> HashMap<String, BTreeSet<String>> {
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

pub(super) fn detect_cycle_sets<'a, I>(
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

pub(super) fn collect_blocked_by_cycles(
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

pub(super) fn filter_adjacency(
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

pub(super) fn compute_descendant_counts(
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

pub(super) fn score_node(
    node: &WorkPlanNode,
    include_ist: bool,
) -> (i64, Vec<String>, Vec<String>) {
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
