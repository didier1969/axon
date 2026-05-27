use serde_json::{json, Value};
use std::collections::HashMap;

use super::format::{evidence_by_mode, format_standard_contract};
use super::tools_framework::ANOMALIES_CACHE_TTL_MS;
use super::tools_framework_anomaly_heuristics::{
    recommend_effort_and_risk, sequencing_dependencies_for_anomaly,
};
use super::tools_framework_support::{cache_read, cache_write};
use super::McpServer;

impl McpServer {
    pub(super) fn axon_anomalies_impl(&self, args: &Value) -> Option<Value> {
        let project = args
            .get("project")
            .and_then(|value| value.as_str())
            .unwrap_or("*");
        // REQ-AXO-043 — when caller passes an unregistered project_code,
        // the anomalies query silently returned all-zero counts and
        // Status: ok. The LLM had no way to know its input was wrong.
        // Mirror the wrong_project_scope contract (see soll_query_context
        // and soll_work_plan).
        // REQ-AXO-043 — wrong_project_scope contract via shared helper. The
        // anomalies tool also accepts `project="*"` (workspace), in which
        // case validation is skipped. The workspace fallback is exposed in
        // next_best_actions via the extras parameter.
        if project != "*" && self.resolve_project_code(project).is_err() {
            return Some(self.wrong_project_scope_response_with_extras(
                project,
                "anomalies",
                &["or omit `project` to scope to workspace:*"],
            ));
        }
        let mode = args.get("mode").and_then(|value| value.as_str());
        let brief_mode = mode.unwrap_or("brief") == "brief";
        let now_ms = Self::now_unix_ms();
        let cache_key = format!("{}::{}", project, mode.unwrap_or("brief"));
        if let Some(cached) = cache_read(
            Self::anomalies_cache(),
            &cache_key,
            now_ms,
            ANOMALIES_CACHE_TTL_MS,
        ) {
            return Some(cached);
        }

        let escaped_project = project.replace('\'', "''");
        let total_symbols = if project == "*" {
            self.graph_store
                .query_count("SELECT count(*) FROM Symbol WHERE kind IN ('function', 'method')")
                .unwrap_or(0)
        } else {
            self.graph_store
                .query_count(&format!(
                    "SELECT count(*) FROM Symbol WHERE project_code = '{}' AND kind IN ('function', 'method')",
                    escaped_project
                ))
                .unwrap_or(0)
        };

        // REQ-AXO-901595 — RAM-first analytics via IstGraphView when the
        // per-project CSR cache is warm. Each `view.*_candidates` returns
        // `Some(vec)` when the snapshot is warm (PIL-AXO-9002), else
        // `None` and we fall through to the canonical PG path. `project="*"`
        // (workspace-wide) bypasses RAM because the cache is per-project.
        let ram_view = if project != "*" {
            Some(crate::ist_snapshot::process_view())
        } else {
            None
        };
        let wrappers = ram_view
            .as_ref()
            .and_then(|view| view.wrapper_candidates(project, 20))
            .unwrap_or_else(|| {
                self.graph_store
                    .get_wrapper_candidates(project)
                    .unwrap_or_default()
            });
        let feature_envy = ram_view
            .as_ref()
            .and_then(|view| view.feature_envy_candidates(project, 20))
            .unwrap_or_else(|| {
                self.graph_store
                    .get_feature_envy_candidates(project)
                    .unwrap_or_default()
            });
        let detours = self
            .graph_store
            .get_detour_candidates(project)
            .unwrap_or_default();
        let abstraction_detours = self
            .graph_store
            .get_abstraction_detour_candidates(project)
            .unwrap_or_default();
        // REQ-AXO-901599 / DEC-AXO-901593 (option B) — RAM-first orphan_code
        // with lazy PG Traceability crosswalk. RAM scans IstGraph CSR in
        // O(N+M) for structural candidates (no callers + non-public +
        // non-test) without SOLL awareness. We then apply a batch
        // `filter_orphans_by_traceability` PG query on JUST those
        // candidates (small list, IN-clause), filtering out symbols
        // already referenced by soll.Traceability. Cost : 1 small PG
        // SELECT vs the full Symbol+Edge+Traceability scan in the
        // legacy `get_orphan_code_symbols` path. Cold cache OR
        // workspace-wide (project='*') falls back to canonical PG.
        let ram_candidates: Option<Vec<String>> = ram_view
            .as_ref()
            .filter(|_| project != "*")
            .and_then(|view| view.orphan_code_symbols(project, 20));
        let orphan_code = match ram_candidates {
            Some(candidates) if candidates.is_empty() => Vec::new(),
            Some(candidates) => self
                .graph_store
                .filter_orphans_by_traceability(project, &candidates)
                .unwrap_or_else(|_| {
                    // PG filter failed — fall back to canonical full scan
                    self.graph_store
                        .get_orphan_code_symbols(project)
                        .unwrap_or_default()
                }),
            None => self
                .graph_store
                .get_orphan_code_symbols(project)
                .unwrap_or_default(),
        };
        let orphan_intent = self
            .graph_store
            .get_orphan_intent_nodes(project)
            .unwrap_or_default();
        let phantom_dead_refs = self
            .graph_store
            .get_phantom_dead_refs(project)
            .unwrap_or_default();
        let phantom_multi_declare = self
            .graph_store
            .get_phantom_multi_declare(project)
            .unwrap_or_default();
        let soll_snapshot = self
            .soll_completeness_snapshot(if project == "*" { None } else { Some(project) })
            .ok();
        let canonical_orphan_intent_ids = soll_snapshot
            .as_ref()
            .map(|snapshot| snapshot.canonical_orphan_intent_ids())
            .unwrap_or_default();
        // REQ-AXO-901595 — RAM-first reciprocal-CALLS cycle count via
        // IstGraphView (PIL-AXO-9002). When the per-project CSR cache is
        // warm, `view.reciprocal_calls_cycle_count` walks the in-memory
        // graph in O(N+M) without a PG roundtrip ; falls back to the
        // existing `get_circular_dependency_count_fast` SQL when cold OR
        // when `project == "*"` (the RAM walk is per-project scoped).
        let ram_cycle_count: Option<usize> = if project != "*" {
            let view = crate::ist_snapshot::process_view();
            if view.is_warm(project) {
                view.reciprocal_calls_cycle_count(project)
            } else {
                None
            }
        } else {
            None
        };
        let (circular_deps, cycle_count) = if brief_mode {
            let count = ram_cycle_count.unwrap_or_else(|| {
                self.graph_store
                    .get_circular_dependency_count_fast(project)
                    .unwrap_or(0) as usize
            });
            (Vec::new(), count)
        } else {
            let cycles = self
                .graph_store
                .get_circular_dependencies(project)
                .unwrap_or_default();
            let cycle_count = cycles.len();
            (cycles, cycle_count)
        };
        // REQ-AXO-901595 — RAM-first god-objects via IstGraphView. Same
        // warm-cache contract as the analytics above.
        let god_objects = ram_view
            .as_ref()
            .and_then(|view| view.god_objects(project))
            .map(|pairs| {
                pairs
                    .into_iter()
                    .map(|(name, count)| {
                        (name, serde_json::Value::Number((count as i64).into()))
                    })
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            })
            .unwrap_or_else(|| {
                self.graph_store
                    .get_god_objects(project)
                    .unwrap_or_default()
            });
        let validation_coverage_score = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let total_intent_nodes = if project == "*" {
            self.graph_store
                .query_count(
                    "SELECT count(*) FROM soll.Node WHERE type IN ('Requirement', 'Decision', 'Concept', 'Validation')",
                )
                .unwrap_or(0)
        } else {
            self.graph_store
                .query_count(&format!(
                    "SELECT count(*) FROM soll.Node WHERE project_code = '{}' AND type IN ('Requirement', 'Decision', 'Concept', 'Validation')",
                    escaped_project
                ))
                .unwrap_or(0)
        };

        let wrapper_entities = wrappers
            .iter()
            .take(if brief_mode { 5 } else { wrappers.len() })
            .cloned()
            .collect::<Vec<_>>();
        let feature_envy_entities = feature_envy
            .iter()
            .take(if brief_mode { 5 } else { feature_envy.len() })
            .cloned()
            .collect::<Vec<_>>();
        let detour_entities = detours
            .iter()
            .take(if brief_mode { 5 } else { detours.len() })
            .cloned()
            .collect::<Vec<_>>();
        let abstraction_detour_entities = abstraction_detours
            .iter()
            .take(if brief_mode {
                5
            } else {
                abstraction_detours.len()
            })
            .cloned()
            .collect::<Vec<_>>();
        let orphan_code_entities = orphan_code
            .iter()
            .take(if brief_mode { 8 } else { orphan_code.len() })
            .cloned()
            .collect::<Vec<_>>();
        let orphan_intent_entities = orphan_intent
            .iter()
            .take(if brief_mode { 8 } else { orphan_intent.len() })
            .cloned()
            .collect::<Vec<_>>();
        let god_object_entities = god_objects
            .keys()
            .take(if brief_mode { 3 } else { 5 })
            .cloned()
            .collect::<Vec<_>>();

        let default_symbol_validation = if brief_mode {
            json!({
                "tested": false,
                "traceability_links": 0,
                "mode": "brief_heuristic"
            })
        } else {
            json!({"tested": false, "traceability_links": 0})
        };
        let default_intent_validation = if brief_mode {
            json!({
                "traceability_links": 0,
                "verifies_edges": 0,
                "validation_nodes": 0,
                "mode": "brief_heuristic"
            })
        } else {
            json!({
                "traceability_links": 0,
                "verifies_edges": 0,
                "validation_nodes": 0
            })
        };

        let symbol_validation_map = if brief_mode {
            HashMap::new()
        } else {
            let mut symbol_signal_names = Vec::new();
            for item in wrapper_entities
                .iter()
                .chain(feature_envy_entities.iter())
                .chain(detour_entities.iter())
                .chain(abstraction_detour_entities.iter())
            {
                symbol_signal_names.push(item.split(" -> ").next().unwrap_or(item).to_string());
            }
            symbol_signal_names.extend(orphan_code_entities.iter().cloned());
            symbol_signal_names.extend(god_object_entities.iter().cloned());
            symbol_signal_names.sort();
            symbol_signal_names.dedup();
            self.batch_symbol_validation_signals(project, &symbol_signal_names)
        };

        let intent_validation_map = if brief_mode {
            HashMap::new()
        } else {
            let intent_ids = orphan_intent_entities
                .iter()
                .map(|node| node.split(' ').next().unwrap_or(node).to_string())
                .collect::<Vec<_>>();
            self.batch_intent_validation_signals(project, &intent_ids)
        };

        let mut findings = Vec::new();
        for wrapper in &wrapper_entities {
            let source_symbol = wrapper.split(" -> ").next().unwrap_or(wrapper);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("wrapper", &validation_signals);
            findings.push(json!({
                "type": "wrapper",
                "entity": wrapper,
                "scope": project,
                "severity": "medium",
                "confidence": "medium",
                "provenance": "heuristic_single_outbound_call",
                "evidence_sources": ["CALLS", "Symbol", "CONTAINS"],
                "recommended_action": "inspect for direct inlining or removal",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": validation_signals.get("tested").and_then(|value| value.as_bool()).unwrap_or(false),
                "needs_human_confirmation": !validation_signals.get("tested").and_then(|value| value.as_bool()).unwrap_or(false)
            }));
        }
        for candidate in &feature_envy_entities {
            let source_symbol = candidate.split(" -> ").next().unwrap_or(candidate);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("feature_envy", &validation_signals);
            findings.push(json!({
                "type": "feature_envy",
                "entity": candidate,
                "scope": project,
                "severity": "medium",
                "confidence": "medium",
                "provenance": "cross_file_outbound_dominance",
                "evidence_sources": ["CALLS", "CONTAINS"],
                "recommended_action": "review module placement and move logic closer to its dominant collaborators",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for candidate in &detour_entities {
            let source_symbol = candidate.split(" -> ").next().unwrap_or(candidate);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("detour", &validation_signals);
            findings.push(json!({
                "type": "detour",
                "entity": candidate,
                "scope": project,
                "severity": "medium",
                "confidence": "medium",
                "provenance": "single_inbound_single_outbound_bridge",
                "evidence_sources": ["CALLS", "CONTAINS"],
                "recommended_action": "inspect whether the intermediate hop can be inlined or collapsed",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for candidate in &abstraction_detour_entities {
            let source_symbol = candidate.split(" -> ").next().unwrap_or(candidate);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("abstraction_detour", &validation_signals);
            findings.push(json!({
                "type": "abstraction_detour",
                "entity": candidate,
                "scope": project,
                "severity": "medium",
                "confidence": "low",
                "provenance": "single_local_interface_implementation_name_match",
                "evidence_sources": ["Symbol", "CONTAINS"],
                "recommended_action": "confirm whether the interface still provides policy value or only indirection",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for symbol in &orphan_code_entities {
            let validation_signals = symbol_validation_map
                .get(symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("orphan_code", &validation_signals);
            findings.push(json!({
                "type": "orphan_code",
                "entity": symbol,
                "scope": project,
                "severity": "high",
                "confidence": "medium",
                "provenance": "missing_traceability_links",
                "evidence_sources": ["Symbol", "soll.Traceability"],
                "recommended_action": "link to intent or delete if obsolete",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        let mut heuristic_intent_gap_count = 0usize;
        for node in &orphan_intent_entities {
            let node_id = node.split(' ').next().unwrap_or(node);
            let validation_signals = intent_validation_map
                .get(node_id)
                .cloned()
                .unwrap_or_else(|| default_intent_validation.clone());
            let canonical_backed = canonical_orphan_intent_ids.contains(node_id);
            let anomaly_type = if canonical_backed {
                "orphan_intent"
            } else {
                heuristic_intent_gap_count += 1;
                "heuristic_intent_gap"
            };
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("orphan_intent", &validation_signals);
            let mut enriched_validation_signals = validation_signals;
            enriched_validation_signals["canonical_backed"] = json!(canonical_backed);
            findings.push(json!({
                "type": anomaly_type,
                "entity": node,
                "scope": project,
                "severity": if canonical_backed { "high" } else { "low" },
                "confidence": if canonical_backed { "medium" } else { "low" },
                "provenance": if canonical_backed { "missing_traceability_evidence" } else { "heuristic_missing_traceability" },
                "evidence_sources": ["soll.Node", "soll.Traceability", "soll.Edge"],
                "recommended_action": if canonical_backed {
                    "attach implementation or validation evidence"
                } else {
                    "review only if this node should carry direct proof at the current project stage"
                },
                "validation_signals": enriched_validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": !canonical_backed,
                "needs_human_confirmation": true
            }));
        }
        for phantom_ref in phantom_dead_refs.iter().take(if brief_mode { 5 } else { 10 }) {
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("phantom_dead_ref", &json!({}));
            findings.push(json!({
                "type": "phantom_dead_ref",
                "entity": phantom_ref,
                "scope": project,
                "severity": "medium",
                "confidence": "high",
                "provenance": "phantom_reads_without_declares",
                "evidence_sources": ["Edge(READS)", "Edge(DECLARES)"],
                "recommended_action": "add a DECLARES source or remove the read",
                "validation_signals": json!({}),
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for phantom_conflict in phantom_multi_declare.iter().take(if brief_mode { 5 } else { 10 }) {
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("phantom_multi_declare", &json!({}));
            findings.push(json!({
                "type": "phantom_multi_declare",
                "entity": phantom_conflict,
                "scope": project,
                "severity": "low",
                "confidence": "medium",
                "provenance": "phantom_declared_in_multiple_sources",
                "evidence_sources": ["Edge(DECLARES)"],
                "recommended_action": "review for intentional redundancy or consolidate",
                "validation_signals": json!({}),
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for cycle in circular_deps.iter().take(if brief_mode { 3 } else { 5 }) {
            let validation_signals = json!({
                "tested": Value::Null,
                "traceability_links": 0,
                "verifies_edges": 0
            });
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("cycle", &validation_signals);
            findings.push(json!({
                "type": "cycle",
                "entity": cycle,
                "scope": project,
                "severity": "high",
                "confidence": "high",
                "provenance": "recursive_call_path",
                "evidence_sources": ["CALLS"],
                "recommended_action": "review for justified or accidental recursion",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for name in &god_object_entities {
            let count = god_objects
                .get(name)
                .and_then(|value: &Value| value.as_i64())
                .unwrap_or(0);
            let validation_signals = symbol_validation_map
                .get(name)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                recommend_effort_and_risk("god_object", &validation_signals);
            findings.push(json!({
                "type": "god_object",
                "entity": name,
                "scope": project,
                "severity": "medium",
                "confidence": "high",
                "provenance": "fan_in_threshold",
                "evidence_sources": ["CALLS"],
                "recommended_action": format!("review decomposition candidate (fan_in={})", count),
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        let orphan_code_rate = if total_symbols > 0 {
            ((orphan_code.len() as f64 / total_symbols as f64) * 100.0 * 10.0).round() / 10.0
        } else {
            0.0
        };
        let alignment_proxy_score = if total_symbols > 0 {
            (((total_symbols.saturating_sub(orphan_code.len() as i64)) as f64
                / total_symbols as f64)
                * 100.0
                * 10.0)
                .round()
                / 10.0
        } else {
            0.0
        };
        let rectitude_proxy_score = if total_symbols > 0 {
            let detour_like = wrappers.len() + detours.len();
            (((total_symbols.saturating_sub(detour_like as i64)) as f64 / total_symbols as f64)
                * 100.0
                * 10.0)
                .round()
                / 10.0
        } else {
            0.0
        };
        let cycle_health_score = if total_symbols > 0 {
            (((total_symbols.saturating_sub(cycle_count as i64)) as f64 / total_symbols as f64)
                * 100.0
                * 10.0)
                .round()
                / 10.0
        } else {
            100.0
        };
        let canonical_orphan_intent_count = orphan_intent_entities
            .iter()
            .filter(|node| {
                let node_id = node.split(' ').next().unwrap_or(node);
                canonical_orphan_intent_ids.contains(node_id)
            })
            .count();
        let orphan_intent_rate = if total_intent_nodes > 0 {
            ((canonical_orphan_intent_count as f64 / total_intent_nodes as f64) * 100.0 * 10.0)
                .round()
                / 10.0
        } else {
            0.0
        };

        let mut recommendations = findings
            .iter()
            .map(|finding| {
                let anomaly_type = finding
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let entity = finding
                    .get("entity")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let severity = finding
                    .get("severity")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let recommended_action = finding
                    .get("recommended_action")
                    .and_then(|value| value.as_str())
                    .unwrap_or("review manually");
                let estimated_effort = finding
                    .get("estimated_effort")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let estimated_risk = finding
                    .get("estimated_risk")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let validation_signals = finding
                    .get("validation_signals")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let sequencing_dependencies = sequencing_dependencies_for_anomaly(anomaly_type);
                json!({
                    "anomaly_type": anomaly_type,
                    "entity": entity,
                    "severity": severity,
                    "why_flagged": finding.get("provenance").cloned().unwrap_or_else(|| json!("unknown")),
                    "recommended_action": recommended_action,
                    "estimated_effort": estimated_effort,
                    "estimated_risk": estimated_risk,
                    "validation_signals": validation_signals,
                    "sequencing_dependencies": sequencing_dependencies,
                    "safe_to_act": finding.get("safe_to_act").cloned().unwrap_or_else(|| json!(false)),
                    "needs_human_confirmation": finding.get("needs_human_confirmation").cloned().unwrap_or_else(|| json!(true))
                })
            })
            .collect::<Vec<_>>();
        recommendations.sort_by_key(|item| {
            match item
                .get("severity")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown")
            {
                "high" => 0,
                "medium" => 1,
                "low" => 2,
                _ => 3,
            }
        });
        if brief_mode {
            recommendations.truncate(12);
        }

        let evidence = format!(
            "**Scope:** `{}`\n\
**Wrappers:** {}\n\
**Feature envy:** {}\n\
**Detours:** {}\n\
**Abstraction detours:** {}\n\
**Orphan code:** {}\n\
**Orphan intent (canonical):** {}\n\
**Heuristic intent gaps:** {}\n\
**Phantom dead refs:** {}\n\
**Phantom multi-declare:** {}\n\
**Cycles:** {}\n\
**God objects:** {}\n",
            project,
            wrappers.len(),
            feature_envy.len(),
            detours.len(),
            abstraction_detours.len(),
            orphan_code.len(),
            canonical_orphan_intent_count,
            heuristic_intent_gap_count,
            phantom_dead_refs.len(),
            phantom_multi_declare.len(),
            cycle_count,
            god_objects.len()
        );
        let report = format!(
            "## 🚨 Axon Anomalies\n\n{}",
            format_standard_contract(
                "ok",
                "structural anomalies aggregated",
                &format!("project:{}", project),
                &evidence_by_mode(&evidence, mode),
                &[
                    "review top orphan intent and orphan code first",
                    "inspect wrapper candidates before broad refactors",
                    "use `impact` on any high-risk symbol before mutation"
                ],
                "medium",
            )
        );

        // REQ-AXO-91517 (MIL-AXO-019 vague 1c follow-up) — cognitive
        // signals on the in-memory IST snapshot : bridges (single-edge
        // failure points), articulation points (single-node failure
        // points), and structural SCCs (> 1 node mutual recursion
        // clusters). All three run in O(N+M) on the IstGraph CSR
        // (`ist_snapshot::algorithms`) without any PG roundtrip ;
        // empty result when the per-project cache is cold or
        // `project == "*"` (the algos are per-project scoped).
        let mut surfaces_used: Vec<&'static str> = vec!["graph_pg"];
        if ram_cycle_count.is_some() {
            // REQ-AXO-901595 — declare the RAM cycle-count surface for
            // qualify-mcp telemetry and post-hoc PIL-AXO-9002 audit.
            surfaces_used.push("graph_ram_cycles");
        }
        let cognitive_signals: Value = if project != "*" {
            let view = crate::ist_snapshot::process_view();
            if view.is_warm(project) {
                surfaces_used.push("graph_ram_cognitive");
                let cache = view.cache_handle();
                if let Some(snap) = cache.get(project) {
                    let (bridges_raw, ap_raw) =
                        crate::ist_snapshot::algorithms::bridges_and_articulation(&snap);
                    let sccs_raw = crate::ist_snapshot::algorithms::structural_sccs(&snap);
                    let bridge_cap = if brief_mode { 10 } else { 50 };
                    let scc_cap = if brief_mode { 5 } else { 20 };
                    let ap_cap = if brief_mode { 10 } else { 50 };
                    let bridges: Vec<Value> = bridges_raw
                        .into_iter()
                        .take(bridge_cap)
                        .map(|(a, b)| json!({"source": a, "target": b}))
                        .collect();
                    let articulation_points: Vec<Value> =
                        ap_raw.into_iter().take(ap_cap).map(Value::from).collect();
                    let strongly_connected: Vec<Value> = sccs_raw
                        .into_iter()
                        .take(scc_cap)
                        .map(|members| json!({"size": members.len(), "members": members}))
                        .collect();
                    json!({
                        "bridges": bridges,
                        "articulation_points": articulation_points,
                        "strongly_connected_components": strongly_connected,
                        "algo_provenance": "petgraph_tarjan_lowlink",
                    })
                } else {
                    json!({})
                }
            } else {
                json!({})
            }
        } else {
            json!({})
        };

        // REQ-AXO-901755 — SRS slice 5: residual_legacy anomaly category.
        let residual_legacy: Vec<serde_json::Value> = self
            .soll_cache()
            .snapshot(project)
            .ok()
            .map(|snap| {
                super::tools_srs::detect_all_superseded_proximity(&snap)
                    .into_iter()
                    .filter(|n| {
                        n.strategy == super::tools_srs::LegacyStrategy::ProgressiveActive
                            || n.strategy == super::tools_srs::LegacyStrategy::Abandoned
                    })
                    .map(|n| {
                        let residual = snap
                            .nodes
                            .get(&n.id)
                            .map(|node| {
                                snap.traceability_count_for(
                                    &node.entity_type.to_ascii_lowercase(),
                                    &n.id,
                                )
                            })
                            .unwrap_or(0);
                        json!({
                            "id": n.id,
                            "strategy": n.strategy,
                            "successor": n.successor,
                            "ist_residual_count": residual,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let response = json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "summary": {
                    "project": project,
                    "wrapper_count": wrappers.len(),
                    "feature_envy_count": feature_envy.len(),
                    "detour_count": detours.len(),
                    "abstraction_detour_count": abstraction_detours.len(),
                    "alignment_proxy_score": alignment_proxy_score,
                    "rectitude_proxy_score": rectitude_proxy_score,
                    "cycle_health_score": cycle_health_score,
                    "orphan_code_count": orphan_code.len(),
                    "orphan_code_rate": orphan_code_rate,
                    "orphan_intent_count": canonical_orphan_intent_count,
                    "orphan_intent_rate": orphan_intent_rate,
                    "heuristic_intent_gap_count": heuristic_intent_gap_count,
                    "cycle_count": cycle_count,
                    "god_object_count": god_objects.len(),
                    "phantom_dead_ref_count": phantom_dead_refs.len(),
                    "phantom_multi_declare_count": phantom_multi_declare.len(),
                    "validation_coverage_score": validation_coverage_score,
                    "total_symbols": total_symbols,
                    "total_intent_nodes": total_intent_nodes,
                    "residual_legacy_count": residual_legacy.len(),
                    "concept_completeness": soll_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.concept_complete())
                        .unwrap_or(false),
                    "implementation_completeness": soll_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.implementation_complete())
                        .unwrap_or(false)
                },
                "snapshot": {
                    "generated_at": Self::now_unix_ms(),
                    "provenance": "aggregated_graph_analytics",
                    "confidence": "medium",
                    "semantic_boundary": "heuristic anomaly overlays must not silently override canonical SOLL completeness"
                },
                "findings": findings,
                "residual_legacy": residual_legacy,
                "recommendations": recommendations,
                "cognitive_signals": cognitive_signals,
                "surfaces_used": surfaces_used,
            }
        });
        cache_write(Self::anomalies_cache(), cache_key, now_ms, &response);
        Some(response)
    }
}
