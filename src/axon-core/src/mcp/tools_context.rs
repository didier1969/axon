use crate::embedding_contract::{CHUNK_MODEL_ID, DIMENSION};
use crate::service_guard::ServicePressure;
use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
#[cfg(not(test))]
use std::sync::Mutex;
use std::time::Instant;

use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;

mod retrieval_model;
use retrieval_model::{
    ChunkCandidate, EntryCandidate, RetrievalDiagnostics, RetrievalRoute, RetrievalRuntimeState,
    RetrievalTimings,
};
#[cfg(not(test))]
use retrieval_model::{
    RetrieveContextCache, RETRIEVE_CONTEXT_CACHE, RETRIEVE_CONTEXT_CACHE_TTL_MS,
};

const DEFAULT_TOKEN_BUDGET: usize = 1400;
const DEFAULT_TOP_K: usize = 8;

impl McpServer {
    #[cfg(not(test))]
    fn retrieve_context_cache() -> &'static Mutex<RetrieveContextCache> {
        RETRIEVE_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    #[cfg(not(test))]
    fn read_retrieve_context_cache(key: &str, now_ms: i64) -> Option<Value> {
        let guard = Self::retrieve_context_cache().lock().ok()?;
        let (stored_at, value) = guard.get(key)?;
        if now_ms.saturating_sub(*stored_at) > RETRIEVE_CONTEXT_CACHE_TTL_MS {
            return None;
        }
        Some(value.clone())
    }

    #[cfg(test)]
    fn read_retrieve_context_cache(_key: &str, _now_ms: i64) -> Option<Value> {
        None
    }

    #[cfg(not(test))]
    fn write_retrieve_context_cache(key: String, now_ms: i64, value: &Value) {
        if let Ok(mut guard) = Self::retrieve_context_cache().lock() {
            guard.insert(key, (now_ms, value.clone()));
        }
    }

    #[cfg(test)]
    fn write_retrieve_context_cache(_key: String, _now_ms: i64, _value: &Value) {}

    pub(crate) fn resolve_scoped_symbol_id_canonical(
        &self,
        symbol: &str,
        project: Option<&str>,
    ) -> Option<String> {
        let query = if project.is_some() {
            format!(
                "SELECT id FROM Symbol \
                 WHERE (name = $sym OR id = $sym){project_filter} \
                 LIMIT 1",
                project_filter = Self::sql_project_filter_for_fields(project, &["project_code"])
            )
        } else {
            "SELECT id FROM Symbol WHERE name = $sym OR id = $sym LIMIT 1".to_string()
        };
        let params = json!({ "sym": symbol });
        let res = self.graph_store.query_json_param(&query, &params).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        rows.first()?
            .first()?
            .as_str()
            .map(|value| value.to_string())
    }

    pub(crate) fn suggest_scoped_symbols_canonical(
        &self,
        symbol: &str,
        project: Option<&str>,
        limit: usize,
    ) -> String {
        let needle = symbol.trim();
        if needle.is_empty() {
            return "[]".to_string();
        }
        let query = if project.is_some() {
            format!(
                "SELECT name, kind, COALESCE(project_code, 'unknown') \
                 FROM Symbol \
                 WHERE lower(name) LIKE lower($pat){project_filter} \
                 ORDER BY name \
                 LIMIT $limit",
                project_filter = Self::sql_project_filter_for_fields(project, &["project_code"])
            )
        } else {
            "SELECT name, kind, COALESCE(project_code, 'unknown') \
             FROM Symbol \
             WHERE lower(name) LIKE lower($pat) \
             ORDER BY name \
             LIMIT $limit"
                .to_string()
        };
        self.graph_store
            .query_json_param(
                &query,
                &json!({ "pat": format!("%{}%", needle), "limit": limit as u64 }),
            )
            .unwrap_or_else(|_| "[]".to_string())
    }

    pub(crate) fn axon_retrieve_context(&self, args: &Value) -> Option<Value> {
        let started_at = Instant::now();
        let question = args.get("question")?.as_str()?.trim();
        if question.is_empty() {
            // REQ-AXO-043 — bare error string was unactionable. Surface a
            // structured contract so the LLM client knows what to supply.
            return Some(json!({
                "content": [{"type": "text", "text": "retrieve_context requires a non-empty `question`. Pass a free-form question describing the target (symbol, file, behavior, or rationale you want context for)."}],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "missing_field": "question",
                    "next_action": "supply a non-empty `question` argument; example: question=\"how does the queue admission policy decide rejection?\"",
                    "operator_guidance": {
                        "problem_class": "input_invalid",
                        "likely_cause": "empty_or_whitespace_question",
                        "next_best_actions": [
                            "supply a non-empty `question` describing the target",
                            "alternatively, narrow to a specific symbol via `inspect` first",
                        ],
                        "follow_up_tools": ["inspect", "query"],
                        "confidence": "high",
                    },
                    "parameter_repair": {
                        "invalid_field": "question",
                        "follow_up_tools": ["inspect", "query"],
                        "hint": "supply a non-empty `question` describing the target (symbol, file, behavior, or rationale you want context for); example: \"how does the queue admission policy decide rejection?\""
                    }
                }
            }));
        }

        let mode = args.get("mode").and_then(|value| value.as_str());
        let prefer_project_intent = Self::prefer_project_intent(question, mode);
        // REQ-AXO-089 — when `project` is omitted, auto-resolve from
        // AXON_PROJECT_ROOT or cwd by matching against the registry. The
        // global CLAUDE.md promises "project_code is auto-resolved from
        // your working directory" but retrieve_context previously fell
        // through to workspace:* whenever the caller skipped the arg,
        // which made answers from inside a project directory look
        // workspace-wide. The auto-resolution only applies when exactly
        // one registered project matches the cwd; ambiguous matches
        // fall back to workspace:*.
        let explicit_project = args.get("project").and_then(|value| value.as_str());
        let auto_project = if explicit_project.is_none() {
            self.auto_resolve_project_code_str()
        } else {
            None
        };
        let project = explicit_project.or(auto_project.as_deref());
        let project_scope_variants = Self::project_scope_variants(project);
        let token_budget = args
            .get("token_budget")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_TOKEN_BUDGET)
            .max(300);
        let top_k = args
            .get("top_k")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_TOP_K)
            .clamp(3, 20);
        let include_graph = args
            .get("include_graph")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        let include_soll = args.get("include_soll").and_then(|value| value.as_bool());
        let should_include_soll = include_soll.unwrap_or(true);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let cache_key = format!(
            "{}|{}|{}|{}|{}|{}|{}",
            project.unwrap_or("*"),
            mode.unwrap_or("brief"),
            token_budget,
            top_k,
            include_graph,
            should_include_soll,
            question
        );
        if let Some(cached) = Self::read_retrieve_context_cache(&cache_key, now_ms) {
            return Some(cached);
        }

        let mut timings = RetrievalTimings::default();
        let stage_started_at = Instant::now();
        let route = Self::plan_retrieval_route(question);
        let terms = Self::question_terms(question);
        let terms_for_reasoning = if terms.is_empty() {
            vec![question.to_ascii_lowercase()]
        } else {
            terms.clone()
        };
        let path_hints = Self::question_path_hints(question);
        timings.planner_ms = stage_started_at.elapsed().as_millis() as u64;

        let mut excluded_because = Vec::new();
        if terms.is_empty() {
            excluded_because.push("planner_terms_empty_fell_back_to_full_question".to_string());
        }
        let rationale_requested = Self::has_rationale_language(question);

        let mut diagnostics = RetrievalDiagnostics::default();
        let stage_started_at = Instant::now();
        let mut entry_candidates = self.find_entry_candidates(
            project,
            &terms_for_reasoning,
            &path_hints,
            top_k * 4,
            &mut diagnostics,
        );
        self.rerank_entry_candidates(
            &mut entry_candidates,
            route,
            &terms_for_reasoning,
            &path_hints,
            &project_scope_variants,
            prefer_project_intent,
        );
        let entry_candidates = self.select_entry_candidates(&entry_candidates, top_k);
        timings.entry_lookup_ms = stage_started_at.elapsed().as_millis() as u64;
        let has_strong_anchor = entry_candidates
            .first()
            .map(Self::is_strong_anchor)
            .unwrap_or(false);

        let stage_started_at = Instant::now();
        let mut runtime = RetrievalRuntimeState::new(self);
        let semantic_allowed = runtime.allow_semantic_search(has_strong_anchor);
        if let Some(reason) = runtime.degraded_reason.clone() {
            excluded_because.push(reason);
        }

        let allow_unanchored_fallback =
            !(runtime.pressure == ServicePressure::Critical && !has_strong_anchor);
        if !allow_unanchored_fallback {
            excluded_because
                .push("unanchored_chunk_fallback_skipped_due_to_pressure_critical".to_string());
        }
        timings.runtime_guard_ms = stage_started_at.elapsed().as_millis() as u64;

        let stage_started_at = Instant::now();
        let mut chunk_candidates = if allow_unanchored_fallback {
            self.find_chunk_candidates(
                project,
                question,
                &terms_for_reasoning,
                &path_hints,
                &entry_candidates,
                route,
                top_k * 5,
                &mut excluded_because,
                semantic_allowed,
                &mut runtime,
            )
        } else {
            Vec::new()
        };
        // DEC-AXO-093 / REQ-AXO-324 — FTS modality (3rd trinity branch
        // alongside graph + vector). The GIN-indexed `content_tsv`
        // column has been live since MIL-AXO-017 slice 4 / REQ-AXO-292;
        // before this PR nothing in the MCP layer was actually
        // querying it. We append FTS hits to the candidate pool and
        // let the existing rerank step decide the final order. RRF
        // formal fusion across modality ranks is REQ-AXO-324 slice 2.
        if allow_unanchored_fallback {
            let fts_hits = self.find_chunk_candidates_via_fts(project, question, top_k * 5);
            if !fts_hits.is_empty() {
                let known: std::collections::HashSet<String> = chunk_candidates
                    .iter()
                    .map(|c| c.chunk_id.clone())
                    .collect();
                for hit in fts_hits {
                    if known.contains(&hit.chunk_id) {
                        continue;
                    }
                    chunk_candidates.push(hit);
                }
            }
        }
        diagnostics.chunk_candidates_considered = chunk_candidates.len();
        let has_direct_soll_traceability =
            self.has_direct_soll_traceability(&entry_candidates, project);
        let linked_evidence_first = rationale_requested && has_direct_soll_traceability;
        self.rerank_chunk_candidates(
            &mut chunk_candidates,
            route,
            &terms_for_reasoning,
            &entry_candidates,
            &project_scope_variants,
            prefer_project_intent,
            linked_evidence_first,
        );
        timings.chunk_lookup_ms = stage_started_at.elapsed().as_millis() as u64;

        let stage_started_at = Instant::now();
        let supporting_chunks = self.select_supporting_chunks(
            &chunk_candidates,
            &entry_candidates,
            route,
            top_k,
            token_budget,
            &mut excluded_because,
            &mut diagnostics,
        );
        timings.chunk_selection_ms = stage_started_at.elapsed().as_millis() as u64;

        let stage_started_at = Instant::now();
        let structural_neighbors = if include_graph
            && allow_unanchored_fallback
            && !runtime.should_skip_graph_expansion()
        {
            let neighbors = self.collect_structural_neighbors(&entry_candidates, route);
            diagnostics.graph_neighbors_selected = neighbors.len();
            neighbors
        } else if include_graph && runtime.should_skip_graph_expansion() {
            excluded_because.push("graph_expansion_skipped_due_to_pressure_guarded".to_string());
            Vec::new()
        } else if include_graph {
            excluded_because.push("graph_expansion_skipped_due_to_pressure_critical".to_string());
            Vec::new()
        } else {
            excluded_because.push("graph_expansion_disabled".to_string());
            Vec::new()
        };
        timings.graph_expansion_ms = stage_started_at.elapsed().as_millis() as u64;

        let should_join_soll = include_soll.unwrap_or_else(|| {
            has_direct_soll_traceability
                || (allow_unanchored_fallback
                    && (matches!(route, RetrievalRoute::SollHybrid) || rationale_requested))
        });
        let stage_started_at = Instant::now();
        let relevant_soll_entities = if should_join_soll
            && !runtime.should_skip_soll_join(route, rationale_requested)
        {
            let entities =
                self.collect_soll_entities(&entry_candidates, project, &terms_for_reasoning, top_k);
            diagnostics.soll_entities_selected = entities.len();
            entities
        } else if should_join_soll && runtime.should_skip_soll_join(route, rationale_requested) {
            excluded_because.push("soll_join_skipped_due_to_pressure_guarded".to_string());
            Vec::new()
        } else {
            excluded_because.push("soll_join_skipped_for_route".to_string());
            Vec::new()
        };
        timings.soll_join_ms = stage_started_at.elapsed().as_millis() as u64;

        let stage_started_at = Instant::now();
        let direct_evidence = self.build_direct_evidence(&entry_candidates);
        let governing_requirements = Self::classify_governing_entities(
            &relevant_soll_entities,
            "Requirement",
            "soll_requirement",
        );
        let governing_decisions =
            Self::classify_governing_entities(&relevant_soll_entities, "Decision", "soll_decision");
        let supporting_guidelines = Self::classify_governing_entities(
            &relevant_soll_entities,
            "Guideline",
            "soll_guideline",
        );
        let direct_code_evidence = Self::classify_direct_code_evidence(&direct_evidence);
        let supporting_docs =
            Self::classify_supporting_chunks_by_provenance(&supporting_chunks, "doc", "supporting");
        let supporting_code_context =
            Self::classify_supporting_code_context(&supporting_chunks, &structural_neighbors);
        let why_these_items = self.build_why_these_items(
            route,
            &entry_candidates,
            &supporting_chunks,
            &structural_neighbors,
            &relevant_soll_entities,
        );
        let missing_evidence = self.build_missing_evidence(
            route,
            &entry_candidates,
            &supporting_chunks,
            &relevant_soll_entities,
            rationale_requested,
            has_direct_soll_traceability,
            runtime.semantic_search_used,
            runtime.degraded_reason.as_deref(),
        );
        let evidence_states = Self::build_evidence_states(
            route,
            rationale_requested,
            has_direct_soll_traceability,
            runtime.degraded_reason.as_deref(),
            &governing_requirements,
            &governing_decisions,
            &supporting_guidelines,
            &direct_code_evidence,
            &supporting_docs,
            &supporting_code_context,
        );
        let rationale_quality = Self::build_rationale_quality(
            &evidence_states,
            &governing_requirements,
            &governing_decisions,
            &supporting_guidelines,
        );
        let answer_sketch = self.build_answer_sketch(
            question,
            route,
            &entry_candidates,
            &supporting_chunks,
            &structural_neighbors,
            &governing_requirements,
            &governing_decisions,
            &supporting_guidelines,
            &evidence_states,
        );
        let confidence = self.compute_confidence(
            route,
            &entry_candidates,
            &supporting_chunks,
            &structural_neighbors,
            &relevant_soll_entities,
        );
        timings.packet_assembly_ms = stage_started_at.elapsed().as_millis() as u64;
        timings.total_ms = started_at.elapsed().as_millis() as u64;

        let packet = json!({
            "answer_sketch": answer_sketch,
            "direct_evidence": direct_evidence,
            "supporting_chunks": supporting_chunks,
            "structural_neighbors": structural_neighbors,
            "relevant_soll_entities": relevant_soll_entities,
            "governing_requirements": governing_requirements,
            "governing_decisions": governing_decisions,
            "supporting_guidelines": supporting_guidelines,
            "supporting_docs": supporting_docs,
            "direct_code_evidence": direct_code_evidence,
            "supporting_code_context": supporting_code_context,
            "evidence_states": evidence_states,
            "rationale_quality": rationale_quality,
            "confidence": confidence,
            "missing_evidence": missing_evidence,
            "why_these_items": why_these_items,
            "retrieval_policy": {
                "rationale_requested": rationale_requested,
                "has_direct_soll_traceability": has_direct_soll_traceability,
                "linked_evidence_first": linked_evidence_first,
                "canonical_project_docs_second": linked_evidence_first,
                "broader_workspace_material_last": linked_evidence_first
            },
            "excluded_because": excluded_because,
            "token_budget_estimate": {
                "requested_budget": token_budget,
                "estimated_tokens": Self::estimate_tokens(&[
                    &answer_sketch,
                    &serde_json::to_string(&direct_evidence).unwrap_or_default(),
                    &serde_json::to_string(&supporting_chunks).unwrap_or_default(),
                    &serde_json::to_string(&structural_neighbors).unwrap_or_default(),
                    &serde_json::to_string(&relevant_soll_entities).unwrap_or_default(),
                ]),
            },
            "retrieval_diagnostics": {
                "symbol_candidates_considered": diagnostics.symbol_candidates_considered,
                "file_candidates_considered": diagnostics.file_candidates_considered,
                "chunk_candidates_considered": diagnostics.chunk_candidates_considered,
                "anchored_chunks_selected": diagnostics.anchored_chunks_selected,
                "unanchored_chunks_selected": diagnostics.unanchored_chunks_selected,
                "multipart_chunks_selected": diagnostics.multipart_chunks_selected,
                "multipart_symbol_groups_selected": diagnostics.multipart_symbol_groups_selected,
                "graph_neighbors_selected": diagnostics.graph_neighbors_selected,
                "soll_entities_selected": diagnostics.soll_entities_selected,
                // REQ-AXO-324 slice 2 — FTS modality observability.
                "fts_chunks_considered": diagnostics.fts_chunks_considered,
                "fts_chunks_selected": diagnostics.fts_chunks_selected,
            },
            "retrieval_timings_ms": {
                "planner": timings.planner_ms,
                "entry_lookup": timings.entry_lookup_ms,
                "runtime_guard": timings.runtime_guard_ms,
                "chunk_lookup": timings.chunk_lookup_ms,
                "chunk_selection": timings.chunk_selection_ms,
                "graph_expansion": timings.graph_expansion_ms,
                "soll_join": timings.soll_join_ms,
                "packet_assembly": timings.packet_assembly_ms,
                "total": timings.total_ms,
            }
        });

        let data = json!({
            "planner": {
                "route": route.as_str(),
                "project_scope": project.unwrap_or("*"),
                "project_scope_variants": project_scope_variants,
                "terms": terms_for_reasoning,
                "graph_enabled": include_graph,
                "soll_joined": should_join_soll,
                "semantic_search_used": runtime.semantic_search_used,
                "degraded_reason": runtime.degraded_reason,
                "service_pressure": format!("{:?}", runtime.pressure),
                "graph_projection_queue_depth": runtime.graph_projection_queue_depth,
                "file_vectorization_queue_depth": runtime.file_vectorization_queue_depth,
            },
            "packet": packet
        });

        let evidence = self.render_evidence_packet(&data["packet"], route);
        let evidence = evidence_by_mode(&evidence, mode);
        let scope = project
            .map(|value| format!("project:{value}"))
            .unwrap_or_else(|| "workspace:*".to_string());
        let report = format!(
            "### Context Retrieval: {}\n\n{}",
            question,
            format_standard_contract(
                "ok",
                "planner-driven evidence packet assembled",
                &scope,
                &evidence,
                &[
                    "inspect the top entrypoint",
                    "impact the entrypoint for deeper blast radius",
                    "query the returned URIs directly if raw detail is needed",
                ],
                Self::confidence_label(
                    data["packet"]["confidence"]["score"]
                        .as_f64()
                        .unwrap_or(0.0),
                ),
            )
        );

        let response = json!({
            "content": [{"type": "text", "text": report}],
            "data": data
        });
        Self::write_retrieve_context_cache(cache_key, now_ms, &response);
        Some(response)
    }

    // REQ-AXO-264 Phase A v0 — layered retrieval envelope.
    //
    // Wraps `axon_retrieve_context` and re-organises its packet into the
    // three bands defined by CPT-AXO-050: intent (SOLL), code (chunks),
    // recent (git log + cwd; v0 stub `not_yet_implemented`).
    //
    // Backward compat invariant: the existing `retrieve_context` tool
    // and its response shape are NOT touched. This is a sibling tool
    // dispatched via `retrieve_context_layered`.
    pub(crate) fn axon_retrieve_context_layered(&self, args: &Value) -> Option<Value> {
        let started_at = Instant::now();
        let inner = self.axon_retrieve_context(args)?;

        // Propagate input-validation errors verbatim (same shape, isError=true).
        if inner.get("isError").and_then(|value| value.as_bool()) == Some(true) {
            return Some(inner);
        }

        let inner_data = inner.get("data").cloned().unwrap_or_else(|| json!({}));
        let packet = inner_data.get("packet").cloned().unwrap_or_else(|| json!({}));

        // REQ-AXO-264 A3 v2: per-band token budgets read from
        // `args.bands.{intent,code,recent}.max_tokens`. Defaults from the
        // working note section 6.1 (intent=2000, code=6000, recent=1500).
        // Each band is truncated to fit within `budget * 1.10` (the ±10%
        // tolerance), then a `tokens_overflowed` counter is reported in
        // metadata. Default behaviour (no `bands` arg) preserves v1.
        let intent_budget = Self::layered_band_max_tokens(args, "intent", 2000);
        let code_budget = Self::layered_band_max_tokens(args, "code", 6000);
        let recent_budget = Self::layered_band_max_tokens(args, "recent", 1500);

        // intent_band ← packet.relevant_soll_entities, partitioned by SOLL kind.
        let mut intent_concepts: Vec<Value> = Vec::new();
        let mut intent_decisions: Vec<Value> = Vec::new();
        let mut intent_requirements: Vec<Value> = Vec::new();
        if let Some(entities) = packet.get("relevant_soll_entities").and_then(|value| value.as_array()) {
            for entity in entities {
                let row = json!({
                    "id": entity.get("id").cloned().unwrap_or(Value::Null),
                    "title": entity.get("title").cloned().unwrap_or(Value::Null),
                    "summary": entity.get("description").cloned().unwrap_or(Value::Null),
                    "status": entity.get("status").cloned().unwrap_or(Value::Null),
                });
                let kind = entity.get("entity_type").or_else(|| entity.get("type")).and_then(|value| value.as_str()).unwrap_or("");
                match kind {
                    "Concept" => intent_concepts.push(row),
                    "Decision" => intent_decisions.push(row),
                    "Requirement" => intent_requirements.push(row),
                    _ => intent_requirements.push(row),
                }
            }
        }
        let intent_text_full = serde_json::to_string(&json!({
            "concepts": intent_concepts,
            "decisions": intent_decisions,
            "requirements": intent_requirements,
        })).unwrap_or_default();
        let intent_tokens_pre = Self::estimate_tokens(&[&intent_text_full]);

        // Truncate intent rows in priority order: requirements > decisions > concepts.
        let (intent_concepts_kept, intent_decisions_kept, intent_requirements_kept,
             intent_tokens_post, intent_overflowed) = Self::truncate_intent_band(
            intent_concepts, intent_decisions, intent_requirements, intent_budget,
        );

        // code_band ← packet.direct_evidence + packet.supporting_chunks (chunks reused).
        let mut code_chunks_full: Vec<Value> = Vec::new();
        if let Some(evidence) = packet.get("direct_evidence").and_then(|value| value.as_array()) {
            code_chunks_full.extend(evidence.iter().cloned());
        }
        if let Some(supporting) = packet.get("supporting_chunks").and_then(|value| value.as_array()) {
            code_chunks_full.extend(supporting.iter().cloned());
        }
        let code_tokens_pre = Self::estimate_tokens(&[&serde_json::to_string(&code_chunks_full).unwrap_or_default()]);
        let (code_chunks, code_tokens_post, code_overflowed) =
            Self::truncate_chunks_band(code_chunks_full, code_budget);

        // recent_band — REQ-AXO-264 A6 v1: populate via `git log --since=24h`
        // on the resolved project path. Each commit yields a {file, ts,
        // subject} row per changed file. cwd hint goes into `current_focus`.
        // Falls back to a structured empty band when no project root is
        // resolvable (LLM clients still get a stable contract).
        let project_root = std::env::var("AXON_PROJECT_ROOT")
            .ok()
            .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()));
        let mut recent_band = Self::collect_recent_band(project_root.as_deref());
        let recent_tokens_pre = recent_band.get("tokens_used").and_then(|t| t.as_u64()).unwrap_or(0) as usize;
        let (recent_band_truncated, recent_tokens_post, recent_overflowed) =
            Self::truncate_recent_band(std::mem::take(&mut recent_band), recent_budget);
        recent_band = recent_band_truncated;

        // metadata
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        let total_tokens = intent_tokens_post + code_tokens_post + recent_tokens_post;
        let total_overflowed = intent_overflowed + code_overflowed + recent_overflowed;
        let retrieval_path = inner_data
            .get("planner")
            .and_then(|p| p.get("route"))
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let freshness = inner_data
            .get("planner")
            .and_then(|p| p.get("degraded_reason"))
            .map(|reason| if reason.is_null() { "fresh" } else { "stale" })
            .unwrap_or("unknown");

        let layered_data = json!({
            "intent_band": {
                "concepts": intent_concepts_kept,
                "decisions": intent_decisions_kept,
                "requirements": intent_requirements_kept,
                "tokens_used": intent_tokens_post,
                "tokens_budget": intent_budget,
                "tokens_overflowed": intent_overflowed,
            },
            "code_band": {
                "chunks": code_chunks,
                "tokens_used": code_tokens_post,
                "tokens_budget": code_budget,
                "tokens_overflowed": code_overflowed,
            },
            "recent_band": recent_band,
            "metadata": {
                "snapshot_id": Value::Null,
                "freshness": freshness,
                "retrieval_path": retrieval_path,
                "total_tokens": total_tokens,
                "total_tokens_overflowed": total_overflowed,
                "tokens_pre_truncation": {
                    "intent": intent_tokens_pre,
                    "code": code_tokens_pre,
                    "recent": recent_tokens_pre,
                },
                "elapsed_ms": elapsed_ms,
                "phase_a_version": "v2",
            },
            "legacy_passthrough": inner_data,
        });

        let summary = format!(
            "### Layered Retrieval (Phase A v2)\n\nintent={} concepts/{} decisions/{} requirements (~{} tokens, overflow={})\ncode={} chunks (~{} tokens, overflow={})\nrecent={} entries (~{} tokens, overflow={})\nretrieval_path={} elapsed={}ms",
            intent_concepts_kept.len(), intent_decisions_kept.len(), intent_requirements_kept.len(),
            intent_tokens_post, intent_overflowed,
            code_chunks.len(), code_tokens_post, code_overflowed,
            recent_band.get("git_recent_edits").and_then(|v| v.as_array()).map_or(0, |a| a.len()),
            recent_tokens_post, recent_overflowed,
            retrieval_path, elapsed_ms,
        );

        Some(json!({
            "content": [{"type": "text", "text": summary}],
            "data": layered_data,
        }))
    }

    // REQ-AXO-264 A3 v2 — read `args.bands.<band>.max_tokens` with a default.
    fn layered_band_max_tokens(args: &Value, band: &str, default: usize) -> usize {
        args.get("bands")
            .and_then(|b| b.get(band))
            .and_then(|cfg| cfg.get("max_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(default)
            .max(50) // hard floor so we always emit at least the band scaffold
    }

    // intent_band truncation: drop rows from the back, prioritising
    // requirements > decisions > concepts (the closer-to-action signal first).
    // Returns (concepts_kept, decisions_kept, requirements_kept, tokens_post, overflow_count).
    fn truncate_intent_band(
        concepts: Vec<Value>,
        decisions: Vec<Value>,
        requirements: Vec<Value>,
        budget: usize,
    ) -> (Vec<Value>, Vec<Value>, Vec<Value>, usize, usize) {
        let measure = |c: &[Value], d: &[Value], r: &[Value]| -> usize {
            let s = serde_json::to_string(&json!({
                "concepts": c, "decisions": d, "requirements": r
            })).unwrap_or_default();
            Self::estimate_tokens(&[&s])
        };
        let pre = measure(&concepts, &decisions, &requirements);
        if pre <= budget {
            return (concepts, decisions, requirements, pre, 0);
        }
        // Truncate in reverse priority: concepts first, then decisions, then requirements.
        let mut c = concepts.clone();
        let mut d = decisions.clone();
        let mut r = requirements.clone();
        let initial_total = c.len() + d.len() + r.len();
        while measure(&c, &d, &r) > budget {
            if !c.is_empty() {
                c.pop();
            } else if !d.is_empty() {
                d.pop();
            } else if !r.is_empty() {
                r.pop();
            } else {
                break;
            }
        }
        let kept_total = c.len() + d.len() + r.len();
        let dropped = initial_total - kept_total;
        let post = measure(&c, &d, &r);
        (c, d, r, post, dropped)
    }

    // code_band truncation: drop chunks from the back (the lowest-ranked).
    fn truncate_chunks_band(chunks: Vec<Value>, budget: usize) -> (Vec<Value>, usize, usize) {
        let pre_text = serde_json::to_string(&chunks).unwrap_or_default();
        let pre = Self::estimate_tokens(&[&pre_text]);
        if pre <= budget {
            return (chunks, pre, 0);
        }
        let mut kept = chunks;
        let initial = kept.len();
        while !kept.is_empty()
            && Self::estimate_tokens(&[&serde_json::to_string(&kept).unwrap_or_default()]) > budget
        {
            kept.pop();
        }
        let dropped = initial - kept.len();
        let post = Self::estimate_tokens(&[&serde_json::to_string(&kept).unwrap_or_default()]);
        (kept, post, dropped)
    }

    // recent_band truncation: drop oldest git_recent_edits entries first.
    fn truncate_recent_band(mut band: Value, budget: usize) -> (Value, usize, usize) {
        let entries: Vec<Value> = band
            .get_mut("git_recent_edits")
            .and_then(|v| v.as_array_mut())
            .map(|a| std::mem::take(a))
            .unwrap_or_default();
        let pre_text = serde_json::to_string(&entries).unwrap_or_default();
        let pre = Self::estimate_tokens(&[&pre_text]);
        if pre <= budget {
            // Restore entries unchanged + recompute tokens_used to be safe.
            band["git_recent_edits"] = json!(entries);
            band["tokens_used"] = json!(pre);
            return (band, pre, 0);
        }
        let mut kept = entries;
        let initial = kept.len();
        while !kept.is_empty()
            && Self::estimate_tokens(&[&serde_json::to_string(&kept).unwrap_or_default()]) > budget
        {
            // Entries are sorted newest-first; pop drops the oldest.
            kept.pop();
        }
        let dropped = initial - kept.len();
        let post = Self::estimate_tokens(&[&serde_json::to_string(&kept).unwrap_or_default()]);
        band["git_recent_edits"] = json!(kept);
        band["tokens_used"] = json!(post);
        (band, post, dropped)
    }

    // REQ-AXO-264 A6 v1 — recent_band collector.
    //
    // Runs `git log --since=24.hours --name-only --pretty=format:%H\x01%ct\x01%s`
    // in the resolved project root. Each commit emits its hash/timestamp/
    // subject followed by changed paths. We collect (file, last_commit_ts,
    // last_subject) keyed by file (most recent commit wins).
    //
    // Returns a stable JSON contract:
    //   { git_recent_edits: [...], current_focus: ..., tokens_used: N,
    //     status: "ok" | "no_project_root" | "git_error", ... }
    //
    // If git fails or there's no project root, returns an empty band tagged
    // with the failure reason so LLM clients can act on it.
    pub(crate) fn collect_recent_band(project_root: Option<&str>) -> Value {
        let Some(root) = project_root else {
            return json!({
                "git_recent_edits": [],
                "current_focus": Value::Null,
                "tokens_used": 0,
                "status": "no_project_root",
            });
        };
        if !std::path::Path::new(root).is_dir() {
            return json!({
                "git_recent_edits": [],
                "current_focus": Value::Null,
                "tokens_used": 0,
                "status": "no_project_root",
                "reason": format!("path not a directory: {root}"),
            });
        }

        let output = std::process::Command::new("git")
            .arg("-C").arg(root)
            .arg("log")
            .arg("--since=24.hours")
            .arg("--name-only")
            .arg("--pretty=format:%H\x01%ct\x01%s")
            .output();

        let stdout = match output {
            Ok(o) if o.status.success() => o.stdout,
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                return json!({
                    "git_recent_edits": [],
                    "current_focus": Value::Null,
                    "tokens_used": 0,
                    "status": "git_error",
                    "reason": stderr.lines().next().unwrap_or("").to_string(),
                });
            }
            Err(err) => {
                return json!({
                    "git_recent_edits": [],
                    "current_focus": Value::Null,
                    "tokens_used": 0,
                    "status": "git_error",
                    "reason": err.to_string(),
                });
            }
        };

        let text = String::from_utf8_lossy(&stdout);
        let mut by_file: std::collections::BTreeMap<String, (i64, String, String)> = std::collections::BTreeMap::new();
        let mut current_hash = String::new();
        let mut current_ts: i64 = 0;
        let mut current_subject = String::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            if line.contains('\x01') {
                let mut parts = line.splitn(3, '\x01');
                current_hash = parts.next().unwrap_or("").to_string();
                current_ts = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                current_subject = parts.next().unwrap_or("").to_string();
            } else if !current_hash.is_empty() {
                // git log already emits files newest first; only insert if
                // this is the first time we see this path (preserves the
                // most recent commit per file).
                by_file
                    .entry(line.to_string())
                    .or_insert_with(|| (current_ts, current_hash.clone(), current_subject.clone()));
            }
        }

        let mut entries: Vec<Value> = by_file
            .into_iter()
            .map(|(file, (ts, hash, subject))| {
                json!({
                    "file": file,
                    "last_commit_ts": ts,
                    "last_commit_hash": hash,
                    "last_commit_subject": subject,
                })
            })
            .collect();
        // Sort by recency (newest first).
        entries.sort_by(|a, b| {
            b["last_commit_ts"].as_i64().unwrap_or(0).cmp(&a["last_commit_ts"].as_i64().unwrap_or(0))
        });
        let recent_text = serde_json::to_string(&entries).unwrap_or_default();
        let tokens_used = Self::estimate_tokens(&[&recent_text]);

        // current_focus: best-effort cwd hint (the dir we're in, relative to
        // the project root if possible). Does NOT touch open editor state.
        let current_focus = std::env::current_dir().ok().map(|cwd| {
            let cwd_str = cwd.to_string_lossy().to_string();
            let rel = cwd_str
                .strip_prefix(root)
                .map(|s| s.trim_start_matches('/').to_string())
                .unwrap_or_else(|| cwd_str.clone());
            json!({ "cwd": cwd_str, "relative_to_project": rel })
        });

        json!({
            "git_recent_edits": entries,
            "current_focus": current_focus,
            "tokens_used": tokens_used,
            "status": "ok",
            "window": "24h",
        })
    }

    fn plan_retrieval_route(question: &str) -> RetrievalRoute {
        let lower = question.to_ascii_lowercase();
        if lower.contains("what breaks if")
            || lower.contains("blast radius")
            || lower.contains("impact of")
            || lower.contains("if ") && (lower.contains(" changes") || lower.contains(" changed"))
        {
            RetrievalRoute::Impact
        } else if lower.contains("why ")
            || lower.contains("rationale")
            || lower.contains("decision")
            || lower.contains("requirement")
            || lower.contains("architectural intent")
        {
            RetrievalRoute::SollHybrid
        } else if lower.contains("where is")
            || lower.contains("wired")
            || lower.contains("hooked")
            || lower.contains("connected")
        {
            RetrievalRoute::Wiring
        } else if Self::looks_like_exact_lookup(question) {
            RetrievalRoute::ExactLookup
        } else {
            RetrievalRoute::Hybrid
        }
    }

    fn looks_like_exact_lookup(question: &str) -> bool {
        let trimmed = question.trim();
        let token_count = trimmed.split_whitespace().count();
        token_count <= 3
            && trimmed
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.' | '-' | '/'))
    }

    fn question_terms(question: &str) -> Vec<String> {
        let stopwords = [
            "what",
            "breaks",
            "if",
            "why",
            "does",
            "use",
            "the",
            "where",
            "is",
            "wired",
            "hooked",
            "connected",
            "changes",
            "changed",
            "and",
            "for",
            "with",
            "this",
            "that",
            "from",
            "into",
            "how",
            "say",
            "about",
        ];
        let stopwords = stopwords.into_iter().collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        question
            .split(|ch: char| {
                !ch.is_ascii_alphanumeric()
                    && ch != '_'
                    && ch != ':'
                    && ch != '-'
                    && ch != '/'
                    && ch != '.'
            })
            .filter_map(|token| {
                let normalized = token.trim().to_ascii_lowercase();
                if normalized.len() < 3 || stopwords.contains(normalized.as_str()) {
                    return None;
                }
                if !seen.insert(normalized.clone()) {
                    return None;
                }
                Some(normalized)
            })
            .collect()
    }

    fn question_path_hints(question: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        question
            .split_whitespace()
            .filter_map(|token| {
                let normalized = token
                    .trim_matches(|ch: char| {
                        matches!(ch, '"' | '\'' | '`' | ',' | '.' | ';' | ':' | '(' | ')')
                    })
                    .trim();
                if normalized.is_empty() {
                    return None;
                }
                if !(normalized.contains('/') || normalized.contains('.')) {
                    return None;
                }
                let value = normalized.to_ascii_lowercase();
                if !seen.insert(value.clone()) {
                    return None;
                }
                Some(value)
            })
            .collect()
    }

    pub(crate) fn project_scope_variants(project: Option<&str>) -> Vec<String> {
        let Some(project) = project.map(str::trim).filter(|value| !value.is_empty()) else {
            return Vec::new();
        };

        let mut values = Vec::new();
        let mut seen = HashSet::new();
        let mut push = |value: String| {
            if !value.is_empty() && seen.insert(value.to_ascii_lowercase()) {
                values.push(value);
            }
        };

        push(project.to_string());
        push(project.to_ascii_lowercase());

        if let Ok(identity) = crate::project_meta::resolve_canonical_project_identity(project) {
            push(identity.code.clone());
            push(identity.code.to_ascii_lowercase());

            if let Some(repo_root) = identity
                .project_path
                .file_name()
                .and_then(|name| name.to_str())
            {
                push(repo_root.to_string());
                push(repo_root.to_ascii_lowercase());
            }
        }

        values
    }

    pub(crate) fn sql_project_filter_for_fields(project: Option<&str>, fields: &[&str]) -> String {
        let variants = Self::project_scope_variants(project);
        if variants.is_empty() || fields.is_empty() {
            return String::new();
        }

        let values = variants
            .iter()
            .map(|value| format!("'{}'", Self::escape_sql(&value.to_ascii_lowercase())))
            .collect::<Vec<_>>()
            .join(", ");
        let predicates = fields
            .iter()
            .map(|field| format!("lower({field}) IN ({values})"))
            .collect::<Vec<_>>()
            .join(" OR ");

        format!(" AND ({predicates})")
    }

    fn term_match_sql(terms: &[String], column: &str) -> String {
        if terms.is_empty() {
            return "1=1".to_string();
        }
        terms
            .iter()
            .map(|term| format!("lower({column}) LIKE '%{}%'", Self::escape_sql(term)))
            .collect::<Vec<_>>()
            .join(" OR ")
    }

    fn path_match_sql(path_hints: &[String], column: &str) -> String {
        if path_hints.is_empty() {
            return "1=0".to_string();
        }
        path_hints
            .iter()
            .map(|hint| format!("lower({column}) LIKE '%{}%'", Self::escape_sql(hint)))
            .collect::<Vec<_>>()
            .join(" OR ")
    }

    fn has_rationale_language(question: &str) -> bool {
        let lower = question.to_ascii_lowercase();
        [
            "why",
            "rationale",
            "decision",
            "requirement",
            "constraint",
            "intent",
            "designed this way",
            "design choice",
            "architectural intent",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    fn route_prefers_operational_code(route: RetrievalRoute) -> bool {
        matches!(
            route,
            RetrievalRoute::ExactLookup | RetrievalRoute::Wiring | RetrievalRoute::Impact
        )
    }

    fn prefer_project_intent(question: &str, mode: Option<&str>) -> bool {
        if mode.is_some_and(|value| value.eq_ignore_ascii_case("intent")) {
            return true;
        }
        let lower = question.to_ascii_lowercase();
        [
            "soll mutation",
            "what soll mutation",
            "implementation plan",
            "concept foundation",
            "must support",
            "weekly plan",
            "project intent",
            "entrench",
            "recipe creation",
            "normalization",
            "attachment",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    pub(crate) fn project_intent_doc_weight(uri: &str) -> f64 {
        let lower = uri.to_ascii_lowercase();
        let mut score = 0.0;
        if lower.contains("/docs/plans/") || lower.starts_with("docs/plans/") {
            score += 4.0;
        }
        if lower.contains("concept-foundation") {
            score += 3.0;
        }
        if lower.contains("implementation-plan") {
            score += 3.0;
        }
        if lower.contains("feedback-axon") {
            score -= 6.0;
        }
        if lower.contains("operator") || lower.contains("retrospective") {
            score -= 2.0;
        }
        score
    }

    fn canonical_project_doc_weight(uri: &str, project_scope_variants: &[String]) -> f64 {
        let lower = uri.to_ascii_lowercase();
        let mut score = 0.0;
        if lower.contains("/docs/plans/") || lower.starts_with("docs/plans/") {
            score += 4.5;
        }
        if lower.contains("/docs/vision/") || lower.starts_with("docs/vision/") {
            score += 4.0;
        }
        if lower.contains("/docs/derived/soll/") || lower.starts_with("docs/derived/soll/") {
            score += 3.0;
        }
        if lower.ends_with("readme.md") || lower == "readme.md" {
            score += 1.5;
        }
        if project_scope_variants.iter().any(|variant| {
            let variant = variant.to_ascii_lowercase();
            lower.contains(&format!("/{variant}/")) || lower.contains(&format!("-{variant}-"))
        }) {
            score += 1.0;
        }
        score
    }

    fn workspace_noise_penalty(uri: &str) -> f64 {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("/.axon/")
            || lower.starts_with(".axon/")
            || lower.contains("/target/")
            || lower.starts_with("target/")
            || lower.contains("/tmp/")
            || lower.starts_with("/tmp/")
            || lower.contains("/scripts/")
            || lower.starts_with("scripts/")
        {
            -3.0
        } else if lower.contains("feedback-") || lower.contains("/feedback/") {
            -2.5
        } else {
            0.0
        }
    }

    fn uri_penalty_reason(uri: &str) -> Option<&'static str> {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("/tests/")
            || lower.contains("/test/")
            || lower.starts_with("tests/")
            || lower.starts_with("test/")
            || lower.ends_with("/tests.rs")
            || lower.ends_with("_test.exs")
            || lower.ends_with("_test.ex")
            || lower.ends_with("_test.rs")
        {
            Some("test_file_penalty")
        } else if lower.contains("/docs/") || lower.starts_with("docs/") || lower.ends_with(".md") {
            Some("docs_file_penalty")
        } else if lower.contains("/examples/") || lower.starts_with("examples/") {
            Some("non_operational_chunk_penalized")
        } else if lower.contains("/fixtures/") || lower.starts_with("fixtures/") {
            Some("non_operational_chunk_penalized")
        } else {
            None
        }
    }

    fn chunk_penalty_reason(candidate: &ChunkCandidate) -> Option<&'static str> {
        if let Some(reason) = Self::uri_penalty_reason(&candidate.uri) {
            return Some(reason);
        }
        let source_lower = candidate.source_id.to_ascii_lowercase();
        let content_lower = candidate.content.to_ascii_lowercase();
        if source_lower.ends_with("::tests")
            || source_lower.contains("::test_")
            || content_lower.contains("fn test_")
            || content_lower.contains("mod tests")
            || content_lower.contains("#[test]")
        {
            Some("test_file_penalty")
        } else {
            None
        }
    }

    fn find_entry_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        path_hints: &[String],
        limit: usize,
        diagnostics: &mut RetrievalDiagnostics,
    ) -> Vec<EntryCandidate> {
        let mut entries = self.find_symbol_candidates(project, terms, path_hints, limit);
        diagnostics.symbol_candidates_considered = entries.len();
        let file_candidates = self.find_file_candidates(project, terms, path_hints, limit);
        diagnostics.file_candidates_considered = file_candidates.len();
        entries.extend(file_candidates);
        if entries.is_empty() {
            if let Some(repo_root) = Self::project_repo_root(project) {
                let mut fallback = self.find_symbol_candidates(None, terms, path_hints, limit);
                fallback.retain(|candidate| candidate.uri.starts_with(&repo_root));
                diagnostics.symbol_candidates_considered += fallback.len();
                let mut fallback_files = self.find_file_candidates(None, terms, path_hints, limit);
                fallback_files.retain(|candidate| candidate.uri.starts_with(&repo_root));
                diagnostics.file_candidates_considered += fallback_files.len();
                fallback.extend(fallback_files);
                entries.extend(fallback);
            }
        }
        if entries.is_empty() {
            let (repo_entries, _) = self.repo_literal_fallback_candidates(project, terms, limit);
            diagnostics.file_candidates_considered += repo_entries.len();
            entries.extend(repo_entries);
        }
        entries
    }

    fn project_repo_root(project: Option<&str>) -> Option<String> {
        let project = project.map(str::trim).filter(|value| !value.is_empty())?;
        let identity = crate::project_meta::resolve_canonical_project_identity(project).ok()?;
        let repo_root = identity.meta_path.parent()?.parent()?;
        Some(repo_root.to_string_lossy().into_owned())
    }

    fn is_strong_identifier_term(term: &str) -> bool {
        term.len() >= 4
            && term
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.'))
    }

    fn repo_literal_file_rank(path: &str) -> i32 {
        let lower = path.to_ascii_lowercase();
        let mut score = 0i32;
        if lower.ends_with(".rs")
            || lower.ends_with(".ex")
            || lower.ends_with(".exs")
            || lower.ends_with(".py")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".js")
            || lower.ends_with(".jsx")
        {
            score += 4;
        }
        if lower.contains("/src/") {
            score += 3;
        }
        if lower.contains("/test/")
            || lower.contains("/tests/")
            || lower.starts_with("test/")
            || lower.starts_with("tests/")
        {
            score -= 4;
        }
        if lower.contains("/docs/") || lower.starts_with("docs/") || lower.ends_with(".md") {
            score -= 3;
        }
        score
    }

    fn should_consider_repo_literal_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        if lower.contains("/.git/")
            || lower.contains("/target/")
            || lower.contains("/.axon/")
            || lower.contains("/node_modules/")
            || lower.contains("/dist/")
            || lower.contains("/build/")
            || lower.contains("/_build/")
            || lower.contains("/deps/")
            || lower.contains("/test/")
            || lower.contains("/tests/")
            || lower.ends_with("/tests.rs")
            || lower.ends_with("_test.exs")
            || lower.ends_with("_test.ex")
            || lower.ends_with("_test.rs")
            || lower.ends_with(".test.ts")
            || lower.ends_with(".test.js")
            || lower.contains("/docs/")
            || lower.ends_with(".md")
        {
            return false;
        }

        lower.ends_with(".rs")
            || lower.ends_with(".ex")
            || lower.ends_with(".exs")
            || lower.ends_with(".py")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".js")
            || lower.ends_with(".jsx")
    }

    fn snippet_around_term(content: &str, term: &str) -> Option<String> {
        let lower = content.to_ascii_lowercase();
        let needle = term.to_ascii_lowercase();
        let offset = lower.find(&needle)?;
        let start = offset.saturating_sub(100);
        let end = (offset + needle.len() + 120).min(content.len());
        Some(Self::truncate(
            content.get(start..end).unwrap_or(content).trim(),
            220,
        ))
    }

    fn repo_literal_fallback_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        limit: usize,
    ) -> (Vec<EntryCandidate>, Vec<ChunkCandidate>) {
        let Some(repo_root) = Self::project_repo_root(project) else {
            return (Vec::new(), Vec::new());
        };
        let repo_root_path = Path::new(&repo_root);
        if !repo_root_path.exists() {
            return (Vec::new(), Vec::new());
        }

        let strong_terms = terms
            .iter()
            .filter(|term| Self::is_strong_identifier_term(term))
            .cloned()
            .collect::<Vec<_>>();
        if strong_terms.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let project_code = project
            .and_then(|value| crate::project_meta::resolve_canonical_project_identity(value).ok())
            .map(|identity| identity.code)
            .or_else(|| {
                project
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown".to_string());
        let mut matches = Vec::new();
        for entry in WalkBuilder::new(repo_root_path)
            .hidden(false)
            .standard_filters(true)
            .build()
        {
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().map(|ty| ty.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let path_str = path.to_string_lossy().into_owned();
            if !Self::should_consider_repo_literal_path(&path_str) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.len() > 512 * 1024 {
                continue;
            }
            let Ok(content) = fs::read_to_string(path) else {
                continue;
            };
            let content_lower = content.to_ascii_lowercase();
            let mut matched_terms = strong_terms
                .iter()
                .filter(|term| content_lower.contains(term.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if matched_terms.is_empty() {
                continue;
            }
            matched_terms.sort();
            let match_term = matched_terms[0].clone();
            let lexical_hits = matched_terms.len();
            let snippet = Self::snippet_around_term(&content, &match_term)
                .unwrap_or_else(|| Self::truncate(content.lines().next().unwrap_or_default(), 220));
            matches.push((
                Self::repo_literal_file_rank(&path_str),
                lexical_hits,
                match_term,
                path_str,
                snippet,
                matched_terms,
            ));
        }

        matches.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.cmp(&left.1))
                .then_with(|| left.3.cmp(&right.3))
        });

        let mut entries = Vec::new();
        let mut chunks = Vec::new();
        let mut seen_paths = HashSet::new();
        for (base_rank, lexical_hits, match_term, path_str, snippet, matched_terms) in matches {
            if !seen_paths.insert(path_str.clone()) {
                continue;
            }
            let reasons = vec![
                "repo_literal_fallback".to_string(),
                "repo_root_match".to_string(),
                "content_term_match".to_string(),
            ];
            entries.push(EntryCandidate {
                id: path_str.clone(),
                name: match_term.clone(),
                kind: "repo_literal".to_string(),
                project_code: project_code.clone(),
                uri: path_str.clone(),
                lexical_hits,
                exact_match: true,
                score: 4.0 + f64::from(base_rank.max(0)),
                reasons: reasons.clone(),
            });
            chunks.push(ChunkCandidate {
                chunk_id: format!("repo_literal::{path_str}::{match_term}"),
                source_id: path_str.clone(),
                project_code: project_code.clone(),
                uri: path_str.clone(),
                content: snippet,
                match_reason: "repo_literal".to_string(),
                lexical_hits: matched_terms.len(),
                semantic_distance: None,
                chunk_part_index: 1,
                chunk_part_count: 1,
                chunk_path: "1/1".to_string(),
                anchored_to_entry: true,
                same_file_as_entry: true,
                score: 4.0 + f64::from(base_rank.max(0)),
                reasons,
                fts_rank: None,
            });
            if entries.len() >= limit.min(2) {
                break;
            }
        }

        (entries, chunks)
    }

    fn find_symbol_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        path_hints: &[String],
        limit: usize,
    ) -> Vec<EntryCandidate> {
        let mut candidates = self.find_exact_symbol_candidates(project, terms, limit);
        let name_match = Self::term_match_sql(terms, "s.name");
        let path_match = Self::path_match_sql(path_hints, "f.path");
        let uri_term_match = Self::term_match_sql(terms, "f.path");
        let query = format!(
            "SELECT s.id, s.name, s.kind, COALESCE(s.project_code, 'unknown'), COALESCE(f.path, '') \
             FROM Symbol s \
             LEFT JOIN CONTAINS c ON c.target_id = s.id \
             LEFT JOIN File f ON f.path = c.source_id \
             WHERE ({name_match} OR {uri_term_match} OR {path_match}){project_filter} \
             LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["s.project_code", "f.project_code"]),
        );

        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        candidates.extend(rows.into_iter().filter_map(|row| {
            let id = row.first()?.as_str()?.to_string();
            let name = row.get(1)?.as_str()?.to_string();
            let kind = row.get(2)?.as_str()?.to_string();
            let project_code = row.get(3)?.as_str()?.to_string();
            let uri = row.get(4)?.as_str().unwrap_or_default().to_string();
            let lexical_hits = terms
                .iter()
                .filter(|term| {
                    name.to_ascii_lowercase().contains(term.as_str())
                        || uri.to_ascii_lowercase().contains(term.as_str())
                })
                .count();
            let exact_match = terms.iter().any(|term| name.eq_ignore_ascii_case(term))
                || path_hints.iter().any(|hint| uri.eq_ignore_ascii_case(hint));
            Some(EntryCandidate {
                id,
                name,
                kind,
                project_code,
                uri,
                lexical_hits,
                exact_match,
                score: 0.0,
                reasons: Vec::new(),
            })
        }));
        candidates
    }

    fn find_exact_symbol_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        limit: usize,
    ) -> Vec<EntryCandidate> {
        let exact_terms = terms
            .iter()
            .filter(|term| Self::is_strong_identifier_term(term))
            .map(|term| term.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        if exact_terms.is_empty() {
            return Vec::new();
        }

        let exact_values = exact_terms
            .iter()
            .map(|term| format!("'{}'", Self::escape_sql(term)))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT s.id, s.name, s.kind, COALESCE(s.project_code, 'unknown'), COALESCE(f.path, '') \
             FROM Symbol s \
             LEFT JOIN CONTAINS c ON c.target_id = s.id \
             LEFT JOIN File f ON f.path = c.source_id \
             WHERE lower(s.name) IN ({exact_values}){project_filter} \
             LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["s.project_code", "f.project_code"]),
        );

        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|row| {
                let id = row.first()?.as_str()?.to_string();
                let name = row.get(1)?.as_str()?.to_string();
                let kind = row.get(2)?.as_str()?.to_string();
                let project_code = row.get(3)?.as_str()?.to_string();
                let uri = row.get(4)?.as_str().unwrap_or_default().to_string();
                Some(EntryCandidate {
                    id,
                    name,
                    kind,
                    project_code,
                    uri,
                    lexical_hits: 1,
                    exact_match: true,
                    score: 0.0,
                    reasons: vec!["exact_symbol_lookup".to_string()],
                })
            })
            .collect()
    }

    fn find_file_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        path_hints: &[String],
        limit: usize,
    ) -> Vec<EntryCandidate> {
        let path_match = Self::path_match_sql(path_hints, "f.path");
        let term_match = Self::term_match_sql(terms, "f.path");
        let query = format!(
            "SELECT f.path, COALESCE(f.project_code, 'unknown') \
             FROM File f \
             WHERE ({path_match} OR {term_match}){project_filter} \
             LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["f.project_code"]),
        );

        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|row| {
                let path = row.first()?.as_str()?.to_string();
                let project_code = row.get(1)?.as_str()?.to_string();
                let lexical_hits = terms
                    .iter()
                    .filter(|term| path.to_ascii_lowercase().contains(term.as_str()))
                    .count();
                let exact_match = path_hints
                    .iter()
                    .any(|hint| path.eq_ignore_ascii_case(hint))
                    || terms.iter().any(|term| path.eq_ignore_ascii_case(term));
                Some(EntryCandidate {
                    id: path.clone(),
                    name: path.clone(),
                    kind: "file".to_string(),
                    project_code,
                    uri: path,
                    lexical_hits,
                    exact_match,
                    score: 0.0,
                    reasons: Vec::new(),
                })
            })
            .collect()
    }

    fn resolve_file_symbol_bindings(
        &self,
        project: Option<&str>,
        file_paths: &[String],
    ) -> Vec<(String, String)> {
        if file_paths.is_empty() {
            return Vec::new();
        }
        // REQ-AXO-299 / MIL-AXO-017 slice 5 : CONTAINS legacy table superseded
        // by public.Edge (REQ-AXO-295) with relation_type='contains'. A3
        // dual-writes the relation since REQ-AXO-297 slice 3.
        let values = file_paths
            .iter()
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT target_id, source_id \
             FROM public.Edge \
             WHERE relation_type = 'contains' \
               AND source_id IN ({values}){project_filter}",
            project_filter = Self::sql_project_filter_for_fields(project, &["project_code"]),
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|row| {
                Some((
                    row.first()?.as_str()?.to_string(),
                    row.get(1)?.as_str()?.to_string(),
                ))
            })
            .collect()
    }

    fn rerank_entry_candidates(
        &self,
        candidates: &mut [EntryCandidate],
        route: RetrievalRoute,
        terms: &[String],
        path_hints: &[String],
        project_scope_variants: &[String],
        prefer_project_intent: bool,
    ) {
        let scope_lc = project_scope_variants
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        for candidate in candidates.iter_mut() {
            let mut score = (candidate.lexical_hits as f64) * 2.0;
            if candidate.exact_match {
                score += 5.0;
                candidate.reasons.push("exact_anchor_match".to_string());
            }
            if !candidate.uri.is_empty() {
                score += 1.0;
                candidate.reasons.push("file_anchored".to_string());
            }
            if scope_lc.contains(&candidate.project_code.to_ascii_lowercase()) {
                score += 1.5;
                candidate.reasons.push("project_scope_match".to_string());
            }
            if matches!(route, RetrievalRoute::Wiring | RetrievalRoute::Impact)
                && matches!(candidate.kind.as_str(), "function" | "method")
            {
                score += 1.5;
                candidate
                    .reasons
                    .push("route_prefers_callable_anchor".to_string());
            }
            if candidate.kind == "file" {
                score += 1.0;
                candidate.reasons.push("file_entrypoint".to_string());
            }
            if prefer_project_intent {
                let intent_weight = Self::project_intent_doc_weight(&candidate.uri);
                if intent_weight > 0.0 {
                    score += intent_weight;
                    candidate
                        .reasons
                        .push("intent_canonical_plan_bonus".to_string());
                } else if intent_weight < 0.0 {
                    score += intent_weight;
                    candidate
                        .reasons
                        .push("intent_feedback_penalty".to_string());
                }
            }
            if path_hints
                .iter()
                .any(|hint| candidate.uri.to_ascii_lowercase().contains(hint))
            {
                score += 2.0;
                candidate.reasons.push("path_hint_match".to_string());
            }
            if terms
                .iter()
                .any(|term| candidate.uri.to_ascii_lowercase().contains(term))
            {
                score += 1.0;
                candidate.reasons.push("uri_term_match".to_string());
            }
            candidate.score = score;
        }
        candidates.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.uri.cmp(&right.uri))
        });
    }

    fn select_entry_candidates(
        &self,
        candidates: &[EntryCandidate],
        top_k: usize,
    ) -> Vec<EntryCandidate> {
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for candidate in candidates.iter().take(top_k * 2) {
            let key = format!("{}:{}", candidate.kind, candidate.id);
            if !seen.insert(key) {
                continue;
            }
            selected.push(candidate.clone());
            if selected.len() >= top_k.min(2) {
                break;
            }
        }
        selected
    }

    fn is_strong_anchor(candidate: &EntryCandidate) -> bool {
        candidate.exact_match || candidate.lexical_hits > 0
    }

    #[allow(clippy::too_many_arguments)]
    fn find_chunk_candidates(
        &self,
        project: Option<&str>,
        question: &str,
        terms: &[String],
        path_hints: &[String],
        entry_candidates: &[EntryCandidate],
        route: RetrievalRoute,
        limit: usize,
        excluded_because: &mut Vec<String>,
        semantic_allowed: bool,
        runtime: &mut RetrievalRuntimeState,
    ) -> Vec<ChunkCandidate> {
        // MIL-AXO-015 P4 slice 4d (post-CPT-AXO-039 supersedure
        // 2026-05-08): IST tables live in `public` with `project_code`
        // as a row column, identical to the DuckDB layout. Under PG we
        // only swap the *vector* dialect (pgvector `<=>` instead of
        // `array_cosine_distance`, `'[..]'::vector(N)` instead of
        // `CAST(... AS FLOAT[N])`). Table names + project_code filters
        // stay the same as the DuckDB path.
        let is_pg = self.graph_store.is_postgres_backend();

        let entry_ids = entry_candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect::<HashSet<_>>();
        let entry_uris = entry_candidates
            .iter()
            .map(|candidate| candidate.uri.clone())
            .collect::<HashSet<_>>();
        let entry_id_match = if entry_ids.is_empty() {
            "1=0".to_string()
        } else {
            entry_ids
                .iter()
                .map(|entry_id| format!("c.source_id = '{}'", Self::escape_sql(entry_id)))
                .collect::<Vec<_>>()
                .join(" OR ")
        };
        let entry_uri_match = if entry_uris.is_empty() {
            "1=0".to_string()
        } else {
            entry_uris
                .iter()
                .map(|uri| format!("f.path = '{}'", Self::escape_sql(uri)))
                .collect::<Vec<_>>()
                .join(" OR ")
        };
        let lexical_predicate = Self::term_match_sql(terms, "c.content");
        let path_match = Self::path_match_sql(path_hints, "f.path");
        let lexical_uri_match = Self::term_match_sql(terms, "f.path");

        let semantic = if semantic_allowed {
            match crate::embedder::batch_embed(vec![question.to_string()]) {
                Ok(vectors) => {
                    runtime.semantic_search_used = true;
                    vectors.into_iter().next()
                }
                Err(err) => {
                    excluded_because.push("semantic_chunk_search_unavailable".to_string());
                    excluded_because.push(format!(
                        "semantic_chunk_search_error:{}",
                        Self::truncate(&err.to_string(), 120)
                    ));
                    None
                }
            }
        } else {
            excluded_because.push("semantic_chunk_search_skipped".to_string());
            None
        };

        if Self::route_prefers_operational_code(route)
            && (!entry_ids.is_empty() || !entry_uris.is_empty())
        {
            let file_bindings = self.resolve_file_symbol_bindings(
                project,
                &entry_uris.iter().cloned().collect::<Vec<_>>(),
            );
            let mut source_to_uri = entry_candidates
                .iter()
                .filter(|candidate| !candidate.uri.is_empty())
                .map(|candidate| (candidate.id.clone(), candidate.uri.clone()))
                .collect::<std::collections::HashMap<_, _>>();
            let mut same_file_source_ids = Vec::new();
            for (source_id, file_path) in file_bindings {
                source_to_uri
                    .entry(source_id.clone())
                    .or_insert(file_path.clone());
                same_file_source_ids.push(source_id);
            }
            let fast_path_ids = entry_ids
                .iter()
                .cloned()
                .chain(same_file_source_ids.iter().cloned())
                .collect::<HashSet<_>>();
            let fast_path_filter = if fast_path_ids.is_empty() {
                String::new()
            } else {
                let values = fast_path_ids
                    .iter()
                    .map(|value| format!("'{}'", Self::escape_sql(value)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("c.source_id IN ({values})")
            };
            let anchored_query = format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), c.content, \
                        COALESCE(c.chunk_part_index, 1), COALESCE(c.chunk_part_count, 1), COALESCE(c.chunk_path, '1/1'), \
                 CASE \
                            WHEN ({entry_id_match}) THEN 'entry_anchor' \
                            ELSE 'same_file' \
                        END \
                 FROM Chunk c \
                 WHERE ({fast_path_filter}){project_filter} \
                 LIMIT {limit}",
                project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]),
                limit = limit.min(12),
            );
            let anchored_raw = self
                .graph_store
                .query_json(&anchored_query)
                .unwrap_or_else(|_| "[]".to_string());
            let anchored_rows: Vec<Vec<Value>> =
                serde_json::from_str(&anchored_raw).unwrap_or_default();
            let anchored_candidates = anchored_rows
                .into_iter()
                .filter_map(|row| {
                    let chunk_id = row.first()?.as_str()?.to_string();
                    let source_id = row.get(1)?.as_str()?.to_string();
                    let project_code = row.get(2)?.as_str()?.to_string();
                    let content = row.get(3)?.as_str()?.to_string();
                    let chunk_part_index = Self::parse_usize_value(row.get(4)?).unwrap_or(1).max(1);
                    let chunk_part_count = Self::parse_usize_value(row.get(5)?).unwrap_or(1).max(1);
                    let chunk_path = row.get(6)?.as_str().unwrap_or("1/1").to_string();
                    let match_reason = row.get(7)?.as_str()?.to_string();
                    let uri = source_to_uri
                        .get(&source_id)
                        .cloned()
                        .or_else(|| {
                            if source_id.contains('/') {
                                Some(source_id.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();
                    let lexical_hits = terms
                        .iter()
                        .filter(|term| {
                            content.to_ascii_lowercase().contains(term.as_str())
                                || uri.to_ascii_lowercase().contains(term.as_str())
                        })
                        .count();
                    let anchored_to_entry = entry_ids.contains(&source_id);
                    let same_file_as_entry = entry_uris.contains(&uri);
                    Some(ChunkCandidate {
                        chunk_id,
                        source_id,
                        project_code,
                        uri,
                        content,
                        match_reason,
                        lexical_hits,
                        semantic_distance: None,
                        chunk_part_index,
                        chunk_part_count,
                        chunk_path,
                        anchored_to_entry,
                        same_file_as_entry,
                        score: 0.0,
                        reasons: Vec::new(),
                        fts_rank: None,
                    })
                })
                .collect::<Vec<_>>();
            if !anchored_candidates.is_empty() {
                return anchored_candidates;
            }
        }

        let project_filter =
            Self::sql_project_filter_for_fields(project, &["c.project_code", "f.project_code"]);

        // Vector dialect swap (PG vs DuckDB) — same table layout, same
        // filter clauses; only the cosine-distance expression differs.
        let cosine_expr = if let Some(embedding) = semantic.as_ref() {
            if is_pg {
                match crate::postgres::vector::vector_literal(embedding) {
                    Ok(lit) => Some(format!("(ce.embedding <=> {lit})")),
                    Err(err) => {
                        excluded_because.push(format!(
                            "pg_semantic_vector_literal_error:{}",
                            Self::truncate(&err.to_string(), 120)
                        ));
                        return Vec::new();
                    }
                }
            } else {
                let vector = format!("{embedding:?}");
                Some(format!(
                    "array_cosine_distance(ce.embedding, CAST({vector} AS FLOAT[{DIMENSION}]))"
                ))
            }
        } else {
            None
        };

        let query = if let Some(cosine_expr) = cosine_expr.as_ref() {
            format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), COALESCE(f.path, ''), c.content, \
                        COALESCE(c.chunk_part_index, 1), COALESCE(c.chunk_part_count, 1), COALESCE(c.chunk_path, '1/1'), \
                        CASE \
                            WHEN ({entry_id_match}) THEN 'entry_anchor' \
                            WHEN ({entry_uri_match}) THEN 'same_file' \
                            WHEN ({path_match}) THEN 'file_path' \
                            WHEN ({lexical_predicate}) THEN 'lexical+semantic' \
                        ELSE 'semantic' \
                        END, \
                        {cosine_expr} \
                 FROM Chunk c \
                 JOIN ChunkEmbedding ce ON ce.chunk_id = c.id AND ce.model_id = '{model_id}' AND ce.source_hash = c.content_hash \
                 LEFT JOIN CONTAINS rel ON rel.target_id = c.source_id \
                 LEFT JOIN File f ON f.path = rel.source_id \
                 WHERE (({entry_id_match}) OR ({entry_uri_match}) OR ({lexical_predicate}) OR ({lexical_uri_match}) OR ({path_match}) OR {cosine_expr} < 0.55){project_filter} \
                 ORDER BY {cosine_expr} ASC \
                 LIMIT {limit}",
                model_id = CHUNK_MODEL_ID,
            )
        } else {
            format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), COALESCE(f.path, ''), c.content, \
                        COALESCE(c.chunk_part_index, 1), COALESCE(c.chunk_part_count, 1), COALESCE(c.chunk_path, '1/1'), \
                        CASE \
                            WHEN ({entry_id_match}) THEN 'entry_anchor' \
                            WHEN ({entry_uri_match}) THEN 'same_file' \
                            WHEN ({path_match}) THEN 'file_path' \
                            ELSE 'lexical' \
                        END, \
                        NULL \
                 FROM Chunk c \
                 LEFT JOIN CONTAINS rel ON rel.target_id = c.source_id \
                 LEFT JOIN File f ON f.path = rel.source_id \
                 WHERE (({entry_id_match}) OR ({entry_uri_match}) OR ({lexical_predicate}) OR ({lexical_uri_match}) OR ({path_match})){project_filter} \
                 LIMIT {limit}",
            )
        };

        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let mut rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        if rows.is_empty() {
            // Repo-root fallback: drop project_code filter, post-filter
            // by repo_root prefix. Works identically on PG and DuckDB
            // since post-CPT-AXO-039 the table layout is the same.
            let _ = is_pg; // suppress unused-warn when fallback reached
            if let Some(repo_root) = Self::project_repo_root(project) {
                let fallback_query = query.replacen(
                    &Self::sql_project_filter_for_fields(
                        project,
                        &["c.project_code", "f.project_code"],
                    ),
                    "",
                    1,
                );
                let fallback_raw = self
                    .graph_store
                    .query_json(&fallback_query)
                    .unwrap_or_else(|_| "[]".to_string());
                let fallback_rows: Vec<Vec<Value>> =
                    serde_json::from_str(&fallback_raw).unwrap_or_default();
                rows = fallback_rows
                    .into_iter()
                    .filter(|row| {
                        row.get(3)
                            .and_then(|value| value.as_str())
                            .map(|uri| uri.starts_with(&repo_root))
                            .unwrap_or(false)
                    })
                    .collect();
            }
        }
        let mut candidates = rows
            .into_iter()
            .filter_map(|row| {
                let chunk_id = row.first()?.as_str()?.to_string();
                let source_id = row.get(1)?.as_str()?.to_string();
                let project_code = row.get(2)?.as_str()?.to_string();
                let uri = row.get(3)?.as_str().unwrap_or_default().to_string();
                let content = row.get(4)?.as_str()?.to_string();
                let chunk_part_index = Self::parse_usize_value(row.get(5)?).unwrap_or(1).max(1);
                let chunk_part_count = Self::parse_usize_value(row.get(6)?).unwrap_or(1).max(1);
                let chunk_path = row.get(7)?.as_str().unwrap_or("1/1").to_string();
                let match_reason = row.get(8)?.as_str()?.to_string();
                let semantic_distance = row.get(9).and_then(|value| value.as_f64());
                let lexical_hits = terms
                    .iter()
                    .filter(|term| {
                        content.to_ascii_lowercase().contains(term.as_str())
                            || uri.to_ascii_lowercase().contains(term.as_str())
                    })
                    .count();
                let anchored_to_entry = entry_ids.contains(&source_id);
                let same_file_as_entry = entry_uris.contains(&uri);
                Some(ChunkCandidate {
                    chunk_id,
                    source_id,
                    project_code,
                    uri,
                    content,
                    match_reason,
                    lexical_hits,
                    semantic_distance,
                    chunk_part_index,
                    chunk_part_count,
                    chunk_path,
                    anchored_to_entry,
                    same_file_as_entry,
                    score: 0.0,
                    reasons: Vec::new(),
                    fts_rank: None,
                })
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            let (_, repo_chunks) = self.repo_literal_fallback_candidates(project, terms, limit);
            candidates.extend(repo_chunks);
        }
        candidates
    }

    /// DEC-AXO-093 / REQ-AXO-324 — FTS modality for hybrid retrieval.
    ///
    /// Queries `public.Chunk.content_tsv` (GIN-indexed by MIL-AXO-017
    /// slice 4 / REQ-AXO-292) via `websearch_to_tsquery` so operators
    /// can pass natural-language questions, multi-word phrases, or
    /// boolean operators interchangeably. Ranked by `ts_rank_cd`
    /// which considers proximity + density of matches, not just
    /// presence.
    ///
    /// Returns empty when:
    /// - The backend is not PostgreSQL (FTS infrastructure is PG-only)
    /// - The env knob `AXON_IST_FTS_DISABLED=1` is set (rollback safety)
    /// - `websearch_to_tsquery` returns an empty tsquery (no hits)
    /// - The SQL fails (returns Vec::new(), no propagation)
    ///
    /// The caller merges these into the existing chunk candidate pool;
    /// the rerank step downstream decides final ordering. v1 ships as
    /// an additive candidate source; explicit RRF fusion across
    /// modalities is REQ-AXO-324 slice 2.
    fn find_chunk_candidates_via_fts(
        &self,
        project: Option<&str>,
        question: &str,
        limit: usize,
    ) -> Vec<ChunkCandidate> {
        if !self.graph_store.is_postgres_backend() {
            return Vec::new();
        }
        // Rollback knobs: legacy `AXON_IST_FTS_DISABLED` (slice 1)
        // stays for backwards-compat; new `AXON_HYBRID_RETRIEVAL_DISABLED`
        // (slice 2) is the canonical superset knob disabling the
        // whole hybrid path. Either one short-circuits FTS.
        let env_is_truthy = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false)
        };
        if env_is_truthy("AXON_IST_FTS_DISABLED") || env_is_truthy("AXON_HYBRID_RETRIEVAL_DISABLED") {
            return Vec::new();
        }
        let trimmed = question.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let escaped_question = Self::escape_sql(trimmed);
        // `public.Chunk` carries `file_path` directly — no need to
        // join the legacy `CONTAINS` table (which was retired by
        // MIL-AXO-017 slice 6 in favour of `public.Edge` with
        // relation_type='CONTAINS'). Filter on `c.project_code` only.
        let project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]);
        // websearch_to_tsquery with the `english` dictionary matches
        // the DDL's content-body indexing (postgres/ddl.rs:414 builds
        // content_tsv with `english` for content body, `simple` for
        // path/kind metadata). English stemming normalises
        // `recommendations`/`recommendation`/`recommend` to the same
        // lexeme and removes a handful of natural-language stop-words
        // (`how`/`the`/`a`) which is fine for question-style queries.
        // Identifiers like `soll_work_plan` are split on `_` by both
        // dictionaries so `soll`, `work`, `plan` lexemes still match.
        // ts_rank_cd uses cover density which favours dense matches.
        let query = format!(
            "WITH q AS (SELECT websearch_to_tsquery('english', '{q}') AS tsq) \
             SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), \
                    COALESCE(c.file_path, ''), c.content, \
                    COALESCE(c.chunk_part_index, 1), \
                    COALESCE(c.chunk_part_count, 1), \
                    COALESCE(c.chunk_path, '1/1'), \
                    ts_rank_cd(c.content_tsv, q.tsq) AS fts_score \
             FROM public.Chunk c \
             CROSS JOIN q \
             WHERE c.content_tsv @@ q.tsq AND q.tsq IS NOT NULL{project_filter} \
             ORDER BY ts_rank_cd(c.content_tsv, q.tsq) DESC \
             LIMIT {limit}",
            q = escaped_question,
        );
        let Ok(raw) = self.graph_store.query_json(&query) else {
            return Vec::new();
        };
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|row| {
                if row.len() < 9 {
                    return None;
                }
                let chunk_id = row.first()?.as_str()?.to_string();
                let source_id = row.get(1)?.as_str().unwrap_or("").to_string();
                let project_code = row.get(2)?.as_str().unwrap_or("unknown").to_string();
                let uri = row.get(3)?.as_str().unwrap_or("").to_string();
                let content = row.get(4)?.as_str().unwrap_or("").to_string();
                let chunk_part_index = row
                    .get(5)
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(1);
                let chunk_part_count = row
                    .get(6)
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(1);
                let chunk_path = row.get(7)?.as_str().unwrap_or("1/1").to_string();
                // SLICE 2 — the FTS signal lives in its own dedicated
                // `fts_rank` field (raw `ts_rank_cd`, ∈ [0, +∞), higher
                // is better) instead of being smuggled through
                // `semantic_distance` as a negated value. The rerank
                // step picks it up and gives FTS hits a dedicated
                // bonus band; `select_supporting_chunks` reserves
                // slots for them even when anchors exist.
                let fts_score = row.get(8).and_then(|v| v.as_f64()).unwrap_or(0.0);
                Some(ChunkCandidate {
                    chunk_id,
                    source_id,
                    project_code,
                    uri,
                    content,
                    match_reason: "fts".to_string(),
                    lexical_hits: 0,
                    semantic_distance: None,
                    chunk_part_index,
                    chunk_part_count,
                    chunk_path,
                    anchored_to_entry: false,
                    same_file_as_entry: false,
                    score: 0.0,
                    reasons: vec![format!("fts:ts_rank_cd={:.4}", fts_score)],
                    fts_rank: Some(fts_score),
                })
            })
            .collect()
    }

    fn rerank_chunk_candidates(
        &self,
        candidates: &mut [ChunkCandidate],
        route: RetrievalRoute,
        terms: &[String],
        entry_candidates: &[EntryCandidate],
        project_scope_variants: &[String],
        prefer_project_intent: bool,
        linked_evidence_first: bool,
    ) {
        let entry_uris = entry_candidates
            .iter()
            .map(|candidate| candidate.uri.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let scope_lc = project_scope_variants
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        for candidate in candidates.iter_mut() {
            let mut score = (candidate.lexical_hits as f64) * 1.5;
            if candidate.anchored_to_entry {
                score += 5.0;
                candidate.reasons.push("anchored_to_entry".to_string());
            } else if candidate.same_file_as_entry {
                score += 3.5;
                candidate.reasons.push("same_file_as_entry".to_string());
            }
            if let Some(distance) = candidate.semantic_distance {
                score += (1.0 - distance).max(0.0) * 3.0;
                candidate.reasons.push("semantic_chunk_match".to_string());
            }
            // DEC-AXO-093 / REQ-AXO-324 slice 2 — FTS modality bonus
            // band. `ts_rank_cd` typically sits in [0.01, 1.5] for
            // relevant matches. Multiplying by 4 and clamping at 6
            // places a strong FTS hit at the same magnitude as an
            // anchored hit (5.0), so FTS-found chunks compete on
            // equal footing rather than being structurally outranked
            // by anchor affinity.
            if let Some(fts_rank) = candidate.fts_rank {
                let bonus = (fts_rank * 4.0).min(6.0);
                score += bonus;
                candidate
                    .reasons
                    .push(format!("fts_rank_cd_match:{:.4}", fts_rank));
            }
            if candidate.chunk_part_count > 1 {
                score += 0.25;
                candidate.reasons.push("multipart_symbol_chunk".to_string());
                if candidate.chunk_part_index == 1 {
                    score += 0.6;
                    candidate.reasons.push("multipart_lead_chunk".to_string());
                } else if candidate.chunk_part_index == 2 {
                    score += 0.3;
                    candidate
                        .reasons
                        .push("multipart_adjacent_continuation_bonus".to_string());
                } else {
                    score -= 0.35;
                    candidate
                        .reasons
                        .push("multipart_late_chunk_penalty".to_string());
                }
            }
            if scope_lc.contains(&candidate.project_code.to_ascii_lowercase()) {
                score += 1.0;
                candidate.reasons.push("project_scope_match".to_string());
            }
            if matches!(route, RetrievalRoute::Hybrid | RetrievalRoute::SollHybrid) {
                score += 0.5;
            }
            if terms
                .iter()
                .any(|term| candidate.content.to_ascii_lowercase().contains(term))
            {
                score += 0.5;
                candidate.reasons.push("content_term_match".to_string());
            }
            if entry_uris.contains(&candidate.uri.to_ascii_lowercase()) {
                score += 1.0;
            }
            if prefer_project_intent {
                let intent_weight = Self::project_intent_doc_weight(&candidate.uri);
                if intent_weight > 0.0 {
                    score += intent_weight;
                    candidate
                        .reasons
                        .push("intent_canonical_plan_bonus".to_string());
                } else if intent_weight < 0.0 {
                    score += intent_weight;
                    candidate
                        .reasons
                        .push("intent_feedback_penalty".to_string());
                }
            }
            if linked_evidence_first
                && !candidate.anchored_to_entry
                && !candidate.same_file_as_entry
            {
                let canonical_doc_weight =
                    Self::canonical_project_doc_weight(&candidate.uri, project_scope_variants);
                if canonical_doc_weight > 0.0 {
                    score += canonical_doc_weight;
                    candidate
                        .reasons
                        .push("canonical_project_doc_bonus".to_string());
                }
                let workspace_noise_penalty = Self::workspace_noise_penalty(&candidate.uri);
                if workspace_noise_penalty < 0.0 {
                    score += workspace_noise_penalty;
                    candidate
                        .reasons
                        .push("workspace_noise_penalty".to_string());
                }
            }
            if Self::route_prefers_operational_code(route) {
                if let Some(reason) = Self::chunk_penalty_reason(candidate) {
                    score -= 2.0;
                    candidate.reasons.push(reason.to_string());
                }
            }
            // REQ-AXO-324 slice 2 — FTS hits are explicitly not
            // "generic semantic". Without the `fts_rank.is_none()`
            // guard, an FTS hit (lexical_hits=0, semantic_distance=None,
            // !anchored, !same_file, but fts_rank.is_some()) used to
            // pass this predicate and lose 1.0 — but now that FTS
            // carries its own bonus band the penalty would only fight
            // it. Keeps firing for true vector-only hits.
            if !candidate.anchored_to_entry
                && !candidate.same_file_as_entry
                && candidate.semantic_distance.is_some()
                && candidate.lexical_hits == 0
                && candidate.fts_rank.is_none()
            {
                score -= 1.0;
                candidate
                    .reasons
                    .push("generic_semantic_only_penalty".to_string());
            }
            candidate.score = score;
        }
        candidates.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.uri.cmp(&right.uri))
        });
    }

    fn select_supporting_chunks(
        &self,
        candidates: &[ChunkCandidate],
        entry_candidates: &[EntryCandidate],
        route: RetrievalRoute,
        top_k: usize,
        token_budget: usize,
        excluded_because: &mut Vec<String>,
        diagnostics: &mut RetrievalDiagnostics,
    ) -> Vec<Value> {
        let mut selected = Vec::new();
        let mut selected_ids = HashSet::new();
        let mut seen_uris = HashSet::new();
        let mut selected_source_parts: HashMap<String, Vec<usize>> = HashMap::new();
        let mut consumed_tokens = 0usize;
        let chunk_cap = top_k.min(4);
        let has_anchor = entry_candidates.iter().any(Self::is_strong_anchor);
        let prefers_operational_code = Self::route_prefers_operational_code(route);
        let mut broader_selected = 0usize;
        let mut non_operational_selected = 0usize;
        // DEC-AXO-093 / REQ-AXO-324 slice 2 — FTS-discovered chunks
        // get up to 2 reserved slots in the broader band even when
        // anchors exist. Without this, the hierarchical gate at
        // `has_anchor && !anchored_selected` (or the global cap
        // `broader_selected >= 1`) would systematically suppress
        // FTS hits and the dormant content_tsv GIN index would never
        // pay off. Gated by `AXON_HYBRID_RETRIEVAL_DISABLED` for
        // rollback safety.
        let hybrid_enabled = !std::env::var("AXON_HYBRID_RETRIEVAL_DISABLED")
            .ok()
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false);
        let fts_slot_cap: usize = if hybrid_enabled { 2 } else { 0 };
        let mut fts_selected = 0usize;
        diagnostics.fts_chunks_considered =
            candidates.iter().filter(|c| c.fts_rank.is_some()).count();

        let anchored = candidates
            .iter()
            .filter(|candidate| candidate.anchored_to_entry)
            .cloned()
            .collect::<Vec<_>>();
        let same_file = candidates
            .iter()
            .filter(|candidate| !candidate.anchored_to_entry && candidate.same_file_as_entry)
            .cloned()
            .collect::<Vec<_>>();
        let broader = candidates
            .iter()
            .filter(|candidate| !candidate.anchored_to_entry && !candidate.same_file_as_entry)
            .cloned()
            .collect::<Vec<_>>();

        let ingest = |candidate: &ChunkCandidate,
                      selected: &mut Vec<Value>,
                      selected_ids: &mut HashSet<String>,
                      seen_uris: &mut HashSet<String>,
                      selected_source_parts: &mut HashMap<String, Vec<usize>>,
                      consumed_tokens: &mut usize,
                      diagnostics: &mut RetrievalDiagnostics|
         -> bool {
            if selected.len() >= chunk_cap {
                return false;
            }
            if !selected_ids.insert(candidate.chunk_id.clone()) {
                return false;
            }
            if !Self::can_reuse_uri_for_multipart(candidate, seen_uris, selected_source_parts) {
                return false;
            }
            let snippet = Self::truncate(&candidate.content, 220);
            let estimated = Self::estimate_tokens(&[&snippet]);
            if *consumed_tokens + estimated > token_budget / 2 {
                return false;
            }
            *consumed_tokens += estimated;
            seen_uris.insert(candidate.uri.clone());
            selected_source_parts
                .entry(candidate.source_id.clone())
                .or_default()
                .push(candidate.chunk_part_index);
            if candidate.anchored_to_entry || candidate.same_file_as_entry {
                diagnostics.anchored_chunks_selected += 1;
            } else {
                diagnostics.unanchored_chunks_selected += 1;
            }
            if candidate.chunk_part_count > 1 {
                diagnostics.multipart_chunks_selected += 1;
            }
            if candidate.fts_rank.is_some() {
                diagnostics.fts_chunks_selected += 1;
            }
            selected.push(json!({
                "chunk_id": candidate.chunk_id,
                "source_id": candidate.source_id,
                "project_code": candidate.project_code,
                "uri": candidate.uri,
                "match_reason": candidate.match_reason,
                "evidence_class": "derived_chunk",
                "chunk_path": candidate.chunk_path,
                "chunk_part_index": candidate.chunk_part_index,
                "chunk_part_count": candidate.chunk_part_count,
                "anchored_to_entry": candidate.anchored_to_entry,
                "same_file_as_entry": candidate.same_file_as_entry,
                "snippet": snippet,
                "score": candidate.score,
                "ranking_reasons": candidate.reasons,
            }));
            true
        };

        for candidate in &anchored {
            ingest(
                candidate,
                &mut selected,
                &mut selected_ids,
                &mut seen_uris,
                &mut selected_source_parts,
                &mut consumed_tokens,
                diagnostics,
            );
        }
        for candidate in &same_file {
            ingest(
                candidate,
                &mut selected,
                &mut selected_ids,
                &mut seen_uris,
                &mut selected_source_parts,
                &mut consumed_tokens,
                diagnostics,
            );
        }

        let anchored_selected = diagnostics.anchored_chunks_selected > 0;
        if has_anchor && !anchored.is_empty() && !anchored_selected {
            excluded_because.push("anchored_chunks_over_budget".to_string());
            return selected;
        }

        for candidate in &broader {
            let is_fts = candidate.fts_rank.is_some();
            // SLICE 2 — FTS hits bypass the anchor-affinity gate up
            // to `fts_slot_cap` (= 2 when hybrid enabled). Non-FTS
            // broader candidates still respect the original
            // `has_anchor && !anchored_selected` short-circuit.
            if has_anchor && !anchored_selected && !(is_fts && fts_selected < fts_slot_cap) {
                excluded_because.push("not_anchor_affine".to_string());
                continue;
            }
            if has_anchor && prefers_operational_code && !is_fts {
                if let Some(reason) = Self::chunk_penalty_reason(candidate) {
                    excluded_because.push(reason.to_string());
                    if reason != "test_file_penalty" && reason != "docs_file_penalty" {
                        excluded_because.push("non_operational_chunk_penalized".to_string());
                    }
                    continue;
                }
            }
            // SLICE 2 — global broader cap respected for non-FTS;
            // FTS gets its own dedicated `fts_slot_cap` budget that
            // does not count against `broader_selected`.
            if !is_fts && broader_selected >= 1 {
                excluded_because.push("broader_semantic_dropped_due_to_anchor".to_string());
                continue;
            }
            if is_fts && fts_selected >= fts_slot_cap {
                excluded_because.push("fts_slot_cap_exhausted".to_string());
                continue;
            }
            if !is_fts && candidate.semantic_distance.is_some() && candidate.lexical_hits == 0 {
                excluded_because.push("generic_semantic_only".to_string());
            }
            if prefers_operational_code
                && Self::chunk_penalty_reason(candidate).is_some()
                && !is_fts
            {
                if non_operational_selected >= 1 {
                    excluded_because.push("non_operational_chunk_penalized".to_string());
                    continue;
                }
                non_operational_selected += 1;
            }
            let ingested = ingest(
                candidate,
                &mut selected,
                &mut selected_ids,
                &mut seen_uris,
                &mut selected_source_parts,
                &mut consumed_tokens,
                diagnostics,
            );
            if ingested {
                if is_fts {
                    fts_selected += 1;
                } else {
                    broader_selected += 1;
                }
            }
        }

        if prefers_operational_code
            && !same_file.is_empty()
            && broader_selected > 0
            && diagnostics.anchored_chunks_selected > 0
        {
            excluded_because.push("same_file_preferred".to_string());
        }

        diagnostics.multipart_symbol_groups_selected = selected_source_parts
            .values()
            .filter(|parts| parts.len() > 1)
            .count();

        selected
    }

    fn collect_structural_neighbors(
        &self,
        entry_candidates: &[EntryCandidate],
        route: RetrievalRoute,
    ) -> Vec<Value> {
        let radius = if matches!(route, RetrievalRoute::Impact) {
            2
        } else {
            1
        };
        let mut selected = Vec::new();
        let mut seen = HashSet::new();

        for anchor in entry_candidates.iter().take(2) {
            if anchor.kind == "file" {
                continue;
            }
            let Ok(Some(anchor_id)) = self
                .graph_store
                .refresh_symbol_projection(&anchor.id, radius)
            else {
                continue;
            };
            let raw = self
                .graph_store
                .query_graph_projection("symbol", &anchor_id, radius)
                .unwrap_or_else(|_| "[]".to_string());
            let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                let Some(target_id) = row.get(1).and_then(|value| value.as_str()) else {
                    continue;
                };
                let edge_kind = row
                    .get(2)
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                if target_id == anchor.id || edge_kind == "anchor" {
                    continue;
                }
                let key = format!("{}:{target_id}", anchor.id);
                if !seen.insert(key) {
                    continue;
                }
                selected.push(json!({
                    "anchor_symbol": anchor.name,
                    "target_type": row.first().and_then(|value| value.as_str()).unwrap_or("unknown"),
                    "target_id": target_id,
                    "edge_kind": edge_kind,
                    "distance": row.get(3).and_then(|value| value.as_i64()).unwrap_or(0),
                    "label": row.get(4).and_then(|value| value.as_str()).unwrap_or(target_id),
                    "uri": row.get(5).and_then(|value| value.as_str()).unwrap_or(""),
                    "evidence_class": "derived_graph_projection",
                }));
                if selected.len() >= 2 {
                    return selected;
                }
            }
        }

        selected
    }

    fn has_direct_soll_traceability(
        &self,
        entry_candidates: &[EntryCandidate],
        project: Option<&str>,
    ) -> bool {
        let symbol_names = entry_candidates
            .iter()
            .filter(|candidate| candidate.kind != "file")
            .map(|candidate| {
                format!(
                    "'{}'",
                    Self::escape_sql(&candidate.name.to_ascii_lowercase())
                )
            })
            .collect::<Vec<_>>();
        let file_paths = entry_candidates
            .iter()
            .filter(|candidate| !candidate.uri.is_empty())
            .map(|candidate| format!("'{}'", Self::escape_sql(&candidate.uri)))
            .collect::<Vec<_>>();
        let project_filter = project
            .map(|value| {
                format!(
                    " AND lower(n.project_code) IN ({})",
                    Self::project_scope_variants(Some(value))
                        .iter()
                        .map(|variant| format!(
                            "'{}'",
                            Self::escape_sql(&variant.to_ascii_lowercase())
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_default();
        let mut predicates = Vec::new();
        if !symbol_names.is_empty() {
            predicates.push(format!(
                "(t.artifact_type = 'Symbol' AND lower(t.artifact_ref) IN ({}))",
                symbol_names.join(",")
            ));
        }
        if !file_paths.is_empty() {
            predicates.push(format!(
                "(t.artifact_type = 'File' AND t.artifact_ref IN ({}))",
                file_paths.join(",")
            ));
        }
        if predicates.is_empty() {
            return false;
        }
        let query = format!(
            "SELECT count(*) FROM soll.Traceability t \
             JOIN soll.Node n ON n.id = t.soll_entity_id \
             WHERE ({predicates}){project_filter}",
            predicates = predicates.join(" OR "),
        );
        self.graph_store.query_count(&query).unwrap_or(0) > 0
    }

    fn collect_soll_entities(
        &self,
        entry_candidates: &[EntryCandidate],
        project: Option<&str>,
        terms: &[String],
        top_k: usize,
    ) -> Vec<Value> {
        let symbol_names = entry_candidates
            .iter()
            .filter(|candidate| candidate.kind != "file")
            .map(|candidate| {
                format!(
                    "'{}'",
                    Self::escape_sql(&candidate.name.to_ascii_lowercase())
                )
            })
            .collect::<Vec<_>>();
        let file_paths = entry_candidates
            .iter()
            .filter(|candidate| !candidate.uri.is_empty())
            .map(|candidate| format!("'{}'", Self::escape_sql(&candidate.uri)))
            .collect::<Vec<_>>();
        if symbol_names.is_empty() && file_paths.is_empty() {
            return Vec::new();
        }

        let project_filter = project
            .map(|value| {
                format!(
                    " AND lower(n.project_code) IN ({})",
                    Self::project_scope_variants(Some(value))
                        .iter()
                        .map(|variant| format!(
                            "'{}'",
                            Self::escape_sql(&variant.to_ascii_lowercase())
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_default();
        let mut predicates = Vec::new();
        if !symbol_names.is_empty() {
            predicates.push(format!(
                "(t.artifact_type = 'Symbol' AND lower(t.artifact_ref) IN ({}))",
                symbol_names.join(",")
            ));
        }
        if !file_paths.is_empty() {
            predicates.push(format!(
                "(t.artifact_type = 'File' AND t.artifact_ref IN ({}))",
                file_paths.join(",")
            ));
        }
        let query = format!(
            "SELECT n.id, n.type, COALESCE(n.title, ''), COALESCE(e.relation_type, ''), \
                    COALESCE(t.artifact_ref, ''), t.artifact_type, \
                    CASE \
                        WHEN t.artifact_type = 'Symbol' THEN 'direct_symbol_traceability' \
                        WHEN t.artifact_type = 'File' THEN 'direct_file_traceability' \
                        WHEN e.relation_type = 'SOLVES' THEN 'requirement_support' \
                        ELSE 'decision_proximity' \
                    END AS ranking_reason, \
                    CASE \
                        WHEN t.artifact_type = 'Symbol' THEN 100 \
                        WHEN t.artifact_type = 'File' THEN 95 \
                        WHEN n.type = 'Decision' THEN 80 \
                        WHEN n.type = 'Requirement' THEN 70 \
                        ELSE 50 \
                    END AS ranking_score \
             FROM soll.Traceability t \
             JOIN soll.Node n ON n.id = t.soll_entity_id \
             LEFT JOIN soll.Edge e ON e.source_id = n.id \
             WHERE ({predicates}){project_filter} \
             ORDER BY ranking_score DESC, n.type DESC, n.id ASC \
             LIMIT {limit}",
            predicates = predicates.join(" OR "),
            limit = top_k.min(2),
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut selected = rows
            .into_iter()
            .filter_map(|row| {
                Some(json!({
                    "id": row.first()?.as_str()?.to_string(),
                    "type": row.get(1)?.as_str()?.to_string(),
                    "title": row.get(2)?.as_str().unwrap_or_default().to_string(),
                    "relation_type": row.get(3)?.as_str().unwrap_or_default().to_string(),
                    "source_symbol": row.get(4)?.as_str().unwrap_or_default().to_string(),
                    "artifact_type": row.get(5)?.as_str().unwrap_or_default().to_string(),
                    "ranking_reasons": [row.get(6)?.as_str().unwrap_or_default().to_string()],
                    "ranking_score": row.get(7)?.as_i64().unwrap_or_default(),
                    "evidence_class": "soll_traceability",
                }))
            })
            .collect::<Vec<_>>();

        self.expand_concept_governing_entities(&mut selected, project, top_k);
        if !selected.is_empty() {
            return selected;
        }

        let filtered_terms = terms
            .iter()
            .filter(|term| term.len() >= 4)
            .cloned()
            .collect::<Vec<_>>();
        if filtered_terms.is_empty() {
            return Vec::new();
        }

        let project_filter = project
            .map(|value| {
                format!(
                    " AND lower(n.project_code) IN ({})",
                    Self::project_scope_variants(Some(value))
                        .iter()
                        .map(|variant| format!(
                            "'{}'",
                            Self::escape_sql(&variant.to_ascii_lowercase())
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_default();
        let lexical_predicate = filtered_terms
            .iter()
            .map(|term| {
                format!(
                    "(lower(n.title) LIKE '%{t}%' OR lower(COALESCE(n.description, '')) LIKE '%{t}%')",
                    t = Self::escape_sql(term)
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        let fallback_query = format!(
            "SELECT n.id, n.type, COALESCE(n.title, ''), \
                    CASE \
                        WHEN n.type = 'Requirement' THEN 'direct_requirement_match' \
                        WHEN n.type = 'Decision' THEN 'direct_decision_match' \
                        ELSE 'direct_intent_match' \
                    END AS ranking_reason, \
                    CASE \
                        WHEN n.type = 'Requirement' THEN 95 \
                        WHEN n.type = 'Decision' THEN 90 \
                        WHEN n.type = 'Concept' THEN 80 \
                        ELSE 70 \
                    END AS ranking_score
             FROM soll.Node n
             WHERE ({lexical_predicate}){project_filter}
             ORDER BY ranking_score DESC, n.id ASC
             LIMIT {limit}",
            limit = top_k.min(4),
        );
        let fallback_raw = self
            .graph_store
            .query_json(&fallback_query)
            .unwrap_or_else(|_| "[]".to_string());
        let fallback_rows: Vec<Vec<Value>> =
            serde_json::from_str(&fallback_raw).unwrap_or_default();
        selected.extend(fallback_rows.into_iter().filter_map(|row| {
            Some(json!({
                "id": row.first()?.as_str()?.to_string(),
                "type": row.get(1)?.as_str()?.to_string(),
                "title": row.get(2)?.as_str().unwrap_or_default().to_string(),
                "relation_type": "",
                "source_symbol": "",
                "artifact_type": "",
                "ranking_reasons": [row.get(3)?.as_str().unwrap_or_default().to_string()],
                "ranking_score": row.get(4)?.as_i64().unwrap_or_default(),
                "evidence_class": "soll_lexical_fallback",
            }))
        }));
        self.expand_concept_governing_entities(&mut selected, project, top_k);
        selected
    }

    fn expand_concept_governing_entities(
        &self,
        selected: &mut Vec<Value>,
        project: Option<&str>,
        top_k: usize,
    ) {
        let concept_ids = selected
            .iter()
            .filter(|row| row.get("type").and_then(|value| value.as_str()) == Some("Concept"))
            .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
            .map(str::to_string)
            .collect::<Vec<_>>();
        if concept_ids.is_empty() {
            return;
        }

        let mut seen_ids = selected
            .iter()
            .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
            .map(str::to_string)
            .collect::<HashSet<_>>();
        let project_filter = project
            .map(|value| {
                format!(
                    " AND lower(n.project_code) IN ({})",
                    Self::project_scope_variants(Some(value))
                        .iter()
                        .map(|variant| format!(
                            "'{}'",
                            Self::escape_sql(&variant.to_ascii_lowercase())
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_default();
        let concept_ids_sql = concept_ids
            .iter()
            .map(|id| format!("'{}'", Self::escape_sql(id)))
            .collect::<Vec<_>>()
            .join(", ");

        let requirement_query = format!(
            "SELECT DISTINCT n.id, n.type, COALESCE(n.title, ''), COALESCE(e.relation_type, ''), \
                    c.id AS source_symbol, '' AS artifact_type, \
                    'concept_requirement_bridge' AS ranking_reason, \
                    88 AS ranking_score \
             FROM soll.Node c \
             JOIN soll.Edge e ON e.source_id = c.id \
             JOIN soll.Node n ON n.id = e.target_id \
             WHERE c.id IN ({concept_ids_sql}) AND n.type = 'Requirement'{project_filter} \
             ORDER BY ranking_score DESC, n.id ASC \
             LIMIT {limit}",
            limit = top_k.min(4),
        );
        let decision_project_filter = project
            .map(|value| {
                format!(
                    " AND lower(d.project_code) IN ({})",
                    Self::project_scope_variants(Some(value))
                        .iter()
                        .map(|variant| format!(
                            "'{}'",
                            Self::escape_sql(&variant.to_ascii_lowercase())
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_default();
        let decision_query = format!(
            "SELECT DISTINCT d.id, d.type, COALESCE(d.title, ''), COALESCE(de.relation_type, ''), \
                    c.id AS source_symbol, '' AS artifact_type, \
                    'concept_decision_bridge' AS ranking_reason, \
                    84 AS ranking_score \
             FROM soll.Node c \
             JOIN soll.Edge ce ON ce.source_id = c.id \
             JOIN soll.Node r ON r.id = ce.target_id AND r.type = 'Requirement' \
             JOIN soll.Edge de ON de.target_id = r.id \
             JOIN soll.Node d ON d.id = de.source_id \
             WHERE c.id IN ({concept_ids_sql}) AND d.type = 'Decision'{decision_project_filter} \
             ORDER BY ranking_score DESC, d.id ASC \
             LIMIT {limit}",
            limit = top_k.min(4),
        );

        for query in [requirement_query, decision_query] {
            let raw = self
                .graph_store
                .query_json(&query)
                .unwrap_or_else(|_| "[]".to_string());
            let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                let Some(id) = row.first().and_then(|value| value.as_str()) else {
                    continue;
                };
                if !seen_ids.insert(id.to_string()) {
                    continue;
                }
                selected.push(json!({
                    "id": id.to_string(),
                    "type": row.get(1).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "title": row.get(2).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "relation_type": row.get(3).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "source_symbol": row.get(4).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "artifact_type": row.get(5).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "ranking_reasons": [row.get(6).and_then(|value| value.as_str()).unwrap_or_default().to_string()],
                    "ranking_score": row.get(7).and_then(|value| value.as_i64()).unwrap_or_default(),
                    "evidence_class": "soll_concept_bridge",
                }));
            }
        }
    }

    fn build_answer_sketch(
        &self,
        question: &str,
        route: RetrievalRoute,
        entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value],
        structural_neighbors: &[Value],
        governing_requirements: &[Value],
        governing_decisions: &[Value],
        supporting_guidelines: &[Value],
        evidence_states: &[Value],
    ) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Route `{}` selected for `{}`.",
            route.as_str(),
            question
        ));
        if let Some(anchor) = entry_candidates.first() {
            lines.push(format!(
                "Primary entrypoint: `{}` ({}) in `{}`.",
                anchor.name, anchor.kind, anchor.uri
            ));
        }
        if !structural_neighbors.is_empty() {
            let labels = structural_neighbors
                .iter()
                .filter_map(|row| row.get("label").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Bounded structural expansion found: {}.", labels));
        }
        if !supporting_chunks.is_empty() {
            lines.push(format!(
                "{} supporting chunk(s) added for grounded detail.",
                supporting_chunks.len()
            ));
        }
        if !governing_requirements.is_empty() {
            let ids = governing_requirements
                .iter()
                .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            let label = if governing_requirements
                .iter()
                .all(|row| row.get("link_mode").and_then(|value| value.as_str()) == Some("direct"))
            {
                "Direct governing requirement(s)"
            } else {
                "Governing requirement(s) inferred from supporting intent"
            };
            lines.push(format!("{label}: {}.", ids));
        }
        if !governing_decisions.is_empty() {
            let ids = governing_decisions
                .iter()
                .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            let label = if governing_decisions
                .iter()
                .all(|row| row.get("link_mode").and_then(|value| value.as_str()) == Some("direct"))
            {
                "Direct governing decision(s)"
            } else {
                "Governing decision(s) inferred from supporting intent"
            };
            lines.push(format!("{label}: {}.", ids));
        }
        if !supporting_guidelines.is_empty() {
            let ids = supporting_guidelines
                .iter()
                .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Supporting guideline(s): {}.", ids));
        }
        if evidence_states.iter().any(|row| {
            row.get("state").and_then(|value| value.as_str()) == Some("missing_governing_intent")
        }) {
            lines.push("No direct governing intent was found for this symbol.".to_string());
        }
        if evidence_states
            .iter()
            .any(|row| row.get("state").and_then(|value| value.as_str()) == Some("support_only"))
        {
            lines.push("Current rationale is supported by local evidence only and should not be treated as canonical intent.".to_string());
        }
        lines.join(" ")
    }

    fn build_direct_evidence(&self, entry_candidates: &[EntryCandidate]) -> Vec<Value> {
        entry_candidates
            .iter()
            .map(|candidate| {
                let evidence_class = if candidate.kind == "file" {
                    "canonical_file"
                } else if candidate.kind == "repo_literal" {
                    "repo_literal_file"
                } else {
                    "canonical_symbol"
                };
                json!({
                    "symbol_id": candidate.id,
                    "name": candidate.name,
                    "kind": candidate.kind,
                    "project_code": candidate.project_code,
                    "uri": candidate.uri,
                    "evidence_class": evidence_class,
                    "score": candidate.score,
                    "ranking_reasons": candidate.reasons,
                })
            })
            .collect()
    }

    fn build_why_these_items(
        &self,
        route: RetrievalRoute,
        entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value],
        structural_neighbors: &[Value],
        relevant_soll_entities: &[Value],
    ) -> Vec<Value> {
        let mut items = Vec::new();
        if !entry_candidates.is_empty() {
            items.push(json!({
                "reason": "entrypoints_selected",
                "detail": format!("{} entrypoint(s) selected for route {}", entry_candidates.len(), route.as_str())
            }));
        }
        if !supporting_chunks.is_empty() {
            items.push(json!({
                "reason": "grounding_chunks_selected",
                "detail": format!("{} supporting chunk(s) chosen under diversity and budget constraints", supporting_chunks.len())
            }));
        }
        if !structural_neighbors.is_empty() {
            items.push(json!({
                "reason": "bounded_graph_expansion",
                "detail": format!("{} structural neighbor(s) retained from derived graph projection", structural_neighbors.len())
            }));
        }
        if !relevant_soll_entities.is_empty() {
            items.push(json!({
                "reason": "soll_join_materially_helpful",
                "detail": format!("{} SOLL item(s) joined because the route requested rationale/intent", relevant_soll_entities.len())
            }));
        }
        items
    }

    fn build_missing_evidence(
        &self,
        route: RetrievalRoute,
        entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value],
        relevant_soll_entities: &[Value],
        rationale_requested: bool,
        has_direct_traceability: bool,
        semantic_search_used: bool,
        degraded_reason: Option<&str>,
    ) -> Vec<Value> {
        let mut missing = Vec::new();
        if entry_candidates.is_empty() {
            missing.push(json!({"type": "entrypoint", "detail": "No strong symbol or file entrypoint was found"}));
        } else if supporting_chunks.is_empty() {
            missing.push(json!({"type": "supporting_chunks", "detail": "An anchor was found but no anchored chunk-level grounding evidence was retained"}));
        }
        if !semantic_search_used {
            if let Some(reason) = degraded_reason {
                missing.push(json!({"type": "semantic_search", "detail": format!("Semantic chunk search was skipped or unavailable: {}", reason)}));
            }
        }
        if !entry_candidates.is_empty()
            && !has_direct_traceability
            && (matches!(route, RetrievalRoute::SollHybrid) || rationale_requested)
        {
            missing.push(json!({"type": "anchor_found_but_no_traceability", "detail": "A structural anchor was found but no direct Symbol/File traceability matched it"}));
        }
        if matches!(route, RetrievalRoute::SollHybrid) && relevant_soll_entities.is_empty() {
            missing.push(json!({"type": "soll_intent", "detail": "SOLL rationale was requested but no direct traceability or intentional evidence was found"}));
        } else if rationale_requested && relevant_soll_entities.is_empty() {
            missing.push(json!({"type": "rationale_requested_but_no_intent_evidence", "detail": "The question requested rationale, but no intentional evidence was available after anchored retrieval"}));
        }
        missing
    }

    fn classify_governing_entities(
        entities: &[Value],
        expected_type: &str,
        provenance: &str,
    ) -> Vec<Value> {
        entities
            .iter()
            .filter(|row| row.get("type").and_then(|value| value.as_str()) == Some(expected_type))
            .map(|row| {
                let evidence_class = row
                    .get("evidence_class")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let ranking_reason = row
                    .get("ranking_reasons")
                    .and_then(|value| value.as_array())
                    .and_then(|items| items.first())
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let link_mode = if evidence_class == "soll_traceability"
                    && (ranking_reason.starts_with("direct_")
                        || ranking_reason.starts_with("requirement_"))
                {
                    "direct"
                } else if evidence_class == "soll_traceability"
                    || evidence_class == "soll_concept_bridge"
                {
                    "inferred"
                } else {
                    "weak_correlation"
                };
                let authority_class = if link_mode == "weak_correlation" {
                    "correlated"
                } else if expected_type == "Guideline" {
                    "supporting"
                } else {
                    "governing"
                };
                let mut enriched = row.clone();
                if let Some(object) = enriched.as_object_mut() {
                    object.insert(
                        "authority_class".to_string(),
                        Value::String(authority_class.to_string()),
                    );
                    object.insert(
                        "evidence_provenance".to_string(),
                        Value::String(provenance.to_string()),
                    );
                    object.insert(
                        "link_mode".to_string(),
                        Value::String(link_mode.to_string()),
                    );
                    object.insert(
                        "inclusion_reason".to_string(),
                        Value::String(ranking_reason.to_string()),
                    );
                }
                enriched
            })
            .collect()
    }

    fn evidence_provenance_for_uri(uri: &str) -> &'static str {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("benchmark") {
            "benchmark"
        } else if matches!(Self::uri_penalty_reason(uri), Some("test_file_penalty")) {
            "test"
        } else if lower.contains("/scripts/") || lower.starts_with("scripts/") {
            "script"
        } else if matches!(Self::uri_penalty_reason(uri), Some("docs_file_penalty")) {
            "doc"
        } else {
            "code_chunk"
        }
    }

    fn classify_direct_code_evidence(direct_evidence: &[Value]) -> Vec<Value> {
        direct_evidence
            .iter()
            .map(|row| {
                let mut enriched = row.clone();
                let kind = row
                    .get("kind")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let uri = row
                    .get("uri")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let evidence_class = row
                    .get("evidence_class")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let uri_provenance = Self::evidence_provenance_for_uri(uri);
                let provenance = if evidence_class == "repo_literal_file" {
                    uri_provenance
                } else if matches!(uri_provenance, "benchmark" | "test" | "script" | "doc") {
                    uri_provenance
                } else if kind == "file" {
                    "code_file"
                } else {
                    "code_symbol"
                };
                let authority_class = match provenance {
                    "benchmark" | "test" | "script" | "doc" => "correlated",
                    _ => "supporting",
                };
                let link_mode = if evidence_class == "repo_literal_file" {
                    "weak_correlation"
                } else {
                    "direct"
                };
                if let Some(object) = enriched.as_object_mut() {
                    object.insert(
                        "authority_class".to_string(),
                        Value::String(authority_class.to_string()),
                    );
                    object.insert(
                        "evidence_provenance".to_string(),
                        Value::String(provenance.to_string()),
                    );
                    object.insert(
                        "link_mode".to_string(),
                        Value::String(link_mode.to_string()),
                    );
                    object.insert(
                        "inclusion_reason".to_string(),
                        Value::String(
                            row.get("evidence_class")
                                .and_then(|value| value.as_str())
                                .unwrap_or("direct_evidence")
                                .to_string(),
                        ),
                    );
                }
                enriched
            })
            .collect()
    }

    fn classify_supporting_chunks_by_provenance(
        chunks: &[Value],
        provenance: &str,
        authority_class: &str,
    ) -> Vec<Value> {
        chunks
            .iter()
            .filter_map(|row| {
                let uri = row
                    .get("uri")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let row_provenance = Self::evidence_provenance_for_uri(uri);
                (row_provenance == provenance).then(|| {
                    let mut enriched = row.clone();
                    let link_mode = match row
                        .get("anchored_to_entry")
                        .and_then(|value| value.as_bool())
                    {
                        Some(true) => "direct",
                        _ => "inferred",
                    };
                    if let Some(object) = enriched.as_object_mut() {
                        object.insert(
                            "authority_class".to_string(),
                            Value::String(authority_class.to_string()),
                        );
                        object.insert(
                            "evidence_provenance".to_string(),
                            Value::String(provenance.to_string()),
                        );
                        object.insert(
                            "link_mode".to_string(),
                            Value::String(link_mode.to_string()),
                        );
                        object.insert(
                            "inclusion_reason".to_string(),
                            Value::String(
                                row.get("match_reason")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("supporting_chunk")
                                    .to_string(),
                            ),
                        );
                    }
                    enriched
                })
            })
            .collect()
    }

    fn classify_supporting_code_context(chunks: &[Value], neighbors: &[Value]) -> Vec<Value> {
        let mut items = chunks
            .iter()
            .filter_map(|row| {
                let uri = row
                    .get("uri")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let provenance = Self::evidence_provenance_for_uri(uri);
                (provenance != "doc").then(|| {
                    let mut enriched = row.clone();
                    let link_mode = if matches!(provenance, "benchmark" | "test" | "script") {
                        "weak_correlation"
                    } else if row
                        .get("anchored_to_entry")
                        .and_then(|value| value.as_bool())
                        == Some(true)
                    {
                        "direct"
                    } else {
                        "inferred"
                    };
                    let authority_class = if link_mode == "weak_correlation" {
                        "correlated"
                    } else {
                        "supporting"
                    };
                    if let Some(object) = enriched.as_object_mut() {
                        object.insert(
                            "authority_class".to_string(),
                            Value::String(authority_class.to_string()),
                        );
                        object.insert(
                            "evidence_provenance".to_string(),
                            Value::String(provenance.to_string()),
                        );
                        object.insert(
                            "link_mode".to_string(),
                            Value::String(link_mode.to_string()),
                        );
                        object.insert(
                            "inclusion_reason".to_string(),
                            Value::String(
                                row.get("match_reason")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("supporting_chunk")
                                    .to_string(),
                            ),
                        );
                    }
                    enriched
                })
            })
            .collect::<Vec<_>>();

        for neighbor in neighbors {
            let mut enriched = neighbor.clone();
            if let Some(object) = enriched.as_object_mut() {
                object.insert(
                    "authority_class".to_string(),
                    Value::String("supporting".to_string()),
                );
                object.insert(
                    "evidence_provenance".to_string(),
                    Value::String("code_chunk".to_string()),
                );
                object.insert(
                    "link_mode".to_string(),
                    Value::String("inferred".to_string()),
                );
                object.insert(
                    "inclusion_reason".to_string(),
                    Value::String(
                        neighbor
                            .get("edge_kind")
                            .and_then(|value| value.as_str())
                            .unwrap_or("structural_neighbor")
                            .to_string(),
                    ),
                );
            }
            items.push(enriched);
        }

        items
    }

    #[allow(clippy::too_many_arguments)]
    fn build_evidence_states(
        route: RetrievalRoute,
        rationale_requested: bool,
        has_direct_traceability: bool,
        degraded_reason: Option<&str>,
        governing_requirements: &[Value],
        governing_decisions: &[Value],
        supporting_guidelines: &[Value],
        direct_code_evidence: &[Value],
        supporting_docs: &[Value],
        supporting_code_context: &[Value],
    ) -> Vec<Value> {
        let mut states = Vec::new();
        let has_governing = !governing_requirements.is_empty() || !governing_decisions.is_empty();
        let has_support = !supporting_guidelines.is_empty()
            || !direct_code_evidence.is_empty()
            || !supporting_docs.is_empty()
            || !supporting_code_context.is_empty();
        if (matches!(route, RetrievalRoute::SollHybrid) || rationale_requested) && !has_governing {
            states.push(json!({
                "state": "missing_governing_intent",
                "severity": "medium",
                "detail": "No direct governing requirement or decision was found for this rationale request"
            }));
        }
        if !has_direct_traceability
            && (matches!(route, RetrievalRoute::SollHybrid) || rationale_requested)
        {
            states.push(json!({
                "state": "no_direct_traceability",
                "severity": "medium",
                "detail": "No direct Symbol/File traceability was found for the current anchor"
            }));
        }
        if degraded_reason.is_some() {
            states.push(json!({
                "state": "retrieval_degraded",
                "severity": "low",
                "detail": degraded_reason
                    .map(|value| format!("Retrieval ran under degraded conditions: {value}"))
                    .unwrap_or_else(|| "Retrieval ran under degraded conditions".to_string())
            }));
        }
        if !has_governing && has_support {
            let only_correlated = direct_code_evidence
                .iter()
                .chain(supporting_docs.iter())
                .chain(supporting_code_context.iter())
                .chain(supporting_guidelines.iter())
                .all(|row| {
                    row.get("authority_class").and_then(|value| value.as_str())
                        == Some("correlated")
                });
            states.push(json!({
                "state": if only_correlated { "correlation_only" } else { "support_only" },
                "severity": "medium",
                "detail": if only_correlated {
                    "Only correlated support artifacts were available for this rationale packet"
                } else {
                    "Only supporting local evidence was available for this rationale packet"
                }
            }));
        }
        states
    }

    fn build_rationale_quality(
        evidence_states: &[Value],
        governing_requirements: &[Value],
        governing_decisions: &[Value],
        supporting_guidelines: &[Value],
    ) -> Value {
        let has_governing = !governing_requirements.is_empty() || !governing_decisions.is_empty();
        let has_missing_governing = evidence_states.iter().any(|row| {
            row.get("state").and_then(|value| value.as_str()) == Some("missing_governing_intent")
        });
        let has_no_direct_traceability = evidence_states.iter().any(|row| {
            row.get("state").and_then(|value| value.as_str()) == Some("no_direct_traceability")
        });
        let has_correlation_only = evidence_states.iter().any(|row| {
            row.get("state").and_then(|value| value.as_str()) == Some("correlation_only")
        });
        let level = if has_governing && evidence_states.is_empty() {
            "strong"
        } else if has_missing_governing || has_no_direct_traceability || has_correlation_only {
            "weak"
        } else if has_governing || !supporting_guidelines.is_empty() {
            "mixed"
        } else {
            "weak"
        };
        let confidence_reason = if has_missing_governing {
            "governing intent is missing, so the packet should be read as non-canonical rationale"
        } else if has_no_direct_traceability {
            "supporting evidence exists, but no direct traceability was found for the current anchor"
        } else if has_governing {
            "governing intent is present, but downstream support may still be partial"
        } else if has_correlation_only {
            "only correlated support artifacts were found"
        } else {
            "no governing intent was found; only local support evidence is available"
        };
        json!({
            "level": level,
            "confidence_reason": confidence_reason,
            "automation_contract": "informational_only"
        })
    }

    fn compute_confidence(
        &self,
        route: RetrievalRoute,
        entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value],
        structural_neighbors: &[Value],
        relevant_soll_entities: &[Value],
    ) -> Value {
        let mut score: f64 = 0.20;
        if !entry_candidates.is_empty() {
            score += 0.35;
        }
        if !supporting_chunks.is_empty() {
            score += 0.20;
        }
        if matches!(route, RetrievalRoute::Impact | RetrievalRoute::Wiring)
            && !structural_neighbors.is_empty()
        {
            score += 0.15;
        }
        if matches!(route, RetrievalRoute::SollHybrid) && !relevant_soll_entities.is_empty() {
            score += 0.10;
        }
        score = score.min(0.95);
        json!({
            "score": score,
            "label": Self::confidence_label(score),
        })
    }

    fn confidence_label(score: f64) -> &'static str {
        if score >= 0.8 {
            "high"
        } else if score >= 0.55 {
            "medium"
        } else {
            "low"
        }
    }

    fn render_evidence_packet(&self, packet: &Value, route: RetrievalRoute) -> String {
        let answer_sketch = packet
            .get("answer_sketch")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let direct = packet
            .get("direct_evidence")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let neighbors = packet
            .get("structural_neighbors")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let confidence = packet
            .get("confidence")
            .and_then(|value| value.get("label"))
            .and_then(|value| value.as_str())
            .unwrap_or("low");
        let evidence_states = packet
            .get("evidence_states")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let governing_requirements = packet
            .get("governing_requirements")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let governing_decisions = packet
            .get("governing_decisions")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let supporting_guidelines = packet
            .get("supporting_guidelines")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let supporting_docs = packet
            .get("supporting_docs")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let supporting_code_context = packet
            .get("supporting_code_context")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let rationale_quality = packet
            .get("rationale_quality")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let mut rendered = format!(
            "**Planner route:** `{}`\n**Evidence confidence:** `{}`\n\n### Answer sketch\n{}\n",
            route.as_str(),
            confidence,
            answer_sketch
        );

        if !evidence_states.is_empty() {
            rendered.push_str("\n### Evidence states\n");
            for row in evidence_states.iter().take(4) {
                rendered.push_str(&format!(
                    "- `{}`: {}\n",
                    row.get("state")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("detail")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                ));
            }
        }

        if !governing_requirements.is_empty() {
            rendered.push_str("\n### Governing requirements\n");
            for row in governing_requirements.iter().take(2) {
                rendered.push_str(&format!(
                    "- `{}` [{} / {}]\n",
                    row.get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("link_mode")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("evidence_provenance")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                ));
            }
        }

        if !governing_decisions.is_empty() {
            rendered.push_str("\n### Governing decisions\n");
            for row in governing_decisions.iter().take(2) {
                rendered.push_str(&format!(
                    "- `{}` [{} / {}]\n",
                    row.get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("link_mode")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("evidence_provenance")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                ));
            }
        }

        if !supporting_guidelines.is_empty() {
            rendered.push_str("\n### Supporting guidelines\n");
            for row in supporting_guidelines.iter().take(2) {
                rendered.push_str(&format!(
                    "- `{}` [{}]\n",
                    row.get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("link_mode")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                ));
            }
        }

        if !direct.is_empty() {
            rendered.push_str("\n### Direct evidence\n");
            for row in direct.iter().take(2) {
                rendered.push_str(&format!(
                    "- `{}` ({}) in `{}` [{}]\n",
                    row.get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("kind")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("uri")
                        .and_then(|value| value.as_str())
                        .unwrap_or(""),
                    row.get("evidence_class")
                        .and_then(|value| value.as_str())
                        .unwrap_or("canonical")
                ));
            }
        }

        if !supporting_docs.is_empty() {
            rendered.push_str("\n### Supporting docs\n");
            for row in supporting_docs.iter().take(2) {
                rendered.push_str(&format!(
                    "- `{}` [{} / {}]: {}\n",
                    row.get("uri")
                        .and_then(|value| value.as_str())
                        .unwrap_or(""),
                    row.get("link_mode")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("evidence_provenance")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("snippet")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                ));
            }
        }

        if !supporting_code_context.is_empty() {
            rendered.push_str("\n### Supporting code context\n");
            for row in supporting_code_context.iter().take(4) {
                rendered.push_str(&format!(
                    "- `{}` [{} / {}]: {}\n",
                    row.get("uri")
                        .or_else(|| row.get("label"))
                        .and_then(|value| value.as_str())
                        .unwrap_or(""),
                    row.get("link_mode")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("evidence_provenance")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("snippet")
                        .or_else(|| row.get("label"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                ));
            }
        }

        if !neighbors.is_empty() {
            rendered.push_str("\n### Structural neighbors\n");
            for row in neighbors.iter().take(2) {
                rendered.push_str(&format!(
                    "- `{}` via `{}` at distance {}\n",
                    row.get("label")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("edge_kind")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("distance")
                        .and_then(|value| value.as_i64())
                        .unwrap_or(0)
                ));
            }
        }

        if rationale_quality.get("level").is_some() {
            rendered.push_str("\n### Rationale quality\n");
            rendered.push_str(&format!(
                "- level: `{}`\n- reason: {}\n- contract: `{}`\n",
                rationale_quality
                    .get("level")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown"),
                rationale_quality
                    .get("confidence_reason")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                rationale_quality
                    .get("automation_contract")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown"),
            ));
        }

        if let Some(diag) = packet.get("retrieval_diagnostics") {
            rendered.push_str("\n### Retrieval diagnostics\n");
            rendered.push_str(&format!(
                "- symbol candidates: {}\n- file candidates: {}\n- chunk candidates: {}\n- anchored chunks selected: {}\n- unanchored chunks selected: {}\n- multipart chunks selected: {}\n- multipart symbol groups selected: {}\n",
                diag.get("symbol_candidates_considered").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("file_candidates_considered").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("chunk_candidates_considered").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("anchored_chunks_selected").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("unanchored_chunks_selected").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("multipart_chunks_selected").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("multipart_symbol_groups_selected").and_then(|value| value.as_u64()).unwrap_or(0),
            ));
        }

        rendered
    }

    fn parse_usize_value(value: &Value) -> Option<usize> {
        value
            .as_u64()
            .and_then(|raw| usize::try_from(raw).ok())
            .or_else(|| {
                value
                    .as_i64()
                    .and_then(|raw| usize::try_from(raw.max(0)).ok())
            })
            .or_else(|| value.as_str().and_then(|raw| raw.parse::<usize>().ok()))
    }

    fn can_reuse_uri_for_multipart(
        candidate: &ChunkCandidate,
        seen_uris: &HashSet<String>,
        selected_source_parts: &HashMap<String, Vec<usize>>,
    ) -> bool {
        if !seen_uris.contains(&candidate.uri) {
            return true;
        }
        if !candidate.anchored_to_entry && !candidate.same_file_as_entry {
            return false;
        }
        if candidate.chunk_part_count <= 1 {
            return false;
        }
        let Some(existing_parts) = selected_source_parts.get(&candidate.source_id) else {
            return false;
        };
        if existing_parts.len() >= 2 {
            return false;
        }
        !existing_parts.contains(&candidate.chunk_part_index)
            && existing_parts
                .iter()
                .any(|existing| existing.abs_diff(candidate.chunk_part_index) <= 1)
    }

    fn truncate(value: &str, max_chars: usize) -> String {
        if value.chars().count() <= max_chars {
            return value.replace('\n', " ");
        }
        let mut end = value.len();
        for (count, (idx, _)) in value.char_indices().enumerate() {
            if count == max_chars {
                end = idx;
                break;
            }
        }
        format!("{}...", value[..end].replace('\n', " "))
    }

    fn estimate_tokens(parts: &[&str]) -> usize {
        parts.iter().map(|part| part.chars().count() / 4 + 1).sum()
    }

    fn escape_sql(value: &str) -> String {
        value.replace('\'', "''")
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkCandidate;
    use super::McpServer;
    use crate::parser::{ExtractionResult, Symbol};
    use crate::queue::ProcessingMode;
    use crate::worker::DbWriteTask;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn candidate(
        source_id: &str,
        uri: &str,
        part_index: usize,
        part_count: usize,
        anchored_to_entry: bool,
        same_file_as_entry: bool,
    ) -> ChunkCandidate {
        ChunkCandidate {
            chunk_id: format!("{source_id}::{part_index}"),
            source_id: source_id.to_string(),
            project_code: "PRJ".to_string(),
            uri: uri.to_string(),
            content: "snippet".to_string(),
            match_reason: "entry_anchor".to_string(),
            lexical_hits: 1,
            semantic_distance: None,
            chunk_part_index: part_index,
            chunk_part_count: part_count,
            chunk_path: format!("{part_index}/{part_count}"),
            anchored_to_entry,
            same_file_as_entry,
            score: 0.0,
            reasons: Vec::new(),
            fts_rank: None,
        }
    }

    #[test]
    fn multipart_uri_reuse_allows_one_adjacent_anchor_chunk() {
        let first = candidate("PRJ::sym", "/repo/file.rs", 1, 3, true, true);
        let second = candidate("PRJ::sym", "/repo/file.rs", 2, 3, true, true);
        let third = candidate("PRJ::sym", "/repo/file.rs", 3, 3, true, true);
        let other = candidate("PRJ::other", "/repo/file.rs", 1, 1, false, false);

        let mut seen_uris = HashSet::new();
        seen_uris.insert(first.uri.clone());
        let mut selected_source_parts = HashMap::new();
        selected_source_parts.insert(first.source_id.clone(), vec![1]);

        assert!(McpServer::can_reuse_uri_for_multipart(
            &second,
            &seen_uris,
            &selected_source_parts
        ));
        assert!(!McpServer::can_reuse_uri_for_multipart(
            &third,
            &seen_uris,
            &selected_source_parts
        ));
        assert!(!McpServer::can_reuse_uri_for_multipart(
            &other,
            &seen_uris,
            &selected_source_parts
        ));

        selected_source_parts.insert(first.source_id.clone(), vec![1, 2]);
        assert!(!McpServer::can_reuse_uri_for_multipart(
            &third,
            &seen_uris,
            &selected_source_parts
        ));
    }

    #[test]
    fn retrieve_context_retains_adjacent_chunks_for_split_symbol() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let server = McpServer::new(store.clone());
        let path = "/tmp/multipart_lookup_probe.rs".to_string();

        unsafe {
            std::env::set_var("AXON_TARGET_CHUNK_TOKENS", "64");
            std::env::set_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH", "32");
            std::env::set_var("AXON_GRAY_ZONE_CHAR_THRESHOLD", "64");
        }

        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 42, 1)])
            .unwrap();
        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                reservation_id: "res-multipart-lookup".to_string(),
                path: path.clone(),
                content: Some(
                    [
                        "fn multipart_lookup_probe() {",
                        "    let alpha = very_long_identifier_name_for_a_large_symbol_payload();",
                        "",
                        "    let beta = very_long_identifier_name_for_a_large_symbol_payload();",
                        "",
                        "    let gamma = very_long_identifier_name_for_a_large_symbol_payload();",
                        "",
                        "    let delta = very_long_identifier_name_for_a_large_symbol_payload();",
                        "}",
                    ]
                    .join("\n"),
                ),
                extraction: ExtractionResult {
                    project_code: Some("PRJ".to_string()),
                    symbols: vec![Symbol {
                        name: "multipart_lookup_probe".to_string(),
                        kind: "function".to_string(),
                        start_line: 1,
                        end_line: 9,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: Default::default(),
                        embedding: None,
                    }],
                    relations: vec![],
                },
                processing_mode: ProcessingMode::Full,
                trace_id: "trace-multipart-lookup".to_string(),
                observed_cost_bytes: 1,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let response = server
            .axon_retrieve_context(&json!({
                "question": "multipart_lookup_probe",
                "project": "PRJ",
                "top_k": 4,
                "token_budget": 1200,
                "include_graph": false,
                "include_soll": false,
            }))
            .expect("retrieve_context response");

        let supporting_chunks = response["data"]["packet"]["supporting_chunks"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(supporting_chunks.len(), 2);
        assert!(supporting_chunks.iter().all(|chunk| {
            chunk["source_id"]
                .as_str()
                .unwrap_or_default()
                .ends_with("::multipart_lookup_probe")
        }));
        assert_eq!(
            supporting_chunks[0]["chunk_path"]
                .as_str()
                .unwrap_or_default(),
            "1/4"
        );
        assert_eq!(
            supporting_chunks[1]["chunk_path"]
                .as_str()
                .unwrap_or_default(),
            "2/4"
        );

        let diagnostics = &response["data"]["packet"]["retrieval_diagnostics"];
        assert_eq!(
            diagnostics["multipart_chunks_selected"]
                .as_u64()
                .unwrap_or_default(),
            2
        );
        assert_eq!(
            diagnostics["multipart_symbol_groups_selected"]
                .as_u64()
                .unwrap_or_default(),
            1
        );

        unsafe {
            std::env::remove_var("AXON_TARGET_CHUNK_TOKENS");
            std::env::remove_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH");
            std::env::remove_var("AXON_GRAY_ZONE_CHAR_THRESHOLD");
        }
    }

    #[test]
    fn rerank_prefers_head_and_adjacent_multipart_chunks() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let server = McpServer::new(store);
        let entry_candidates = vec![super::EntryCandidate {
            id: "PRJ::file.rs::multipart_lookup_probe".to_string(),
            name: "multipart_lookup_probe".to_string(),
            kind: "function".to_string(),
            project_code: "PRJ".to_string(),
            uri: "/repo/file.rs".to_string(),
            lexical_hits: 1,
            exact_match: true,
            score: 1.0,
            reasons: vec!["exact".to_string()],
        }];
        let mut candidates = vec![
            candidate(
                "PRJ::file.rs::multipart_lookup_probe",
                "/repo/file.rs",
                4,
                4,
                true,
                true,
            ),
            candidate(
                "PRJ::file.rs::multipart_lookup_probe",
                "/repo/file.rs",
                2,
                4,
                true,
                true,
            ),
            candidate(
                "PRJ::file.rs::multipart_lookup_probe",
                "/repo/file.rs",
                1,
                4,
                true,
                true,
            ),
        ];

        server.rerank_chunk_candidates(
            &mut candidates,
            super::RetrievalRoute::ExactLookup,
            &["multipart_lookup_probe".to_string()],
            &entry_candidates,
            &["PRJ".to_string()],
            false,
            false,
        );

        assert_eq!(candidates[0].chunk_part_index, 1);
        assert_eq!(candidates[1].chunk_part_index, 2);
        assert_eq!(candidates[2].chunk_part_index, 4);
        assert!(candidates[0]
            .reasons
            .iter()
            .any(|reason| reason == "multipart_lead_chunk"));
        assert!(candidates[1]
            .reasons
            .iter()
            .any(|reason| reason == "multipart_adjacent_continuation_bonus"));
        assert!(candidates[2]
            .reasons
            .iter()
            .any(|reason| reason == "multipart_late_chunk_penalty"));
    }
}
