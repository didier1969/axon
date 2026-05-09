use super::planning_output::build_top_recommendations;
use super::*;

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
        let schedulable_adj = filter_adjacency(&adjacency, &schedulable_ids);
        let descendants = compute_descendant_counts(&schedulable_ids, &schedulable_adj);

        for node in nodes.values_mut() {
            node.descendants = *descendants.get(&node.id).unwrap_or(&0);
            let (score, reasons, gates) =
                score_node(node, include_ist, include_decay, half_life_days, now_ms);
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
        let decision_evidence_counts =
            self.load_work_plan_evidence_counts(&project_code, "DEC", "decision");
        let milestone_evidence_counts =
            self.load_work_plan_evidence_counts(&project_code, "MIL", "milestone");
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
                let updated_at_ms = meta.get("updated_at").and_then(|v| v.as_i64());
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
                        updated_at_ms,
                    },
                );
            }
        }

        let dec_query = format!(
            "SELECT id, title, COALESCE(status,''), COALESCE(metadata,'{{}}') FROM soll.Node WHERE type='Decision' AND id LIKE 'DEC-{}-%' ORDER BY id",
            escape_sql(&project_code)
        );
        if let Ok(raw) = self.graph_store.query_json(&dec_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() < 4 {
                    continue;
                }
                let id = row[0].clone();
                let evidence_count = decision_evidence_counts.get(&id).copied().unwrap_or(0);
                let meta: serde_json::Value =
                    serde_json::from_str(&row[3]).unwrap_or(serde_json::json!({}));
                let updated_at_ms = meta.get("updated_at").and_then(|v| v.as_i64());
                nodes.insert(
                    id.clone(),
                    WorkPlanNode {
                        id,
                        title: row[1].clone(),
                        entity_type: WorkPlanEntityType::Decision,
                        status: row[2].clone(),
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
        }

        let mil_query = format!(
            "SELECT id, title, COALESCE(status,''), COALESCE(metadata,'{{}}') FROM soll.Node WHERE type='Milestone' AND id LIKE 'MIL-{}-%' ORDER BY id",
            escape_sql(&project_code)
        );
        if let Ok(raw) = self.graph_store.query_json(&mil_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if row.len() < 4 {
                    continue;
                }
                let id = row[0].clone();
                let evidence_count = milestone_evidence_counts.get(&id).copied().unwrap_or(0);
                let meta: serde_json::Value =
                    serde_json::from_str(&row[3]).unwrap_or(serde_json::json!({}));
                let updated_at_ms = meta.get("updated_at").and_then(|v| v.as_i64());
                nodes.insert(
                    id.clone(),
                    WorkPlanNode {
                        id,
                        title: row[1].clone(),
                        entity_type: WorkPlanEntityType::Milestone,
                        status: row[2].clone(),
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
        }

        nodes
    }

    fn load_work_plan_evidence_counts(
        &self,
        project_code: &str,
        id_prefix: &str,
        entity_type: &str,
    ) -> HashMap<String, usize> {
        let query = format!(
            "SELECT soll_entity_id, COUNT(*) FROM soll.Traceability
             WHERE soll_entity_id LIKE '{}-{}-%'
               AND LOWER(COALESCE(soll_entity_type, '')) = '{}'
             GROUP BY soll_entity_id",
            escape_sql(id_prefix),
            escape_sql(project_code),
            escape_sql(entity_type)
        );
        let Ok(raw) = self.graph_store.query_json(&query) else {
            return HashMap::new();
        };
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|row| {
                if row.len() < 2 {
                    return None;
                }
                let count = row[1].parse::<usize>().ok()?;
                Some((row[0].clone(), count))
            })
            .collect()
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
        // REQ-AXO-251: under PG age-only-relations, the SQL SUBSTANTIATES /
        // IMPACTS / CONTAINS relation tables are empty/dropped — return 0
        // gracefully (no degraded-link signal). The authoritative
        // SUBSTANTIATES/IMPACTS edges live on soll.main.Edge.relation_type;
        // an AGE/SOLL-Edge-native equivalent for this query is a follow-up
        // (tracked on REQ-AXO-251 closure notes).
        if self.graph_store.skip_sql_relations() {
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
