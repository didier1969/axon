use anyhow::anyhow;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use super::format::format_standard_contract;
use super::soll::{
    canonical_soll_export_dir, find_latest_soll_export, parse_soll_export, SollRestoreCounts,
};
use super::McpServer;

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
                items.iter()
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
                .then_with(|| left.entity_type.sort_rank().cmp(&right.entity_type.sort_rank()))
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
    pub(crate) fn axon_soll_manager(&self, args: &Value) -> Option<Value> {
        let action = args.get("action")?.as_str()?;
        let entity = args.get("entity")?.as_str()?;
        let data = args.get("data")?;

        match action {
            "create" => {
                let project_slug = data
                    .get("project_slug")
                    .and_then(|v| v.as_str())
                    .unwrap_or("AXO");
                let formatted_id = match entity {
                    "stakeholder" => data.get("name")?.as_str()?.to_string(),
                    "vision" => data
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("VIS-AXO-001")
                        .to_string(),
                    _ => match self.next_soll_numeric_id(project_slug, entity) {
                        Ok((prefix, next_num)) => {
                            format!("{}-{}-{:03}", prefix, project_slug, next_num)
                        }
                        Err(e) => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Erreur registre: {}", e) }], "isError": true }),
                            )
                        }
                    },
                };

                let insert_res = match entity {
                    "vision" => {
                        let title = data.get("title")?.as_str()?;
                        let description = data.get("description")?.as_str()?;
                        let goal = data.get("goal")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Vision (id, title, description, goal, metadata) VALUES (?, ?, ?, ?, ?) \
                                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, goal = EXCLUDED.goal, metadata = EXCLUDED.metadata";
                        self.graph_store.execute_param(
                            q,
                            &json!([formatted_id, title, description, goal, meta.to_string()]),
                        )
                    }
                    "pillar" => {
                        let title = data.get("title")?.as_str()?;
                        let desc = data.get("description")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Pillar (id, title, description, metadata) VALUES (?, ?, ?, ?)";
                        self.graph_store
                            .execute_param(q, &json!([formatted_id, title, desc, meta.to_string()]))
                    }
                    "requirement" => {
                        let title = data.get("title")?.as_str()?;
                        let desc = data.get("description")?.as_str()?;
                        let prio = data
                            .get("priority")
                            .and_then(|v| v.as_str())
                            .unwrap_or("P2");
                        let status = data
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("current");
                        let owner = data
                            .get("owner")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let acceptance_criteria = data
                            .get("acceptance_criteria")
                            .cloned()
                            .unwrap_or(json!([]))
                            .to_string();
                        let evidence_refs = data
                            .get("evidence_refs")
                            .cloned()
                            .unwrap_or(json!([]))
                            .to_string();
                        let updated_at = now_unix_ms();
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Requirement (id, title, description, status, priority, metadata, owner, acceptance_criteria, evidence_refs, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                formatted_id,
                                title,
                                desc,
                                status,
                                prio,
                                meta.to_string(),
                                owner,
                                acceptance_criteria,
                                evidence_refs,
                                updated_at
                            ]),
                        )
                    }
                    "concept" => {
                        let name = data.get("name")?.as_str()?;
                        let expl = data.get("explanation")?.as_str()?;
                        let rat = data.get("rationale")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let final_name = if name.starts_with(&formatted_id) {
                            name.to_string()
                        } else {
                            format!("{}: {}", formatted_id, name)
                        };
                        let q = "INSERT INTO soll.Concept (name, explanation, rationale, metadata) VALUES (?, ?, ?, ?)";
                        self.graph_store
                            .execute_param(q, &json!([final_name, expl, rat, meta.to_string()]))
                    }
                    "decision" => {
                        let title = data.get("title")?.as_str()?;
                        let description = data
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let ctx = data.get("context")?.as_str()?;
                        let rat = data.get("rationale")?.as_str()?;
                        let status = data
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("accepted");
                        let supersedes_decision_id = data
                            .get("supersedes_decision_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let impact_scope = data
                            .get("impact_scope")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let updated_at = now_unix_ms();
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Decision (id, title, description, context, rationale, status, metadata, supersedes_decision_id, impact_scope, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                formatted_id,
                                title,
                                description,
                                ctx,
                                rat,
                                status,
                                meta.to_string(),
                                supersedes_decision_id,
                                impact_scope,
                                updated_at
                            ]),
                        )
                    }
                    "milestone" => {
                        let title = data.get("title")?.as_str()?;
                        let status = data
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("planned");
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Milestone (id, title, status, metadata) VALUES (?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([formatted_id, title, status, meta.to_string()]),
                        )
                    }
                    "stakeholder" => {
                        let name = data.get("name")?.as_str()?;
                        let role = data.get("role")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q =
                            "INSERT INTO soll.Stakeholder (name, role, metadata) VALUES (?, ?, ?)";
                        self.graph_store
                            .execute_param(q, &json!([name, role, meta.to_string()]))
                    }
                    "validation" => {
                        let method = data.get("method")?.as_str()?;
                        let result = data.get("result")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;
                        let q = "INSERT INTO soll.Validation (id, method, result, timestamp, metadata) VALUES (?, ?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([formatted_id, method, result, ts, meta.to_string()]),
                        )
                    }
                    _ => Err(anyhow!("Unknown entity")),
                };

                match insert_res {
                    Ok(_) => {
                        let report = format!("✅ Entité SOLL créée : `{}`", formatted_id);
                        Some(json!({ "content": [{ "type": "text", "text": report }] }))
                    }
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur d'insertion: {}", e) }], "isError": true }),
                    ),
                }
            }
            "update" => {
                let id = data.get("id")?.as_str()?;
                let update_res: anyhow::Result<()> = (|| match entity {
                    "pillar" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, description, metadata FROM soll.Pillar WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                3,
                            )?;
                        let q =
                            "UPDATE soll.Pillar SET title = ?, description = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[2].clone()),
                                id
                            ]),
                        )
                    }
                    "requirement" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, description, priority, status, metadata, owner, acceptance_criteria, evidence_refs FROM soll.Requirement WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                8,
                            )?;
                        let q =
                            "UPDATE soll.Requirement SET title = ?, description = ?, priority = ?, status = ?, metadata = ?, owner = ?, acceptance_criteria = ?, evidence_refs = ?, updated_at = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("priority")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[2]),
                                data.get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[3]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[4].clone()),
                                data.get("owner")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[5]),
                                data.get("acceptance_criteria")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[6].clone()),
                                data.get("evidence_refs")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[7].clone()),
                                now_unix_ms(),
                                id
                            ]),
                        )
                    }
                    "concept" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT name, explanation, rationale, metadata FROM soll.Concept WHERE name LIKE '{}:%'",
                                    escape_sql(id)
                                ),
                                4,
                            )?;
                        let concept_name = data
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|name| {
                                if name.starts_with(id) {
                                    name.to_string()
                                } else {
                                    format!("{}: {}", id, name)
                                }
                            })
                            .unwrap_or_else(|| current[0].clone());
                        let q = "UPDATE soll.Concept SET name = ?, explanation = ?, rationale = ?, metadata = ? WHERE name LIKE ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                concept_name,
                                data.get("explanation")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("rationale")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[2]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[3].clone()),
                                format!("{}:%", id)
                            ]),
                        )
                    }
                    "decision" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, description, context, rationale, status, metadata, supersedes_decision_id, impact_scope FROM soll.Decision WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                8,
                            )?;
                        let q = "UPDATE soll.Decision SET title = ?, description = ?, context = ?, rationale = ?, status = ?, metadata = ?, supersedes_decision_id = ?, impact_scope = ?, updated_at = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("context")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[2]),
                                data.get("rationale")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[3]),
                                data.get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[4]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[5].clone()),
                                data.get("supersedes_decision_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[6]),
                                data.get("impact_scope")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[7]),
                                now_unix_ms(),
                                id
                            ]),
                        )
                    }
                    "milestone" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, status, metadata FROM soll.Milestone WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                3,
                            )?;
                        let q = "UPDATE soll.Milestone SET title = ?, status = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[2].clone()),
                                id
                            ]),
                        )
                    }
                    "stakeholder" => {
                        let current = self.query_named_row(
                            &format!(
                                "SELECT role, metadata FROM soll.Stakeholder WHERE name = '{}'",
                                escape_sql(id)
                            ),
                            2,
                        )?;
                        let q = "UPDATE soll.Stakeholder SET role = ?, metadata = ? WHERE name = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("role")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[1].clone()),
                                id
                            ]),
                        )
                    }
                    "validation" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT method, result, metadata FROM soll.Validation WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                3,
                            )?;
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;
                        let q = "UPDATE soll.Validation SET method = ?, result = ?, timestamp = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("method")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("result")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                ts,
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[2].clone()),
                                id
                            ]),
                        )
                    }
                    "vision" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, description, goal, metadata FROM soll.Vision WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                4,
                            )?;
                        let q = "UPDATE soll.Vision SET title = ?, description = ?, goal = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("goal")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[2]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[3].clone()),
                                id
                            ]),
                        )
                    }
                    _ => Err(anyhow!("Unknown entity")),
                })();
                match update_res {
                    Ok(_) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("✅ Mise à jour réussie pour `{}`", id) }] }),
                    ),
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur update: {}", e) }], "isError": true }),
                    ),
                }
            }
            "link" => {
                let src = data.get("source_id")?.as_str()?;
                let tgt = data.get("target_id")?.as_str()?;
                let explicit_rel = data.get("relation_type").and_then(|v| v.as_str());

                let rel_table = if let Some(r) = explicit_rel {
                    match r.to_uppercase().as_str() {
                        "EPITOMIZES" => "soll.EPITOMIZES",
                        "BELONGS_TO" => "soll.BELONGS_TO",
                        "EXPLAINS" => "soll.EXPLAINS",
                        "SOLVES" => "soll.SOLVES",
                        "TARGETS" => "soll.TARGETS",
                        "VERIFIES" => "soll.VERIFIES",
                        "ORIGINATES" => "soll.ORIGINATES",
                        "SUPERSEDES" => "soll.SUPERSEDES",
                        "CONTRIBUTES_TO" => "soll.CONTRIBUTES_TO",
                        "REFINES" => "soll.REFINES",
                        "IMPACTS" => "IMPACTS",
                        "SUBSTANTIATES" => "SUBSTANTIATES",
                        _ => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Erreur: Type de relation inconnu '{}'", r) }], "isError": true }),
                            )
                        }
                    }
                } else {
                    match (
                        src.split('-').next().unwrap_or(""),
                        tgt.split('-').next().unwrap_or(""),
                    ) {
                        ("PIL", "REQ") | ("REQ", "PIL") => "soll.BELONGS_TO",
                        ("CPT", "REQ") | ("REQ", "CPT") => "soll.EXPLAINS",
                        ("PIL", "AXO") | ("AXO", "PIL") => "soll.EPITOMIZES",
                        ("DEC", "REQ") | ("REQ", "DEC") => "soll.SOLVES",
                        ("MIL", "REQ") | ("REQ", "MIL") => "soll.TARGETS",
                        ("VAL", "REQ") | ("REQ", "VAL") => "soll.VERIFIES",
                        ("STK", "REQ") | ("REQ", "STK") => "soll.ORIGINATES",
                        ("DEC", _) => "IMPACTS",
                        _ => "SUBSTANTIATES",
                    }
                };

                let q = format!(
                    "INSERT INTO {} (source_id, target_id) VALUES (?, ?)",
                    rel_table
                );
                match self.graph_store.execute_param(&q, &json!([src, tgt])) {
                    Ok(_) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("✅ Liaison établie : `{}` -> `{}` (via {})", src, tgt, rel_table) }] }),
                    ),
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur liaison: {}", e) }], "isError": true }),
                    ),
                }
            }
            _ => None,
        }
    }

    pub(crate) fn axon_export_soll(&self) -> Option<Value> {
        let mut markdown = String::from("# SOLL Extraction\n\n");

        let now = std::time::SystemTime::now();
        let datetime: chrono::DateTime<chrono::Local> = now.into();
        let timestamp_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        markdown.push_str(&format!("*Généré le : {}*\n\n", timestamp_str));

        markdown.push_str("## 1. Vision & Objectifs Stratégiques\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT title, description, goal, metadata FROM soll.Vision")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                let meta = r.get(3).cloned().unwrap_or_default();
                markdown.push_str(&format!(
                    "### {}\n**Description:** {}\n**Goal:** {}\n**Meta:** `{}`\n\n",
                    r[0], r[1], r[2], meta
                ));
            }
        }

        markdown.push_str("## 2. Piliers d'Architecture\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT id, title, description, metadata FROM soll.Pillar")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("* **{}** : {} ({})\n", r[0], r[1], r[2]));
                if let Some(meta) = r.get(3).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("  Meta: `{}`\n", meta));
                }
            }
        }

        markdown.push_str("\n## 2b. Concepts\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT name, explanation, rationale, metadata FROM soll.Concept")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("* **{}** : {} ({})\n", r[0], r[1], r[2]));
                if let Some(meta) = r.get(3).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("  Meta: `{}`\n", meta));
                }
            }
        }

        markdown.push_str("\n## 3. Jalons & Roadmap (Milestones)\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT id, title, status, metadata FROM soll.Milestone")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!(
                    "### {} : {}\n*Statut :* `{}`\n\n",
                    r[0], r[1], r[2]
                ));
                if let Some(meta) = r.get(3).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("*Meta :* `{}`\n", meta));
                }
                markdown.push('\n');
            }
        }

        markdown.push_str("## 4. Exigences & Rayon d'Impact (Requirements)\n");
        let req_query =
            "SELECT id, title, priority, description, status, metadata FROM soll.Requirement";
        if let Ok(res) = self.graph_store.query_json(req_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!(
                    "### {} - {}\n*Priorité :* `{}`\n*Description :* {}\n",
                    r[0], r[1], r[2], r[3]
                ));
                if let Some(status) = r.get(4).filter(|m| !m.is_empty()) {
                    markdown.push_str(&format!("*Statut :* `{}`\n", status));
                }
                if let Some(meta) = r.get(5).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("*Meta :* `{}`\n", meta));
                }
                markdown.push('\n');
            }
        }

        markdown.push_str("## 5. Registre des Décisions (ADR)\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, status, context, description, rationale, metadata FROM soll.Decision") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {}\n**Titre :** {}\n**Statut :** `{}`\n", r[0], r[1], r[2]));
                if let Some(context) = r.get(3).filter(|m| !m.is_empty()) {
                    markdown.push_str(&format!("**Contexte :** {}\n", context));
                }
                if let Some(description) = r.get(4).filter(|m| !m.is_empty()) {
                    markdown.push_str(&format!("**Description :** {}\n", description));
                }
                markdown.push_str(&format!("**Rationnel :** {}\n", r[5]));
                if let Some(meta) = r.get(6).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("**Meta :** `{}`\n", meta));
                }
                markdown.push('\n');
            }
        }

        markdown.push_str("## 6. Preuves de Validation & Witness\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT id, method, result, timestamp, metadata FROM soll.Validation")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!(
                    "*   `{}` : **{}** via `{}` (Certifié le {})\n",
                    r[0], r[2], r[1], r[3]
                ));
                if let Some(meta) = r.get(4).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("  Meta: `{}`\n", meta));
                }
            }
        }

        markdown.push_str("\n## 7. Liens de Traçabilité SOLL\n");
        for (relation_type, table_name) in SOLL_RELATION_EXPORTS {
            if let Ok(res) = self.graph_store.query_json(&format!(
                "SELECT source_id, target_id FROM {} ORDER BY source_id, target_id",
                table_name
            )) {
                let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
                for row in rows {
                    if row.len() >= 2 {
                        markdown.push_str(&format!(
                            "* `{}`: `{}` -> `{}`\n",
                            relation_type, row[0], row[1]
                        ));
                    }
                }
            }
        }

        let export_dir = match canonical_soll_export_dir() {
            Some(path) => path,
            None => {
                return Some(json!({
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
                    "✅ Exported to {}\n\n---\n\n{}",
                    file_path.display(),
                    markdown.chars().take(300).collect::<String>()
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Erreur d'écriture: {}", e) }], "isError": true }),
            ),
        }
    }

    pub(crate) fn axon_validate_soll(&self) -> Option<Value> {
        let orphan_requirements = self
            .query_single_column(
                "SELECT id FROM soll.Requirement r
                 WHERE NOT EXISTS (SELECT 1 FROM soll.BELONGS_TO WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.EXPLAINS WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.SOLVES WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.TARGETS WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.VERIFIES WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.ORIGINATES WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM SUBSTANTIATES WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM IMPACTS WHERE source_id = r.id OR target_id = r.id)
                 ORDER BY id",
            )
            .ok()?;

        let validations_without_verifies = self
            .query_single_column(
                "SELECT id FROM soll.Validation v
                 WHERE NOT EXISTS (SELECT 1 FROM soll.VERIFIES WHERE source_id = v.id OR target_id = v.id)
                 ORDER BY id",
            )
            .ok()?;

        let decisions_without_links = self
            .query_single_column(
                "SELECT id FROM soll.Decision d
                 WHERE NOT EXISTS (SELECT 1 FROM soll.SOLVES WHERE source_id = d.id OR target_id = d.id)
                   AND NOT EXISTS (SELECT 1 FROM IMPACTS WHERE source_id = d.id OR target_id = d.id)
                 ORDER BY id",
            )
            .ok()?;

        let violation_count = orphan_requirements.len()
            + validations_without_verifies.len()
            + decisions_without_links.len();

        let mut evidence = format!(
            "Validation SOLL: {} violation(s) de cohérence minimale détectée(s).\n",
            violation_count
        );
        evidence.push_str("Mode: lecture seule, sans auto-réparation.\n");

        if !orphan_requirements.is_empty() {
            evidence.push_str("\n- Requirements orphelins:\n");
            for id in orphan_requirements {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }

        if !validations_without_verifies.is_empty() {
            evidence.push_str("\n- Validations sans lien VERIFIES:\n");
            for id in validations_without_verifies {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }

        if !decisions_without_links.is_empty() {
            evidence.push_str("\n- Decisions sans lien SOLVES/IMPACTS:\n");
            for id in decisions_without_links {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }

        let status = if violation_count == 0 {
            "ok"
        } else {
            "warn_soll_invariants"
        };
        let confidence = if violation_count == 0 { "high" } else { "medium" };
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
                "workspace:*",
                &evidence,
                &["run `soll_verify_requirements` for requirement-level coverage", "apply targeted SOLL links with `soll_manager` if needed"],
                confidence,
            )
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
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
            "INSERT INTO soll.Registry (project_slug, id, last_pil, last_req, last_cpt, last_dec, last_mil, last_val)
             VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_slug) DO NOTHING"
        ) {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("SOLL restore registry error: {}", e) }],
                "isError": true
            }));
        }

        let mut restored = SollRestoreCounts::default();

        for vision in restore.vision {
            let metadata = vision.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Vision (id, title, description, goal, metadata)
                 VALUES ('VIS-AXO-001', $title, $description, $goal, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   goal = EXCLUDED.goal,
                   metadata = EXCLUDED.metadata",
                &json!({
                    "title": vision.title,
                    "description": vision.description,
                    "goal": vision.goal,
                    "metadata": metadata
                }),
            ) {
                return Some(
                    json!({ "content": [{ "type": "text", "text": format!("SOLL restore vision error: {}", e) }], "isError": true }),
                );
            }
            restored.vision += 1;
        }

        for pillar in restore.pillars {
            let metadata = pillar.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Pillar (id, title, description, metadata)
                 VALUES ($id, $title, $description, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   metadata = EXCLUDED.metadata",
                &json!({"id": pillar.id, "title": pillar.title, "description": pillar.description, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore pillar error: {}", e) }], "isError": true }));
            }
            restored.pillars += 1;
        }

        for concept in restore.concepts {
            let metadata = concept.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Concept (name, explanation, rationale, metadata)
                 VALUES ($name, $explanation, $rationale, $metadata)
                 ON CONFLICT (name) DO UPDATE SET
                   explanation = EXCLUDED.explanation,
                   rationale = EXCLUDED.rationale,
                   metadata = EXCLUDED.metadata",
                &json!({"name": concept.name, "explanation": concept.explanation, "rationale": concept.rationale, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore concept error: {}", e) }], "isError": true }));
            }
            restored.concepts += 1;
        }

        for milestone in restore.milestones {
            let metadata = milestone.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Milestone (id, title, status, metadata)
                 VALUES ($id, $title, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   status = EXCLUDED.status,
                   metadata = EXCLUDED.metadata",
                &json!({"id": milestone.id, "title": milestone.title, "status": milestone.status, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore milestone error: {}", e) }], "isError": true }));
            }
            restored.milestones += 1;
        }

        for requirement in restore.requirements {
            let metadata = requirement.metadata.unwrap_or_else(|| "{}".to_string());
            let status = requirement.status.unwrap_or_else(|| "restored".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Requirement (id, title, description, status, priority, metadata)
                 VALUES ($id, $title, $description, $status, $priority, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   status = EXCLUDED.status,
                   priority = EXCLUDED.priority,
                   metadata = EXCLUDED.metadata",
                &json!({"id": requirement.id, "title": requirement.title, "description": requirement.description, "priority": requirement.priority, "status": status, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore requirement error: {}", e) }], "isError": true }));
            }
            restored.requirements += 1;
        }

        for decision in restore.decisions {
            let description = decision.description.unwrap_or_default();
            let context = decision.context.unwrap_or_default();
            let metadata = decision.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Decision (id, title, description, context, rationale, status, metadata)
                 VALUES ($id, $title, $description, $context, $rationale, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   context = EXCLUDED.context,
                   rationale = EXCLUDED.rationale,
                   status = EXCLUDED.status,
                   metadata = EXCLUDED.metadata",
                &json!({"id": decision.id, "title": decision.title, "description": description, "context": context, "rationale": decision.rationale, "status": decision.status, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore decision error: {}", e) }], "isError": true }));
            }
            restored.decisions += 1;
        }

        for validation in restore.validations {
            let metadata = validation.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Validation (id, method, result, timestamp, metadata)
                 VALUES ($id, $method, $result, $timestamp, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   method = EXCLUDED.method,
                   result = EXCLUDED.result,
                   timestamp = EXCLUDED.timestamp,
                   metadata = EXCLUDED.metadata",
                &json!({"id": validation.id, "method": validation.method, "result": validation.result, "timestamp": validation.timestamp, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore validation error: {}", e) }], "isError": true }));
            }
            restored.validations += 1;
        }

        for relation in restore.relations {
            if let Err(e) = self.restore_soll_relation(
                &relation.relation_type,
                &relation.source_id,
                &relation.target_id,
            ) {
                return Some(
                    json!({ "content": [{ "type": "text", "text": format!("SOLL restore relation error: {}", e) }], "isError": true }),
                );
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

    fn next_soll_numeric_id(
        &self,
        project_slug: &str,
        entity: &str,
    ) -> anyhow::Result<(&'static str, u64)> {
        let (prefix, reg_col, table, id_expr) = match entity {
            "pillar" => ("PIL", "last_pil", "soll.Pillar", "id"),
            "requirement" => ("REQ", "last_req", "soll.Requirement", "id"),
            "concept" => ("CPT", "last_cpt", "soll.Concept", "name"),
            "decision" => ("DEC", "last_dec", "soll.Decision", "id"),
            "milestone" => ("MIL", "last_mil", "soll.Milestone", "id"),
            "validation" => ("VAL", "last_val", "soll.Validation", "id"),
            _ => return Err(anyhow!("Unknown entity")),
        };

        self.graph_store.execute(&format!(
            "INSERT INTO soll.Registry (project_slug, id, last_pil, last_req, last_cpt, last_dec, last_mil, last_val) \
             VALUES ('{}', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0) ON CONFLICT (project_slug) DO NOTHING",
            escape_sql(project_slug)
        ))?;

        let current_query = format!(
            "SELECT COALESCE({}, 0) FROM soll.Registry WHERE project_slug = '{}'",
            reg_col,
            escape_sql(project_slug)
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
            escape_sql(project_slug)
        );
        let observed_max = self
            .query_single_column(&ids_query)?
            .into_iter()
            .filter_map(|value| parse_numeric_suffix(&value))
            .max()
            .unwrap_or(0);

        let next = current.max(observed_max) + 1;
        self.graph_store.execute(&format!(
            "UPDATE soll.Registry SET {} = {} WHERE project_slug = '{}'",
            reg_col,
            next,
            escape_sql(project_slug)
        ))?;

        Ok((prefix, next))
    }

    fn restore_soll_relation(
        &self,
        relation_type: &str,
        source_id: &str,
        target_id: &str,
    ) -> anyhow::Result<()> {
        let table_name = match relation_type {
            "EPITOMIZES" => "soll.EPITOMIZES",
            "BELONGS_TO" => "soll.BELONGS_TO",
            "EXPLAINS" => "soll.EXPLAINS",
            "SOLVES" => "soll.SOLVES",
            "TARGETS" => "soll.TARGETS",
            "VERIFIES" => "soll.VERIFIES",
            "ORIGINATES" => "soll.ORIGINATES",
            "SUPERSEDES" => "soll.SUPERSEDES",
            "CONTRIBUTES_TO" => "soll.CONTRIBUTES_TO",
            "REFINES" => "soll.REFINES",
            "IMPACTS" => "IMPACTS",
            "SUBSTANTIATES" => "SUBSTANTIATES",
            _ => return Ok(()),
        };

        self.graph_store.execute_param(
            &format!(
                "INSERT INTO {} (source_id, target_id)
                 SELECT ?, ?
                 WHERE NOT EXISTS (
                   SELECT 1 FROM {} WHERE source_id = ? AND target_id = ?
                 )",
                table_name, table_name
            ),
            &json!([source_id, target_id, source_id, target_id]),
        )?;
        Ok(())
    }
}

impl McpServer {
    pub(crate) fn axon_soll_apply_plan(&self, args: &Value) -> Option<Value> {
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let plan = args.get("plan")?;

        let mut created: Vec<Value> = Vec::new();
        let mut updated: Vec<Value> = Vec::new();
        let mut skipped: Vec<Value> = Vec::new();
        let mut errors: Vec<Value> = Vec::new();
        let mut id_map = serde_json::Map::new();

        self.apply_plan_entity_group(
            project_slug,
            "pillar",
            plan.get("pillars"),
            dry_run,
            &mut created,
            &mut updated,
            &mut skipped,
            &mut errors,
            &mut id_map,
        );
        self.apply_plan_entity_group(
            project_slug,
            "requirement",
            plan.get("requirements"),
            dry_run,
            &mut created,
            &mut updated,
            &mut skipped,
            &mut errors,
            &mut id_map,
        );
        self.apply_plan_entity_group(
            project_slug,
            "decision",
            plan.get("decisions"),
            dry_run,
            &mut created,
            &mut updated,
            &mut skipped,
            &mut errors,
            &mut id_map,
        );
        self.apply_plan_entity_group(
            project_slug,
            "milestone",
            plan.get("milestones"),
            dry_run,
            &mut created,
            &mut updated,
            &mut skipped,
            &mut errors,
            &mut id_map,
        );

        let summary = format!(
            "SOLL apply_plan {}: created={}, updated={}, skipped={}, errors={}",
            if dry_run { "DRY-RUN" } else { "APPLIED" },
            created.len(),
            updated.len(),
            skipped.len(),
            errors.len()
        );

        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "project_slug": project_slug,
                "dry_run": dry_run,
                "created": created,
                "updated": updated,
                "skipped": skipped,
                "errors": errors,
                "id_map": id_map
            },
            "isError": !errors.is_empty()
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_plan_entity_group(
        &self,
        project_slug: &str,
        entity: &str,
        items: Option<&Value>,
        dry_run: bool,
        created: &mut Vec<Value>,
        updated: &mut Vec<Value>,
        skipped: &mut Vec<Value>,
        errors: &mut Vec<Value>,
        id_map: &mut serde_json::Map<String, Value>,
    ) {
        let Some(items) = items.and_then(|v| v.as_array()) else {
            return;
        };

        for item in items {
            let Some(obj) = item.as_object() else {
                errors.push(json!({"entity": entity, "error": "item must be an object"}));
                continue;
            };

            let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let logical_key = obj
                .get("logical_key")
                .and_then(|v| v.as_str())
                .unwrap_or(title);

            if logical_key.trim().is_empty() {
                errors.push(json!({"entity": entity, "error": "missing logical_key or title"}));
                continue;
            }

            let existing_id = self.resolve_soll_id(entity, title, logical_key);
            if let Some(existing_id) = existing_id {
                if dry_run {
                    skipped.push(json!({"entity": entity, "logical_key": logical_key, "id": existing_id, "action": "would_update"}));
                    id_map.insert(logical_key.to_string(), json!(existing_id));
                    continue;
                }

                let mut data = serde_json::Map::new();
                data.insert("id".to_string(), json!(existing_id));
                for (k, v) in obj {
                    if k != "logical_key" {
                        data.insert(k.clone(), v.clone());
                    }
                }

                let resp = self.axon_soll_manager(&json!({
                    "action": "update",
                    "entity": entity,
                    "data": Value::Object(data)
                }));

                if soll_tool_is_error(resp.as_ref()) {
                    errors.push(json!({"entity": entity, "logical_key": logical_key, "id": existing_id, "error": soll_tool_text(resp.as_ref())}));
                } else {
                    updated.push(json!({"entity": entity, "logical_key": logical_key, "id": existing_id}));
                    id_map.insert(logical_key.to_string(), json!(existing_id));
                }
                continue;
            }

            if dry_run {
                skipped.push(json!({"entity": entity, "logical_key": logical_key, "action": "would_create"}));
                continue;
            }

            let mut data = serde_json::Map::new();
            data.insert("project_slug".to_string(), json!(project_slug));
            for (k, v) in obj {
                if k != "logical_key" {
                    data.insert(k.clone(), v.clone());
                }
            }

            let mut metadata = data
                .get("metadata")
                .and_then(|m| m.as_object())
                .cloned()
                .unwrap_or_default();
            metadata.insert("logical_key".to_string(), json!(logical_key));
            data.insert("metadata".to_string(), Value::Object(metadata));

            let resp = self.axon_soll_manager(&json!({
                "action": "create",
                "entity": entity,
                "data": Value::Object(data)
            }));

            if soll_tool_is_error(resp.as_ref()) {
                errors.push(json!({"entity": entity, "logical_key": logical_key, "error": soll_tool_text(resp.as_ref())}));
            } else {
                let created_id = soll_tool_text(resp.as_ref())
                    .and_then(extract_soll_id_from_message)
                    .unwrap_or_else(|| "unknown".to_string());
                created.push(json!({"entity": entity, "logical_key": logical_key, "id": created_id}));
                id_map.insert(logical_key.to_string(), json!(created_id));
            }
        }
    }

    fn resolve_soll_id(&self, entity: &str, title: &str, logical_key: &str) -> Option<String> {
        let table = match entity {
            "pillar" => "soll.Pillar",
            "requirement" => "soll.Requirement",
            "decision" => "soll.Decision",
            "milestone" => "soll.Milestone",
            _ => return None,
        };

        let by_metadata = format!(
            "SELECT id FROM {} WHERE metadata LIKE '%\"logical_key\":\"{}\"%' ORDER BY id DESC LIMIT 1",
            table,
            escape_sql(logical_key)
        );
        if let Some(found) = query_first_sql_cell(self, &by_metadata) {
            return Some(found);
        }

        if !title.trim().is_empty() {
            let by_title = format!(
                "SELECT id FROM {} WHERE title = '{}' ORDER BY id DESC LIMIT 1",
                table,
                escape_sql(title)
            );
            if let Some(found) = query_first_sql_cell(self, &by_title) {
                return Some(found);
            }
        }

        None
    }
}

fn query_first_sql_cell(server: &McpServer, query: &str) -> Option<String> {
    let raw = server.execute_raw_sql(query).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    let rows = parsed.get("rows")?.as_array()?;
    let first = rows.first()?.as_array()?;
    first.first()?.as_str().map(|s| s.to_string())
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

impl McpServer {
    pub(crate) fn axon_soll_apply_plan_v2(&self, args: &Value) -> Option<Value> {
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let author = args
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let plan = args.get("plan")?;

        let operations = self.build_plan_operations(project_slug, plan);
        let preview_id = format!("PRV-{}-{}", project_slug, now_unix_ms());
        let payload = json!({
            "project_slug": project_slug,
            "author": author,
            "dry_run": dry_run,
            "operations": operations
        });

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.RevisionPreview (preview_id, author, project_slug, payload, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (preview_id) DO UPDATE SET author = EXCLUDED.author, project_slug = EXCLUDED.project_slug, payload = EXCLUDED.payload, created_at = EXCLUDED.created_at",
            &json!([preview_id, author, project_slug, payload.to_string(), now_unix_ms()]),
        ) {
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan_v2 error: {}", e)}],
                "isError": true
            }));
        }

        let counts = summarize_ops(&operations);
        if dry_run {
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan_v2 DRY-RUN ready. preview_id={} (create={}, update={})", preview_id, counts.0, counts.1)}],
                "data": { "preview_id": preview_id, "counts": {"create": counts.0, "update": counts.1}, "operations": operations }
            }));
        }

        self.axon_soll_commit_revision(&json!({ "preview_id": preview_id, "author": author }))
    }

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
        let project_slug = payload
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");

        let revision_id = format!("REV-{}-{}", project_slug, now_unix_ms());
        let now = now_unix_ms();
        let _ = self.graph_store.execute("BEGIN TRANSACTION");

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.Revision (revision_id, author, source, summary, status, created_at, committed_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            &json!([revision_id, author, "mcp", "SOLL plan commit", "committed", now, now]),
        ) {
            let _ = self.graph_store.execute("ROLLBACK");
            return Some(json!({"content":[{"type":"text","text": format!("SOLL commit error (revision row): {}", e)}],"isError": true}));
        }

        for op in &operations {
            if let Err(e) = self.apply_operation_with_audit(&revision_id, op) {
                let _ = self.graph_store.execute("ROLLBACK");
                return Some(json!({"content":[{"type":"text","text": format!("SOLL commit error (operation): {}", e)}],"isError": true}));
            }
        }

        let _ = self.graph_store.execute("COMMIT");
        let _ = self.graph_store.execute(&format!(
            "DELETE FROM soll.RevisionPreview WHERE preview_id = '{}'",
            escape_sql(preview_id)
        ));

        Some(json!({
            "content": [{"type":"text","text": format!("SOLL revision committed: {} ({} operations)", revision_id, operations.len())}],
            "data": {"revision_id": revision_id, "operations": operations.len()}
        }))
    }

    pub(crate) fn axon_soll_query_context(&self, args: &Value) -> Option<Value> {
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25).max(1);

        let reqs = self.query_single_column(&format!(
            "SELECT id || '|' || title || '|' || COALESCE(status,'') || '|' || COALESCE(priority,'') FROM soll.Requirement WHERE id LIKE 'REQ-{}-%' ORDER BY id DESC LIMIT {}",
            escape_sql(project_slug),
            limit
        )).unwrap_or_default();
        let decisions = self.query_single_column(&format!(
            "SELECT id || '|' || title || '|' || COALESCE(status,'') FROM soll.Decision WHERE id LIKE 'DEC-{}-%' ORDER BY id DESC LIMIT {}",
            escape_sql(project_slug),
            limit
        )).unwrap_or_default();
        let revisions = self.query_single_column(&format!(
            "SELECT revision_id || '|' || COALESCE(summary,'') || '|' || COALESCE(author,'') FROM soll.Revision ORDER BY committed_at DESC LIMIT {}",
            limit
        )).unwrap_or_default();

        Some(json!({
            "content": [{"type":"text","text": format!("SOLL context for {} loaded.", project_slug)}],
            "data": {
                "project_slug": project_slug,
                "requirements": reqs,
                "decisions": decisions,
                "revisions": revisions
            }
        }))
    }

    pub(crate) fn axon_soll_work_plan(&self, args: &Value) -> Option<Value> {
        let project_slug = args.get("project_slug")?.as_str()?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .max(1) as usize;
        let top = args
            .get("top")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .max(1) as usize;
        let include_ist = args
            .get("include_ist")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("brief");

        let mut nodes = self.load_work_plan_nodes(project_slug);
        let edges = self.load_work_plan_edges(project_slug);
        let adjacency = build_adjacency_map(&edges);
        let cycle_sets = detect_cycle_sets(nodes.keys(), &adjacency);
        let cycle_node_ids = cycle_sets
            .iter()
            .flat_map(|set| set.iter().cloned())
            .collect::<HashSet<_>>();
        let blocked_by_cycles = collect_blocked_by_cycles(&adjacency, &cycle_node_ids);
        let backlog_visible = self
            .project_scope_summary(Some(project_slug))
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
        let global_validation = self.axon_soll_verify_requirements(&json!({ "project_slug": project_slug }));
        let soll_validation = self.axon_validate_soll();
        let validation_gates = json!({
            "requirement_verification": global_validation
                .as_ref()
                .and_then(|resp| resp.get("data"))
                .cloned()
                .unwrap_or(json!({})),
            "soll_validation": soll_validation
                .as_ref()
                .and_then(|resp| resp.get("content"))
                .cloned()
                .unwrap_or(json!([])),
            "backlog_visible": backlog_visible
        });
        let data = json!({
            "summary": {
                "project_slug": project_slug,
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
            format!("SOLL work plan generated for {}.", project_slug)
        } else {
            self.render_work_plan_text(
                project_slug,
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
            let metadata = art.get("metadata").cloned().unwrap_or(json!({})).to_string();
            let trace_id = format!("TRC-{}-{}-{}", entity_id, now, idx);

            if self.graph_store.execute_param(
                "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &json!([trace_id, entity_type, entity_id, artifact_type, artifact_ref, confidence, metadata, now]),
            ).is_ok() {
                attached += 1;
            }
        }

        Some(json!({
            "content": [{"type":"text","text": format!("Attached {} evidence item(s) to {}:{}", attached, entity_type, entity_id)}],
            "data": {"attached": attached}
        }))
    }

    fn load_work_plan_nodes(&self, project_slug: &str) -> HashMap<String, WorkPlanNode> {
        let mut nodes = HashMap::new();
        let req_query = format!(
            "SELECT r.id, r.title, COALESCE(r.status,''), COALESCE(r.priority,''), COUNT(t.id), COALESCE(r.acceptance_criteria,'')
             FROM soll.Requirement r
             LEFT JOIN soll.Traceability t ON t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
             WHERE r.id LIKE 'REQ-{}-%'
             GROUP BY 1,2,3,4,6
             ORDER BY r.id",
            escape_sql(project_slug)
        );
        if let Ok(raw) = self.graph_store.query_json(&req_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() < 6 {
                    continue;
                }
                let evidence_count = row[4].parse::<usize>().unwrap_or(0);
                let criteria = row[5].clone();
                let status = row[2].clone();
                let requirement_state =
                    requirement_state_from(status.as_str(), &criteria, evidence_count).to_string();
                let id = row[0].clone();
                nodes.insert(
                    id.clone(),
                    WorkPlanNode {
                        id,
                        title: row[1].clone(),
                        entity_type: WorkPlanEntityType::Requirement,
                        status,
                        priority: row[3].clone(),
                        requirement_state: Some(requirement_state),
                        evidence_count,
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
            "SELECT id, title, COALESCE(status,'') FROM soll.Decision WHERE id LIKE 'DEC-{}-%' ORDER BY id",
            escape_sql(project_slug)
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
            "SELECT id, title, COALESCE(status,'') FROM soll.Milestone WHERE id LIKE 'MIL-{}-%' ORDER BY id",
            escape_sql(project_slug)
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

    fn load_work_plan_edges(&self, project_slug: &str) -> Vec<(String, String)> {
        let mut edges = Vec::new();
        let solves_query = format!(
            "SELECT source_id, target_id FROM soll.SOLVES
             WHERE source_id LIKE 'DEC-{}-%' AND target_id LIKE 'REQ-{}-%'",
            escape_sql(project_slug),
            escape_sql(project_slug)
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
            "SELECT source_id, target_id FROM soll.BELONGS_TO
             WHERE source_id LIKE 'REQ-{}-%'
               AND (target_id LIKE 'REQ-{}-%' OR target_id LIKE 'MIL-{}-%')",
            escape_sql(project_slug),
            escape_sql(project_slug),
            escape_sql(project_slug)
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
        project_slug: &str,
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
                let kind = rec
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("task");
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
            project_slug,
            format_standard_contract(
                "ok",
                "work plan computed from SOLL",
                &format!("project:{}", project_slug),
                &evidence,
                &["review blockers before execution", "use `format=json` for machine consumption"],
                "medium",
            )
        )
    }

    pub(crate) fn axon_soll_verify_requirements(&self, args: &Value) -> Option<Value> {
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let query = format!(
            "SELECT r.id, COALESCE(r.status,''), COALESCE(r.acceptance_criteria,''), COUNT(t.id)
             FROM soll.Requirement r
             LEFT JOIN soll.Traceability t ON t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
             WHERE r.id LIKE 'REQ-{}-%'
             GROUP BY 1,2,3
             ORDER BY r.id",
            escape_sql(project_slug)
        );
        let rows_raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
        let mut done = 0usize;
        let mut partial = 0usize;
        let mut missing = 0usize;
        let mut details: Vec<Value> = Vec::new();

        for row in rows {
            if row.len() < 4 {
                continue;
            }
            let id = row[0].clone();
            let status = row[1].clone();
            let criteria = row[2].clone();
            let evidence_count = row[3].parse::<usize>().unwrap_or(0);
            let has_criteria = !criteria.trim().is_empty() && criteria.trim() != "[]";

            let state = if evidence_count > 0 && has_criteria && (status == "current" || status == "accepted") {
                done += 1;
                "done"
            } else if evidence_count > 0 || has_criteria {
                partial += 1;
                "partial"
            } else {
                missing += 1;
                "missing"
            };

            details.push(json!({"id": id, "state": state, "status": status, "evidence_count": evidence_count}));
        }

        Some(json!({
            "content": [{"type":"text","text": format!("Requirement verification: done={}, partial={}, missing={}", done, partial, missing)}],
            "data": {"project_slug": project_slug, "done": done, "partial": partial, "missing": missing, "details": details}
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
                return Some(json!({"content":[{"type":"text","text": format!("Rollback failed: {}", e)}],"isError": true}));
            }
        }

        let _ = self.graph_store.execute("COMMIT");
        let _ = self.graph_store.execute(&format!(
            "UPDATE soll.Revision SET status = 'rolled_back' WHERE revision_id = '{}'",
            escape_sql(revision_id)
        ));
        Some(json!({"content":[{"type":"text","text": format!("Revision rolled back: {}", revision_id)}]}))
    }

    fn build_plan_operations(&self, project_slug: &str, plan: &Value) -> Vec<Value> {
        let mut operations = Vec::new();
        for entity in ["pillar", "requirement", "decision", "milestone"] {
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
                        let existing_id = self.resolve_soll_id(entity, title, logical_key);
                        let kind = if existing_id.is_some() { "update" } else { "create" };
                        operations.push(json!({
                            "kind": kind,
                            "entity": entity,
                            "project_slug": project_slug,
                            "logical_key": logical_key,
                            "entity_id": existing_id,
                            "payload": Value::Object(obj.clone())
                        }));
                    }
                }
            }
        }
        operations
    }

    fn apply_operation_with_audit(&self, revision_id: &str, op: &Value) -> anyhow::Result<()> {
        let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("create");
        let entity = op.get("entity").and_then(|v| v.as_str()).unwrap_or("requirement");
        let payload = op.get("payload").cloned().unwrap_or(json!({}));
        let project_slug = op
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let entity_id_hint = op
            .get("entity_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let before = if let Some(id) = entity_id_hint.clone() {
            self.snapshot_entity(entity, &id).unwrap_or(json!({}))
        } else {
            json!({})
        };

        let result = if kind == "update" && entity_id_hint.is_some() {
            let mut data = payload.clone();
            data["id"] = json!(entity_id_hint.clone().unwrap_or_default());
            self.axon_soll_manager(&json!({"action":"update","entity":entity,"data":data}))
        } else {
            let mut data = payload.clone();
            data["project_slug"] = json!(project_slug);
            self.axon_soll_manager(&json!({"action":"create","entity":entity,"data":data}))
        };

        if soll_tool_is_error(result.as_ref()) {
            return Err(anyhow!(
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

        let after = self.snapshot_entity(entity, &entity_id).unwrap_or(json!({}));
        self.graph_store.execute_param(
            "INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, action, before_json, after_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            &json!([
                revision_id,
                entity,
                entity_id,
                kind,
                before.to_string(),
                after.to_string(),
                now_unix_ms()
            ]),
        )?;
        Ok(())
    }

    fn apply_rollback_operation(&self, op: &Value) -> anyhow::Result<()> {
        let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let entity = op.get("entity").and_then(|v| v.as_str()).unwrap_or("");
        let entity_id = op.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");

        match (kind, entity) {
            ("delete", "pillar") => self.graph_store.execute(&format!("DELETE FROM soll.Pillar WHERE id = '{}'", escape_sql(entity_id)))?,
            ("delete", "requirement") => self.graph_store.execute(&format!("DELETE FROM soll.Requirement WHERE id = '{}'", escape_sql(entity_id)))?,
            ("delete", "decision") => self.graph_store.execute(&format!("DELETE FROM soll.Decision WHERE id = '{}'", escape_sql(entity_id)))?,
            ("delete", "milestone") => self.graph_store.execute(&format!("DELETE FROM soll.Milestone WHERE id = '{}'", escape_sql(entity_id)))?,
            ("restore", _) => {
                let before = op.get("before").cloned().unwrap_or(json!({}));
                let mut data = before;
                data["id"] = json!(entity_id);
                let resp = self.axon_soll_manager(&json!({"action":"update","entity":entity,"data":data}));
                if soll_tool_is_error(resp.as_ref()) {
                    return Err(anyhow!("{}", soll_tool_text(resp.as_ref()).unwrap_or_default()));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot_entity(&self, entity: &str, entity_id: &str) -> Option<Value> {
        let query = match entity {
            "pillar" => format!("SELECT title, description, metadata FROM soll.Pillar WHERE id = '{}'", escape_sql(entity_id)),
            "requirement" => format!("SELECT title, description, status, priority, metadata, owner, acceptance_criteria, evidence_refs FROM soll.Requirement WHERE id = '{}'", escape_sql(entity_id)),
            "decision" => format!("SELECT title, description, context, rationale, status, metadata, supersedes_decision_id, impact_scope FROM soll.Decision WHERE id = '{}'", escape_sql(entity_id)),
            "milestone" => format!("SELECT title, status, metadata FROM soll.Milestone WHERE id = '{}'", escape_sql(entity_id)),
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
