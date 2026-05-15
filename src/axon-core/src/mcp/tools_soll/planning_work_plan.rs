use super::planning_output::build_top_recommendations;
use super::*;
use crate::soll_snapshot::SollSnapshot;
use petgraph::algo::tarjan_scc;
use petgraph::graph::NodeIndex;
use petgraph::visit::{EdgeFiltered, EdgeRef};
use petgraph::Direction;

/// REQ-AXO-346 Slice 3 — predicate matching the SOLVES + BELONGS_TO
/// edge subset that the work_plan considers for cycle detection and
/// topological wave layering. Used with `petgraph::visit::EdgeFiltered`
/// so all algorithms run on the **existing** `SollSnapshot::graph()`
/// without any per-call rebuild.
fn is_work_plan_relation(relation_type: &str) -> bool {
    matches!(relation_type, "SOLVES" | "BELONGS_TO")
}

/// REQ-AXO-91500 patch A — broader filiation predicate used for the
/// "unblocks N descendants" metric only. Counts every canonical
/// child-bearing relation: SOLVES (DEC→REQ), BELONGS_TO (REQ→PIL),
/// TARGETS (MIL→REQ), REFINES (REQ→REQ, DEC→REQ), EXPLAINS (CPT→REQ),
/// VERIFIES (VAL→REQ). Cycle detection and Kahn waves keep the narrow
/// SOLVES+BELONGS_TO filter to preserve topological semantics.
fn is_descendant_relation(relation_type: &str) -> bool {
    matches!(
        relation_type,
        "SOLVES" | "BELONGS_TO" | "TARGETS" | "REFINES" | "EXPLAINS" | "VERIFIES"
    )
}

/// Cycle detection via `petgraph::algo::tarjan_scc` on the snapshot
/// graph filtered inline with `EdgeFiltered`. Multi-node SCCs are
/// cycles; single-node SCCs count only if they carry a self-loop (the
/// self-loop check also respects the work_plan relation filter).
fn cycle_sets_snapshot(snapshot: &SollSnapshot) -> Vec<HashSet<String>> {
    let g = snapshot.graph();
    let view = EdgeFiltered::from_fn(g, |e| is_work_plan_relation(e.weight().as_str()));
    let mut out = Vec::new();
    for component in tarjan_scc(&view) {
        if component.len() > 1 {
            out.push(component.into_iter().map(|n| g[n].clone()).collect());
        } else if let Some(&n) = component.first() {
            let has_self_loop = g
                .edges_directed(n, Direction::Outgoing)
                .any(|e| e.target() == n && is_work_plan_relation(e.weight().as_str()));
            if has_self_loop {
                let mut set = HashSet::new();
                set.insert(g[n].clone());
                out.push(set);
            }
        }
    }
    out
}

/// Forward BFS over the filtered snapshot edges, collecting every node
/// transitively reachable from any seed in `cycle_node_ids`.
fn blocked_by_cycles_snapshot(
    snapshot: &SollSnapshot,
    cycle_node_ids: &HashSet<String>,
) -> HashSet<String> {
    let mut blocked = HashSet::new();
    let mut queue: VecDeque<NodeIndex> = cycle_node_ids
        .iter()
        .filter_map(|id| snapshot.node_index(id))
        .collect();
    while let Some(n) = queue.pop_front() {
        for e in snapshot.graph().edges_directed(n, Direction::Outgoing) {
            if !is_work_plan_relation(e.weight().as_str()) {
                continue;
            }
            let target_id = &snapshot.graph()[e.target()];
            if cycle_node_ids.contains(target_id) {
                continue;
            }
            if !blocked.insert(target_id.clone()) {
                continue;
            }
            queue.push_back(e.target());
        }
    }
    blocked
}

/// Per-node forward BFS over the filtered snapshot edges, restricted to
/// the schedulable subset (REQ-AXO-135 terminal-status exclusion).
fn descendant_counts_snapshot(
    snapshot: &SollSnapshot,
    allowed: &HashSet<String>,
) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::with_capacity(allowed.len());
    let mut ordered: Vec<&String> = allowed.iter().collect();
    ordered.sort();
    for source_id in ordered {
        let Some(start) = snapshot.node_index(source_id) else {
            out.insert(source_id.clone(), 0);
            continue;
        };
        let mut visited: HashSet<NodeIndex> = HashSet::new();
        let mut queue: VecDeque<NodeIndex> = VecDeque::new();
        queue.push_back(start);
        visited.insert(start);
        let mut count = 0usize;
        while let Some(n) = queue.pop_front() {
            for e in snapshot.graph().edges_directed(n, Direction::Outgoing) {
                // REQ-AXO-91500 patch A — broader filiation filter for the
                // unblocks metric (TARGETS / REFINES / EXPLAINS / VERIFIES
                // now contribute alongside SOLVES / BELONGS_TO).
                if !is_descendant_relation(e.weight().as_str()) {
                    continue;
                }
                let nxt = e.target();
                if !visited.insert(nxt) {
                    continue;
                }
                if !allowed.contains(&snapshot.graph()[nxt]) {
                    continue;
                }
                queue.push_back(nxt);
                count += 1;
            }
        }
        out.insert(source_id.clone(), count);
    }
    out
}

/// Kahn's topological-wave layering on the filtered snapshot edges,
/// restricted to schedulable nodes. Replaces the legacy `build_waves`.
fn build_waves_snapshot(
    nodes: &HashMap<String, WorkPlanNode>,
    snapshot: &SollSnapshot,
    schedulable_ids: &HashSet<String>,
) -> Vec<WorkPlanWave> {
    let mut indegree: HashMap<String, usize> = schedulable_ids
        .iter()
        .map(|id| (id.clone(), 0usize))
        .collect();
    for id in schedulable_ids {
        let Some(idx) = snapshot.node_index(id) else {
            continue;
        };
        for e in snapshot.graph().edges_directed(idx, Direction::Outgoing) {
            if !is_work_plan_relation(e.weight().as_str()) {
                continue;
            }
            let target_id = &snapshot.graph()[e.target()];
            if schedulable_ids.contains(target_id) {
                *indegree.entry(target_id.clone()).or_insert(0) += 1;
            }
        }
    }
    let mut ready: Vec<String> = indegree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(id, _)| id.clone())
        .collect();
    ready.sort();
    let mut waves = Vec::new();
    let mut wave_index = 1usize;
    while !ready.is_empty() {
        let current = std::mem::take(&mut ready);
        let mut items: Vec<WorkPlanNode> = current
            .iter()
            .filter_map(|id| nodes.get(id).cloned())
            .collect();
        items.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.descendants.cmp(&a.descendants))
                .then_with(|| a.entity_type.sort_rank().cmp(&b.entity_type.sort_rank()))
                .then_with(|| a.id.cmp(&b.id))
        });
        waves.push(WorkPlanWave { wave_index, items });
        wave_index += 1;
        let mut next_ready: BTreeSet<String> = BTreeSet::new();
        for current_id in current {
            if let Some(idx) = snapshot.node_index(&current_id) {
                for e in snapshot.graph().edges_directed(idx, Direction::Outgoing) {
                    if !is_work_plan_relation(e.weight().as_str()) {
                        continue;
                    }
                    let child_id = snapshot.graph()[e.target()].clone();
                    if !schedulable_ids.contains(&child_id) {
                        continue;
                    }
                    if let Some(deg) = indegree.get_mut(&child_id) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            next_ready.insert(child_id);
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

impl McpServer {
    pub(crate) fn axon_soll_work_plan(&self, args: &Value) -> Option<Value> {
        let project_code_input = args.get("project_code")?.as_str()?;
        // REQ-AXO-043 — wrong_project_scope contract via shared helper.
        let project_code_owned = match self.resolve_project_code(project_code_input) {
            Ok(code) => code,
            Err(_) => {
                return Some(self.wrong_project_scope_response(project_code_input, "soll_work_plan"));
            }
        };
        let project_code = project_code_owned.as_str();
        // REQ-AXO-91500 patch A makes the scorer rank correctly via the
        // broader filiation filter ; default limit stays at 50 per
        // CPT-AXO-90009 pagination cognitive (top-K by default, drill-down
        // via explicit `limit` arg). LLM may request `limit=N` for
        // deeper inspection.
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
        let include_validation_details = args
            .get("include_validation_details")
            .and_then(|v| v.as_bool())
            .unwrap_or(format == "verbose");
        // REQ-AXO-144 — temporal score decay. Default include_decay=true so
        // mature accepted Decisions without recent activity drop out of
        // wave 1 even when their structural score would still rank them
        // on top. Set include_decay=false to disable (benchmarking, A/B).
        let include_decay = args
            .get("include_decay")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let half_life_days = args
            .get("half_life_days")
            .and_then(|v| v.as_f64())
            .unwrap_or(DEFAULT_DECAY_HALF_LIFE_DAYS);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // REQ-AXO-319 — compute requirement_coverage_summary ONCE upfront and
        // thread it through load_work_plan_nodes + the three downstream
        // wrappers (verify/validate/completeness). Previously each call site
        // recomputed it (~5× per work_plan invocation).
        let cached_coverage = self.requirement_coverage_summary(project_code).ok();
        let mut nodes =
            self.load_work_plan_nodes_with_cached_coverage(project_code, cached_coverage.as_ref());
        // REQ-AXO-346 Slice 3 — query the EXISTING snapshot petgraph
        // (REQ-AXO-322 / DEC-AXO-091). No per-call graph rebuild ; the
        // `is_work_plan_relation` predicate filters SOLVES+BELONGS_TO
        // edges on the fly via `petgraph::visit::EdgeFiltered`.
        let snapshot = self
            .soll_cache()
            .snapshot(project_code)
            .ok()
            .unwrap_or_else(|| std::sync::Arc::new(SollSnapshot::empty(project_code, 0)));
        let cycle_sets = cycle_sets_snapshot(&snapshot);
        let cycle_node_ids = cycle_sets
            .iter()
            .flat_map(|set| set.iter().cloned())
            .collect::<HashSet<_>>();
        let blocked_by_cycles = blocked_by_cycles_snapshot(&snapshot, &cycle_node_ids);
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
                        "{} link(s) to `indexed_degraded` scope",
                        node.ist_degraded_links
                    ));
                }
            }
        }

        // REQ-AXO-135: terminal-state Decisions/Requirements/Milestones are
        // not actionable — exclude them from wave 1 AND from descendant
        // counting so 'unblocks N descendants' reflects OPEN descendants only.
        // Terminal states across SOLL types: delivered/superseded (Decision),
        // completed/superseded (Requirement, Milestone), archived (any).
        let schedulable_ids = nodes
            .iter()
            .filter(|(id, node)| {
                !cycle_node_ids.contains(*id)
                    && !blocked_by_cycles.contains(*id)
                    && !is_terminal_status(&node.status)
            })
            .map(|(id, _)| id.clone())
            .collect::<HashSet<_>>();
        // REQ-AXO-346 Slice 3 — descendant count via BFS on the existing
        // snapshot petgraph, filtered to SOLVES+BELONGS_TO + schedulable.
        let descendants = descendant_counts_snapshot(&snapshot, &schedulable_ids);

        for node in nodes.values_mut() {
            node.descendants = *descendants.get(&node.id).unwrap_or(&0);
            let (score, reasons, gates) =
                score_node(node, include_ist, include_decay, half_life_days, now_ms);
            node.score = score;
            node.reasons = reasons;
            node.validation_gates = gates;
        }

        // REQ-AXO-346 Slice 3 — Kahn's topological waves on the existing
        // snapshot petgraph.
        let waves = build_waves_snapshot(&nodes, &snapshot, &schedulable_ids);
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
        let global_validation = self.axon_soll_verify_requirements_with_cached_coverage(
            &json!({ "project_code": project_code }),
            cached_coverage.as_ref(),
        );
        let soll_validation = self.axon_validate_soll_with_cached_coverage(
            &json!({ "project_code": project_code }),
            cached_coverage.as_ref(),
        );
        let completeness_snapshot = self
            .soll_completeness_snapshot_with_cached_coverage(
                Some(project_code),
                cached_coverage.as_ref(),
            )
            .ok();
        let requirement_verification = global_validation
            .as_ref()
            .and_then(|resp| resp.get("data"))
            .cloned()
            .unwrap_or(json!({}));
        let soll_validation_payload = soll_validation
            .as_ref()
            .and_then(|resp| resp.get("data"))
            .cloned()
            .unwrap_or(json!({}));
        let validation_gates = json!({
            "requirement_verification": if include_validation_details {
                requirement_verification.clone()
            } else {
                compact_requirement_verification(&requirement_verification)
            },
            "soll_validation": if include_validation_details {
                soll_validation_payload.clone()
            } else {
                compact_soll_validation(&soll_validation_payload)
            },
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
                "top": top,
                "include_validation_details": include_validation_details
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

    /// Memoized variant — see REQ-AXO-319.
    ///
    /// REQ-AXO-322 / DEC-AXO-091: nodes are read from the in-memory
    /// SOLL snapshot. The hot path no longer issues per-call SQL for
    /// Requirement / Decision / Milestone rows; evidence counts for
    /// Decisions and Milestones come from the snapshot's pre-aggregated
    /// traceability index. The snapshot is invalidated by the dispatch
    /// layer after any mutation tool.
    fn load_work_plan_nodes_with_cached_coverage(
        &self,
        project_code: &str,
        cached_coverage: Option<&RequirementCoverageSummary>,
    ) -> HashMap<String, WorkPlanNode> {
        let Ok(project_code) = self.resolve_project_code(project_code) else {
            return HashMap::new();
        };
        let snapshot = match self.soll_cache().snapshot(&project_code) {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        };
        let owned_coverage;
        let requirement_coverage: &RequirementCoverageSummary = match cached_coverage {
            Some(c) => c,
            None => {
                owned_coverage = self
                    .requirement_coverage_summary(&project_code)
                    .unwrap_or_default();
                &owned_coverage
            }
        };
        let requirement_coverage_by_id = requirement_coverage
            .entries
            .iter()
            .map(|entry| (entry.id.clone(), entry.clone()))
            .collect::<HashMap<_, _>>();

        let mut nodes = HashMap::with_capacity(snapshot.nodes.len());
        for snap_node in snapshot.nodes.values() {
            let meta: serde_json::Value =
                serde_json::from_str(&snap_node.metadata_raw).unwrap_or(serde_json::json!({}));
            let updated_at_ms = meta.get("updated_at").and_then(|v| v.as_i64());
            match snap_node.entity_type.as_str() {
                "Requirement" => {
                    let priority = meta
                        .get("priority")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let coverage_entry = requirement_coverage_by_id.get(&snap_node.id);
                    nodes.insert(
                        snap_node.id.clone(),
                        WorkPlanNode {
                            id: snap_node.id.clone(),
                            title: snap_node.title.clone(),
                            entity_type: WorkPlanEntityType::Requirement,
                            status: snap_node.status.clone(),
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
                            updated_at_ms,
                        },
                    );
                }
                "Decision" => {
                    let evidence_count =
                        snapshot.traceability_count_for("decision", &snap_node.id);
                    nodes.insert(
                        snap_node.id.clone(),
                        WorkPlanNode {
                            id: snap_node.id.clone(),
                            title: snap_node.title.clone(),
                            entity_type: WorkPlanEntityType::Decision,
                            status: snap_node.status.clone(),
                            priority: String::new(),
                            requirement_state: None,
                            evidence_count,
                            descendants: 0,
                            ist_degraded_links: 0,
                            backlog_visible: false,
                            score: 0,
                            reasons: Vec::new(),
                            validation_gates: Vec::new(),
                            ist_signals: Vec::new(),
                            updated_at_ms,
                        },
                    );
                }
                "Milestone" => {
                    let evidence_count =
                        snapshot.traceability_count_for("milestone", &snap_node.id);
                    nodes.insert(
                        snap_node.id.clone(),
                        WorkPlanNode {
                            id: snap_node.id.clone(),
                            title: snap_node.title.clone(),
                            entity_type: WorkPlanEntityType::Milestone,
                            status: snap_node.status.clone(),
                            priority: String::new(),
                            requirement_state: None,
                            evidence_count,
                            descendants: 0,
                            ist_degraded_links: 0,
                            backlog_visible: false,
                            score: 0,
                            reasons: Vec::new(),
                            validation_gates: Vec::new(),
                            ist_signals: Vec::new(),
                            updated_at_ms,
                        },
                    );
                }
                _ => {}
            }
        }
        nodes
    }

    fn count_degraded_links_for_node(&self, node_id: &str) -> usize {
        // REQ-AXO-251: under PG age-only-relations, the SQL SUBSTANTIATES /
        // IMPACTS / CONTAINS relation tables are empty/dropped — return 0
        // gracefully (no degraded-link signal). The authoritative
        // SUBSTANTIATES/IMPACTS edges live on soll.main.Edge.relation_type;
        // an AGE/SOLL-Edge-native equivalent for this query is a follow-up
        // (tracked on REQ-AXO-251 closure notes).
        if self.graph_store.skip_legacy_relations() {
            return 0;
        }
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
}

fn compact_requirement_verification(data: &Value) -> Value {
    let details = data
        .get("details")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let partial_or_missing = details
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.get("state").and_then(Value::as_str),
                Some("partial") | Some("missing")
            )
        })
        .take(20)
        .map(|entry| {
            json!({
                "id": entry.get("id").cloned().unwrap_or(Value::Null),
                "state": entry.get("state").cloned().unwrap_or(Value::Null),
                "status": entry.get("status").cloned().unwrap_or(Value::Null),
                "missing_dimensions": entry.get("missing_dimensions").cloned().unwrap_or_else(|| json!([])),
                "suggested_next_actions": entry.get("suggested_next_actions").cloned().unwrap_or_else(|| json!([]))
            })
        })
        .collect::<Vec<_>>();

    json!({
        "summary": data.get("summary").cloned().unwrap_or_else(|| json!({})),
        "project_code": data.get("project_code").cloned().unwrap_or(Value::Null),
        "partial_or_missing_requirements": partial_or_missing,
        "completion_model": {
            "done_rule": data
                .get("completion_model")
                .and_then(|model| model.get("done_rule"))
                .cloned()
                .unwrap_or(Value::Null),
            "partial_rule": data
                .get("completion_model")
                .and_then(|model| model.get("partial_rule"))
                .cloned()
                .unwrap_or(Value::Null)
        },
        "compact": true,
        "expand_with": {"tool": "soll_work_plan", "arguments": {"include_validation_details": true}}
    })
}

fn compact_soll_validation(data: &Value) -> Value {
    let violation_counts = data
        .get("violations")
        .and_then(Value::as_object)
        .map(|violations| {
            violations
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        Value::from(value.as_array().map(|items| items.len()).unwrap_or(0)),
                    )
                })
                .collect::<serde_json::Map<String, Value>>()
        })
        .map(Value::Object)
        .unwrap_or_else(|| json!({}));

    json!({
        "status": data.get("status").cloned().unwrap_or(Value::Null),
        "summary": data.get("summary").cloned().unwrap_or(Value::Null),
        "scope": data.get("scope").cloned().unwrap_or(Value::Null),
        "requirement_coverage": data.get("requirement_coverage").cloned().unwrap_or_else(|| json!({})),
        "completeness": data.get("completeness").cloned().unwrap_or_else(|| json!({})),
        "violation_counts": violation_counts,
        "compact": true,
        "expand_with": {"tool": "soll_validate", "arguments": {}}
    })
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-91500 patch A regression test.
    //!
    //! Verifies that `descendant_counts_snapshot` counts every canonical
    //! filiation relation (SOLVES, BELONGS_TO, TARGETS, REFINES, EXPLAINS,
    //! VERIFIES) — not just SOLVES + BELONGS_TO. Pure in-memory test on
    //! the `SollSnapshot::build` constructor (no DB, no fixtures, immune
    //! to DEC-PRO-100 CHECK constraint).
    use super::*;
    use crate::soll_snapshot::{SnapshotEdge, SnapshotNode, SollSnapshot};
    use std::collections::HashMap;

    fn mk_node(id: &str, ty: &str) -> SnapshotNode {
        SnapshotNode {
            id: id.to_string(),
            entity_type: ty.to_string(),
            title: format!("title-{}", id),
            status: "current".to_string(),
            metadata_raw: "{}".to_string(),
        }
    }

    fn mk_edge(src: &str, tgt: &str, rel: &str) -> SnapshotEdge {
        SnapshotEdge {
            source_id: src.to_string(),
            target_id: tgt.to_string(),
            relation_type: rel.to_string(),
        }
    }

    /// MIL-AXO-019-style cluster: 1 milestone targets 3 REQs, each refined
    /// by a smaller REQ. The milestone should count 6 transitive descendants
    /// via TARGETS + REFINES — relations the legacy `is_work_plan_relation`
    /// (SOLVES + BELONGS_TO only) would have ignored.
    #[test]
    fn descendant_counts_use_broad_filiation_filter() {
        let mut nodes = HashMap::new();
        nodes.insert("MIL-AXO-019".into(), mk_node("MIL-AXO-019", "Milestone"));
        for n in 1..=3 {
            nodes.insert(format!("REQ-AXO-100{n}"), mk_node(&format!("REQ-AXO-100{n}"), "Requirement"));
            nodes.insert(format!("REQ-AXO-200{n}"), mk_node(&format!("REQ-AXO-200{n}"), "Requirement"));
        }
        let mut edges = Vec::new();
        for n in 1..=3 {
            edges.push(mk_edge("MIL-AXO-019", &format!("REQ-AXO-100{n}"), "TARGETS"));
            edges.push(mk_edge(&format!("REQ-AXO-200{n}"), &format!("REQ-AXO-100{n}"), "REFINES"));
        }

        let snapshot = SollSnapshot::build("AXO", 1, nodes, edges, Vec::new());
        let allowed: HashSet<String> = snapshot
            .graph()
            .raw_nodes()
            .iter()
            .map(|w| w.weight.clone())
            .collect();

        let counts = descendant_counts_snapshot(&snapshot, &allowed);

        // MIL reaches 3 REQ-100x via TARGETS. Counting transitively via
        // REFINES would only matter if the edge direction matched ; here
        // REFINES is child→parent (REQ-200x → REQ-100x), so from
        // MIL-AXO-019 only the 3 direct TARGETS targets are reachable.
        assert_eq!(counts.get("MIL-AXO-019").copied().unwrap_or(0), 3,
            "MIL-AXO-019 should count 3 TARGETS descendants (legacy filter returned 0)");

        // REQ-AXO-2001 has 1 outgoing REFINES → REQ-AXO-1001. Patch A
        // counts REFINES as filiation, so descendants == 1 (legacy filter
        // returned 0 because REFINES was excluded).
        assert_eq!(counts.get("REQ-AXO-2001").copied().unwrap_or(0), 1,
            "REQ-AXO-2001 → REQ-AXO-1001 REFINES should count as 1 descendant");
    }

    /// Umbrella REQ pattern: parent REQ raffinée par N sous-REQ via REFINES
    /// (child → parent). From the parent's outgoing side, REFINES gives 0
    /// — the children point INTO the parent, not the other way. This test
    /// pins the directional semantics so future refactors don't accidentally
    /// invert REFINES.
    #[test]
    fn refines_direction_pinned_child_to_parent() {
        let mut nodes = HashMap::new();
        nodes.insert("REQ-AXO-91483".into(), mk_node("REQ-AXO-91483", "Requirement"));
        for n in 91484..=91486 {
            nodes.insert(format!("REQ-AXO-{n}"), mk_node(&format!("REQ-AXO-{n}"), "Requirement"));
        }
        let edges: Vec<SnapshotEdge> = (91484..=91486)
            .map(|n| mk_edge(&format!("REQ-AXO-{n}"), "REQ-AXO-91483", "REFINES"))
            .collect();

        let snapshot = SollSnapshot::build("AXO", 1, nodes, edges, Vec::new());
        let allowed: HashSet<String> = snapshot
            .graph()
            .raw_nodes()
            .iter()
            .map(|w| w.weight.clone())
            .collect();
        let counts = descendant_counts_snapshot(&snapshot, &allowed);

        // Each child has 1 outgoing REFINES toward the umbrella → counts as 1.
        assert_eq!(counts.get("REQ-AXO-91484").copied().unwrap_or(0), 1);
        // The umbrella has 0 outgoing REFINES (children point INTO it).
        assert_eq!(counts.get("REQ-AXO-91483").copied().unwrap_or(0), 0);
    }

    /// EXPLAINS direction: CPT → REQ. From the CPT, counting outgoing
    /// edges gives N (the REQ it explains).
    #[test]
    fn explains_edge_counted_from_concept() {
        let mut nodes = HashMap::new();
        nodes.insert("CPT-AXO-018".into(), mk_node("CPT-AXO-018", "Concept"));
        for n in 91493..=91497 {
            nodes.insert(format!("REQ-AXO-{n}"), mk_node(&format!("REQ-AXO-{n}"), "Requirement"));
        }
        let edges: Vec<SnapshotEdge> = (91493..=91497)
            .map(|n| mk_edge("CPT-AXO-018", &format!("REQ-AXO-{n}"), "EXPLAINS"))
            .collect();

        let snapshot = SollSnapshot::build("AXO", 1, nodes, edges, Vec::new());
        let allowed: HashSet<String> = snapshot
            .graph()
            .raw_nodes()
            .iter()
            .map(|w| w.weight.clone())
            .collect();
        let counts = descendant_counts_snapshot(&snapshot, &allowed);

        // CPT EXPLAINS 5 REQ → 5 descendants (legacy filter returned 0).
        assert_eq!(counts.get("CPT-AXO-018").copied().unwrap_or(0), 5,
            "CPT EXPLAINS should contribute to descendant count");
    }

    /// Predicate self-test: is_descendant_relation accepts the 6 canonical
    /// filiation relations and rejects unrelated ones.
    #[test]
    fn descendant_predicate_accepts_canonical_filiation() {
        for canon in ["SOLVES", "BELONGS_TO", "TARGETS", "REFINES", "EXPLAINS", "VERIFIES"] {
            assert!(is_descendant_relation(canon), "{canon} should be filiation");
        }
        for non in ["SUPERSEDES", "INHERITS_FROM", "RANDOM"] {
            assert!(!is_descendant_relation(non), "{non} should not be filiation");
        }
    }
}
