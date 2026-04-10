use anyhow::anyhow;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use super::format::format_standard_contract;
use super::soll::{
    canonical_soll_export_dir, find_latest_soll_export, parse_soll_export, SollRestoreCounts,
};
use super::McpServer;
use crate::project_meta::{discover_project_identities, resolve_canonical_project_identity};

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
struct RelationPolicy {
    allowed: &'static [&'static str],
    default: Option<&'static str>,
    allow_multiple_types: bool,
}

fn relation_table_name(relation_type: &str) -> Option<&'static str> {
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
        }),
        ("REQ", "PIL") => Some(RelationPolicy {
            allowed: &["BELONGS_TO"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: false,
        }),
        ("CPT", "REQ") => Some(RelationPolicy {
            allowed: &["EXPLAINS", "REFINES"],
            default: Some("EXPLAINS"),
            allow_multiple_types: true,
        }),
        ("DEC", "REQ") => Some(RelationPolicy {
            allowed: &["SOLVES", "REFINES"],
            default: Some("SOLVES"),
            allow_multiple_types: true,
        }),
        ("MIL", "REQ") => Some(RelationPolicy {
            allowed: &["TARGETS"],
            default: Some("TARGETS"),
            allow_multiple_types: false,
        }),
        ("VAL", "REQ") => Some(RelationPolicy {
            allowed: &["VERIFIES"],
            default: Some("VERIFIES"),
            allow_multiple_types: false,
        }),
        ("STK", "REQ") => Some(RelationPolicy {
            allowed: &["ORIGINATES", "CONTRIBUTES_TO"],
            default: Some("ORIGINATES"),
            allow_multiple_types: true,
        }),
        ("GUI", "GUI") => Some(RelationPolicy {
            allowed: &["INHERITS_FROM"],
            default: Some("INHERITS_FROM"),
            allow_multiple_types: false,
        }),
        ("REQ", "GUI") => Some(RelationPolicy {
            allowed: &["BELONGS_TO", "COMPLIES_WITH"],
            default: Some("BELONGS_TO"),
            allow_multiple_types: true,
        }),
        ("DEC", "GUI") => Some(RelationPolicy {
            allowed: &["COMPLIES_WITH"],
            default: Some("COMPLIES_WITH"),
            allow_multiple_types: false,
        }),
        ("DEC", "DEC") => Some(RelationPolicy {
            allowed: &["SUPERSEDES", "REFINES"],
            default: None,
            allow_multiple_types: false,
        }),
        ("REQ", "REQ") => Some(RelationPolicy {
            allowed: &["REFINES", "BELONGS_TO"],
            default: Some("REFINES"),
            allow_multiple_types: false,
        }),
        ("MIL", "MIL") => Some(RelationPolicy {
            allowed: &["SUPERSEDES"],
            default: None,
            allow_multiple_types: false,
        }),
        ("VAL", "VAL") => Some(RelationPolicy {
            allowed: &["REFINES", "SUPERSEDES"],
            default: None,
            allow_multiple_types: false,
        }),
        ("DEC", "ART") => Some(RelationPolicy {
            allowed: &["IMPACTS", "SUBSTANTIATES"],
            default: Some("IMPACTS"),
            allow_multiple_types: true,
        }),
        ("REQ", "ART") | ("VAL", "ART") => Some(RelationPolicy {
            allowed: &["SUBSTANTIATES"],
            default: Some("SUBSTANTIATES"),
            allow_multiple_types: false,
        }),
        _ => None,
    }
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
    pub(crate) fn axon_soll_manager(&self, args: &Value) -> Option<Value> {
        let action = args.get("action")?.as_str()?;
        let entity = args.get("entity")?.as_str()?;
        let data = args.get("data")?;

        match action {
            "create" => {
                let project_slug = args
                    .get("project_slug")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("project_slug").and_then(|v| v.as_str()))
                    .unwrap_or("AXO");
                let reserved_id = args.get("reserved_id").and_then(|value| value.as_str());
                let (project_slug, project_code, formatted_id) = if let Some(reserved_id) = reserved_id
                {
                    match self.resolve_canonical_project_identity_for_mutation(project_slug) {
                        Ok((canonical_slug, project_code)) => {
                            (canonical_slug, project_code, reserved_id.to_string())
                        }
                        Err(e) => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Erreur registre: {}", e) }], "isError": true }),
                            )
                        }
                    }
                } else {
                    match self.next_soll_numeric_id(project_slug, entity) {
                        Ok((canonical_slug, project_code, prefix, next_num)) => (
                            canonical_slug,
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

                let q = "INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET project_slug = EXCLUDED.project_slug, project_code = EXCLUDED.project_code, title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata";

                let insert_res = self.graph_store.execute_param(
                    q,
                    &json!([
                        formatted_id,
                        entity_type_cap,
                        project_slug,
                        project_code,
                        title,
                        description,
                        status,
                        meta.to_string()
                    ]),
                );

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
                match self.select_relation_type_for_link(src, tgt, explicit_rel) {
                    Ok((relation_type, policy)) => {
                        let rel_table = relation_table_name(relation_type).unwrap_or(relation_type);
                        match self.insert_validated_relation(relation_type, src, tgt, policy) {
                            Ok(true) => Some(
                                json!({ "content": [{ "type": "text", "text": format!("✅ Liaison établie : `{}` -> `{}` (via {})", src, tgt, rel_table) }] }),
                            ),
                            Ok(false) => Some(
                                json!({ "content": [{ "type": "text", "text": format!("ℹ️ Liaison déjà présente : `{}` -> `{}` (via {})", src, tgt, rel_table) }] }),
                            ),
                            Err(e) => Some(
                                json!({ "content": [{ "type": "text", "text": format!("Erreur liaison: {}", e) }], "isError": true }),
                            ),
                        }
                    }
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur liaison: {}", e) }], "isError": true }),
                    ),
                }
            }
            _ => None,
        }
    }

    pub(crate) fn axon_export_soll(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        let project_slug = args.get("project_slug").and_then(|v| v.as_str());
        let project_code = match project_slug
            .map(|slug| self.resolve_project_code(slug))
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

        if let Some(slug) = project_slug {
            markdown.push_str(&format!(
                "*Portée : projet `{}`*

",
                slug
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
        let project_slug = args.get("project_slug").and_then(|v| v.as_str());
        let project_code = match project_slug
            .map(|slug| self.resolve_project_code(slug))
            .transpose()
        {
            Ok(code) => code,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                    "isError": true
                }))
            }
        };
        let orphan_requirements = self
            .query_single_column(
                &format!("SELECT id FROM soll.Node r
                 WHERE type = 'Requirement'
                   AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE source_id = r.id OR target_id = r.id)
                   {}
                 ORDER BY id", project_scope_predicate("r.id", project_code.as_deref())),
            )
            .ok()?;

        let validations_without_verifies = self
            .query_single_column(
                &format!("SELECT id FROM soll.Node v
                 WHERE type = 'Validation'
                   AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = v.id OR target_id = v.id) AND relation_type = 'VERIFIES')
                   {}
                 ORDER BY id", project_scope_predicate("v.id", project_code.as_deref())),
            )
            .ok()?;

        let decisions_without_links = self
            .query_single_column(
                &format!("SELECT id FROM soll.Node d
                 WHERE type = 'Decision'
                   AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = d.id OR target_id = d.id) AND relation_type IN ('SOLVES', 'IMPACTS'))
                   {}
                 ORDER BY id", project_scope_predicate("d.id", project_code.as_deref())),
            )
            .ok()?;

        let relation_policy_violations = self
            .collect_relation_policy_violations(project_code.as_deref())
            .ok()?;

        let violation_count = orphan_requirements.len()
            + validations_without_verifies.len()
            + decisions_without_links.len()
            + relation_policy_violations.len();

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

        if !relation_policy_violations.is_empty() {
            evidence.push_str("\n- Relations invalides:\n");
            for violation in relation_policy_violations {
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
                &match project_slug {
                    Some(slug) => format!("project:{}", slug),
                    None => "workspace:*".to_string(),
                },
                &evidence,
                &[
                    "run `soll_verify_requirements` for requirement-level coverage",
                    "apply targeted SOLL links with `soll_manager` if needed"
                ],
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
            "INSERT INTO soll.Registry (project_slug, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_prv, last_rev)
             VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_slug) DO NOTHING"
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
                "INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata)
                 VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'AXO', $title, $description, NULL, $metadata)
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
                "INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, metadata)
                 VALUES ($id, 'Concept', $project_slug, $project_code, $name, $explanation, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": cpt.id,
                    "project_slug": "AXO".to_string(),
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
            self.graph_store
                .sync_project_code_registry_entry(&identity.slug, &identity.code, None)?;
        }
        Ok(())
    }

    fn resolve_canonical_project_identity_for_mutation(
        &self,
        project_slug: &str,
    ) -> anyhow::Result<(String, String)> {
        let identity = resolve_canonical_project_identity(project_slug)?;
        self.graph_store
            .sync_project_code_registry_entry(&identity.slug, &identity.code, None)?;
        Ok((identity.slug, identity.code))
    }

    fn resolve_project_code(&self, project_slug: &str) -> anyhow::Result<String> {
        let _ = self.sync_project_code_registry_from_meta();
        let escaped = escape_sql(project_slug);
        let by_slug = self.query_single_column(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_slug = '{}'",
            escaped
        ))?;
        if let Some(code) = by_slug.into_iter().next() {
            return Ok(code);
        }

        if let Ok(identity) = resolve_canonical_project_identity(project_slug) {
            self.graph_store
                .sync_project_code_registry_entry(&identity.slug, &identity.code, None)?;
            return Ok(identity.code);
        }

        if let Err(e) = resolve_canonical_project_identity(project_slug) {
            return Err(e);
        }

        Err(anyhow!(
            "Projet canonique `{}` introuvable dans `.axon/meta.json` ou soll.ProjectCodeRegistry",
            project_slug
        ))
    }

    pub(crate) fn next_server_numeric_id(
        &self,
        project_slug: &str,
        kind: &str,
    ) -> anyhow::Result<(String, String, &'static str, u64)> {
        let (project_slug, project_code) =
            self.resolve_canonical_project_identity_for_mutation(project_slug)?;
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
            "INSERT INTO soll.Registry (project_slug, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_prv, last_rev) \
             VALUES (?, 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) ON CONFLICT (project_slug) DO NOTHING",
            &json!([project_slug]),
        )?;

        let current_query = format!(
            "SELECT COALESCE({}, 0) FROM soll.Registry WHERE project_slug = '{}'",
            reg_col,
            escape_sql(&project_slug)
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
            "UPDATE soll.Registry SET {} = {} WHERE project_slug = '{}'",
            reg_col,
            next,
            escape_sql(&project_slug)
        ))?;

        Ok((project_slug, project_code, prefix, next))
    }

    pub(crate) fn next_soll_numeric_id(
        &self,
        project_slug: &str,
        entity: &str,
    ) -> anyhow::Result<(String, String, &'static str, u64)> {
        self.next_server_numeric_id(project_slug, entity)
    }

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

        let (canonical_slug, _) = match self
            .resolve_canonical_project_identity_for_mutation(project_slug)
        {
            Ok(identity) => identity,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Erreur projet canonique: {}", e) }],
                    "isError": true
                }))
            }
        };

        let operations = self.build_plan_operations(&canonical_slug, args);
        let preview_id = if let Some(reserved_preview_id) = args
            .get("reserved_preview_id")
            .and_then(|value| value.as_str())
        {
            reserved_preview_id.to_string()
        } else {
            let (_, project_code, _, next_preview) =
                match self.next_server_numeric_id(&canonical_slug, "preview") {
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
            "project_slug": canonical_slug,
            "author": author,
            "dry_run": dry_run,
            "operations": operations
        });

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.RevisionPreview (preview_id, author, project_slug, payload, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (preview_id) DO UPDATE SET author = EXCLUDED.author, project_slug = EXCLUDED.project_slug, payload = EXCLUDED.payload, created_at = EXCLUDED.created_at",
            &json!([preview_id, author, canonical_slug, payload.to_string(), now_unix_ms()]),
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
    fn resolve_soll_id(&self, entity: &str, title: &str, logical_key: &str) -> Option<String> {
        let table = match entity {
            "pillar" => "soll.Node",
            "requirement" => "soll.Node",
            "decision" => "soll.Node",
            "milestone" => "soll.Node",
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

fn project_scope_predicate(id_column: &str, project_code: Option<&str>) -> String {
    project_code
        .map(|code| format!("AND {} LIKE '%-{}-%'", id_column, escape_sql(code)))
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
        let project_slug = payload
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");

        let revision_id = if let Some(reserved_revision_id) = args
            .get("reserved_revision_id")
            .and_then(|value| value.as_str())
        {
            reserved_revision_id.to_string()
        } else {
            let (_, project_code, _, next_revision) =
                match self.next_server_numeric_id(project_slug, "revision") {
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
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let project_code = self.resolve_project_code(project_slug).ok()?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(25)
            .max(1);

        let reqs = self.query_single_column(&format!(
            "SELECT id || '|' || title || '|' || COALESCE(status,'') FROM soll.Node WHERE type='Requirement' AND id LIKE 'REQ-{}-%' ORDER BY id DESC LIMIT {}",
            escape_sql(&project_code),
            limit
        )).unwrap_or_default();
        let decisions = self.query_single_column(&format!(
            "SELECT id || '|' || title || '|' || COALESCE(status,'') FROM soll.Node WHERE type='Decision' AND id LIKE 'DEC-{}-%' ORDER BY id DESC LIMIT {}",
            escape_sql(&project_code),
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
        let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(5).max(1) as usize;
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
        let global_validation =
            self.axon_soll_verify_requirements(&json!({ "project_slug": project_slug }));
        let soll_validation = self.axon_validate_soll(&json!({ "project_slug": project_slug }));
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
            let metadata = art
                .get("metadata")
                .cloned()
                .unwrap_or(json!({}))
                .to_string();
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
        let Ok(project_code) = self.resolve_project_code(project_slug) else {
            return HashMap::new();
        };
        let mut nodes = HashMap::new();
        let req_query = format!(
            "SELECT r.id, r.title, COALESCE(r.status,''), COALESCE(r.metadata,'{{}}'), COUNT(t.id)
             FROM soll.Node r
             LEFT JOIN soll.Traceability t ON t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
             WHERE r.type = 'Requirement' AND r.id LIKE 'REQ-{}-%'
             GROUP BY 1,2,3,4
             ORDER BY r.id",
            escape_sql(&project_code)
        );
        if let Ok(raw) = self.graph_store.query_json(&req_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() < 5 {
                    continue;
                }
                let meta: serde_json::Value =
                    serde_json::from_str(&row[3]).unwrap_or(serde_json::json!({}));
                let priority = meta
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let criteria = meta
                    .get("acceptance_criteria")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let evidence_count = row[4].parse::<usize>().unwrap_or(0);
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
                        priority,
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

    fn load_work_plan_edges(&self, project_slug: &str) -> Vec<(String, String)> {
        let Ok(project_code) = self.resolve_project_code(project_slug) else {
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
            project_slug,
            format_standard_contract(
                "ok",
                "work plan computed from SOLL",
                &format!("project:{}", project_slug),
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
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let project_code = self.resolve_project_code(project_slug).ok()?;
        let query = format!(
            "SELECT r.id, COALESCE(r.status,''), COALESCE(r.acceptance_criteria,''), COUNT(t.id)
             FROM soll.Node r WHERE r.type='Requirement' AND
             LEFT JOIN soll.Traceability t ON t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
             WHERE r.id LIKE 'REQ-{}-%'
             GROUP BY 1,2,3
             ORDER BY r.id",
            escape_sql(&project_code)
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

            let state = if evidence_count > 0
                && has_criteria
                && (status == "current" || status == "accepted")
            {
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

    fn build_plan_operations(&self, project_slug: &str, args: &Value) -> Vec<Value> {
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
                            let existing_id = self.resolve_soll_id(entity, title, logical_key);
                            let kind = if existing_id.is_some() {
                                "update"
                            } else {
                                "create"
                            };
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
        }

        // 2. Relations
        if let Some(relations) = args.get("relations").and_then(|v| v.as_array()) {
            for rel in relations {
                if let Some(obj) = rel.as_object() {
                    operations.push(json!({
                        "kind": "link",
                        "entity": "relation",
                        "project_slug": project_slug,
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
        let project_slug = op
            .get("project_slug")
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
            data["project_slug"] = serde_json::json!(project_slug);
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
        add_cmd.arg("docs/vision/");

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
                    "content": [{ "type": "text", "text": format!("Validation réussie.\n\n{}\n\nExport Report:\n{}", status, export_report) }]
                }))
            }
            Err(e) => Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Git commit failed: {}", e) }],
                "isError": true
            })),
        }
    }

    pub(crate) fn axon_init_project(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        let project_name = args.get("project_name")?.as_str()?;
        let project_slug = args.get("project_slug")?.as_str()?;
        let concept_text = args
            .get("concept_document_url_or_text")
            .and_then(|v| v.as_str());

        // 1. Register project
        if let Err(e) = self
            .graph_store
            .sync_project_code_registry_entry(project_slug, project_slug, None)
        {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Erreur lors de l'enregistrement du projet: {}", e) }],
                "isError": true
            }));
        }

        // 2. Fetch global guidelines
        let rows_raw = self.graph_store.query_json(
            "SELECT id, title, description, metadata FROM soll.Node WHERE type='Guideline' AND project_slug='GLOBAL'"
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
            project_name, project_slug
        );

        if let Some(concept) = concept_text {
            response_text.push_str(&format!(
                "📄 Un document de concept a été détecté. Extrayez-en la Vision et les Piliers, et utilisez `soll_manager` pour les créer sous le projet {}.\n\n",
                project_slug
            ));
        }

        response_text.push_str("Voici les règles globales disponibles. Lesquelles souhaitez-vous activer, ignorer ou spécialiser pour ce projet ?\n");
        response_text.push_str(&rules_text);
        response_text
            .push_str("\n(Utilisez l'outil `axon_apply_guidelines` pour appliquer ces choix).");

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": response_text }]
        }))
    }

    pub(crate) fn axon_apply_guidelines(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let project_slug = args.get("project_slug")?.as_str()?;
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
                let (p_slug, p_code, prefix, num) = self
                    .next_soll_numeric_id(project_slug, "guideline")
                    .unwrap();
                let local_id = format!("{}-{}-{:03}", prefix, p_code, num);

                // Insert local rule
                let _ = self.graph_store.execute_param(
                    "INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) 
                     VALUES (?, 'Guideline', ?, ?, ?, ?, 'active', ?)",
                    &serde_json::json!([local_id, p_slug, p_code, title, desc, meta])
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
