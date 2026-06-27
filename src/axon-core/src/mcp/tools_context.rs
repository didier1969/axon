use crate::embedding_contract::CHUNK_MODEL_ID;
use crate::service_guard::ServicePressure;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
#[cfg(not(test))]
use std::sync::Mutex;
use std::time::Instant;

use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;

mod entry_candidates;
mod evidence_classification;
mod evidence_packet;
mod question_analysis;
mod rationale_quality;
mod repo_literal;
mod retrieval_bands;
mod retrieval_model;
mod retrieval_routing;
mod semantic_pressure;
mod soll_collection;
mod soll_retrieval;
mod soll_traceability;
mod structural_neighbors;
mod retrieval_scoring;
mod util;
use util::*;
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
/// REQ-AXO-902023 tier C.1 — bounded wait for semantic pressure recovery.
/// Poll cadence, hard ceiling, and the implied budget when the caller passes
/// `wait_for_semantic: true` (bool shorthand) instead of an explicit ms value.
const WAIT_FOR_SEMANTIC_STEP_MS: u64 = 50;
const MAX_WAIT_FOR_SEMANTIC_MS: u64 = 3000;
pub(super) const DEFAULT_WAIT_FOR_SEMANTIC_MS: u64 = 1000;
/// REQ-AXO-901952 — upper bound on symbols pulled per file when resolving
/// forward CONTAINS from the RAM snapshot (replaces the unbounded SQL `IN`
/// scan). Generous: a single source file rarely declares more symbols.
pub(super) const CONTAINS_SYMBOL_CAP: usize = 10_000;

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
        // REQ-AXO-902039 element 2 — RAM-first via IstGraphView (PIL-AXO-9002).
        // This is the `SELECT id FROM Symbol WHERE name=$sym OR id=$sym` the REQ
        // names explicitly. When the project is scoped and the IST snapshot is
        // warm, resolve `symbol` as a canonical id (already present) or by short
        // name without touching PG. A cold cache returns None below and we fall
        // through to the explicit PG fallback.
        if let Some(proj) = project {
            let view = crate::ist_snapshot::process_view();
            // `symbol` may already be a canonical id.
            if view.node_kind_db(proj, symbol).is_some() {
                crate::soll_snapshot::record_fusion_read(true);
                return Some(symbol.to_string());
            }
            // Otherwise resolve by short name (PG used LIMIT 1; first id matches).
            if let Some(ids) = view.ids_for_short_name(proj, symbol) {
                crate::soll_snapshot::record_fusion_read(true);
                return ids.into_iter().next();
            }
            // ids_for_short_name returned None ⇒ snapshot cold ⇒ PG fallback.
        }
        // PG-direct fallback: project unscoped or IST snapshot cold (PIL-AXO-9002).
        crate::soll_snapshot::record_fusion_read(false);
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
        // PG-direct (deliberate, REQ-AXO-902039 element 4): error-path symbol
        // suggestions. Off the hot fusion lane (only fires when resolution fails),
        // and returns name+kind+project_code as a JSON shape the lexical RAM
        // search does not mirror 1:1. Kept on PG by design — not a cache bypass to
        // migrate (PIL-AXO-9002 PG lane for reporting-shaped reads).
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
        // REQ-AXO-902023 tier C.2 — detect a composed question (≥2 sub-questions).
        // The lexical/FTS/structural lanes already read every word of the full
        // question, so the split's value is SEMANTIC: each sub-question gets its
        // own clean embed and candidates are re-ranked by their CLOSEST sub-
        // question (a blended whole-question vector is muddy). None = single
        // question → today's single-vector path, unchanged.
        let sub_questions = Self::split_composed_question(question);
        let is_composed = sub_questions.is_some();
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
        // REQ-AXO-901937 / DEC-AXO-901632 — embed the question once, up front,
        // when service pressure permits, and attach each symbol candidate's
        // cosine distance so `rerank_entry_candidates` can order open-question
        // routes by relevance (semantic-primary) instead of bare lexical name
        // matches. The vector is threaded into the chunk lane below so the
        // question is embedded at most once per call.
        // REQ-AXO-901978 (A) — `semantic=lexical|off` lets the caller skip the
        // question-embedding entirely (FTS + structural lanes only) when it wants
        // the fastest answer. Default (absent / auto / semantic) embeds, since
        // retrieve_context is question-oriented.
        let semantic_lexical_off = matches!(
            args.get("semantic").and_then(|v| v.as_str()),
            Some("lexical") | Some("off")
        );
        // REQ-AXO-901987 / REQ-AXO-902018 tier B (DEC-AXO-901642) — embed the
        // question whenever the caller did not opt out; `batch_embed`'s own guard
        // (GPU query-embed is decoupled from ServicePressure, REQ-AXO-901987)
        // decides feasibility. The vector then powers two SEPARATE-cost paths: the
        // CHEAP candidate re-rank (`fill_*_semantic_distances` over already-found
        // rows, runnable under pressure) and the EXPENSIVE corpus-wide ANN pool
        // expansion (kept gated on low pressure so a degraded backend never pays
        // the seq-scan cost). The old binary gate conflated the two and threw the
        // cheap signal away with the expensive one.
        // REQ-AXO-902023 tier C.1 (DEC-AXO-901642) — opt-in bounded wait. When the
        // caller passes `wait_for_semantic` (ms, clamped to MAX) and the current
        // pressure would degrade the corpus-wide ANN, poll `current_pressure()`
        // over a short bounded window and proceed as soon as it recovers to
        // Healthy/Recovering, instead of degrading immediately. Absent param =
        // today's behavior (one sample, no wait). The resolved pressure is the
        // single source threaded into both the corpus gate AND the runtime state
        // (RetrievalRuntimeState previously re-sampled, which could disagree).
        let wait_for_semantic_ms = args
            .get("wait_for_semantic")
            .and_then(Self::parse_wait_for_semantic)
            .map(|ms| ms.min(MAX_WAIT_FOR_SEMANTIC_MS));
        let (pressure, waited_for_semantic_ms) = Self::resolve_pressure_with_wait(
            wait_for_semantic_ms,
            WAIT_FOR_SEMANTIC_STEP_MS,
            crate::service_guard::current_pressure,
            |ms| std::thread::sleep(std::time::Duration::from_millis(ms)),
        );
        if waited_for_semantic_ms > 0 {
            excluded_because
                .push(format!("waited_{waited_for_semantic_ms}ms_for_semantic_pressure_recovery"));
        }
        let semantic_corpus_allowed = Self::semantic_corpus_pressure_ok(pressure);
        // REQ-AXO-902023 tier C.2 — embed per sub-question when composed (each
        // clean), else the whole question once. The fills below keep the MIN
        // distance across vectors, so a candidate is ranked by whichever sub-
        // question it answers best. Single question → one vector → one iteration,
        // identical to the prior single-embed path.
        let question_vectors: Vec<Vec<f32>> = if semantic_lexical_off {
            Vec::new()
        } else if let Some(parts) = sub_questions.as_ref() {
            crate::embedder::batch_embed(parts.clone()).unwrap_or_default()
        } else {
            crate::embedder::batch_embed(vec![question.to_string()])
                .ok()
                .unwrap_or_default()
        };
        let question_vector: Option<Vec<f32>> = question_vectors.first().cloned();
        for qvec in &question_vectors {
            if let Ok(qvec_literal) = crate::postgres::vector::vector_literal(qvec) {
                // CHEAP: fill cosine distances on the already-found symbol pool.
                self.fill_entry_semantic_distances(&mut entry_candidates, &qvec_literal);
                if semantic_corpus_allowed {
                    // EXPENSIVE corpus-wide ANN pool expansion — guarantees the
                    // semantically-closest symbols are present even when the
                    // lexical arm + arbitrary LIMIT missed them. Gated on pressure.
                    self.add_semantic_entry_candidates(
                        &mut entry_candidates,
                        &qvec_literal,
                        project,
                        200,
                        top_k * 4,
                    );
                }
            }
        }
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
        let mut runtime = RetrievalRuntimeState::new_with_pressure(self, pressure);
        // REQ-AXO-901978 (A) — `semantic=lexical|off` disables the semantic CHUNK
        // lane too (not just the up-front entry-ranking embed), so the escape-hatch
        // genuinely avoids the embedding round-trip end-to-end. Short-circuits
        // before `allow_semantic_search` so no embed is attempted.
        let semantic_allowed =
            !semantic_lexical_off && runtime.allow_semantic_search(has_strong_anchor);
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
                question_vector.as_deref(),
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
        // column is back-filled by the pgmq tsv_worker (REQ-AXO-901624);
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
        // REQ-AXO-902018 tier B (DEC-AXO-901642, ROOT) — when the corpus-wide
        // semantic CHUNK ANN was skipped (pressure / backlog) but a question
        // vector is available, re-rank the structural + FTS candidates already
        // found with one cheap cosine pass over their existing embeddings. Recovers
        // the essential win (right answer first) without the ANN's corpus scan,
        // instead of degrading to a lexical-only ranking and a silent miss.
        // REQ-AXO-902023 tier C.2 — composed questions ALWAYS get the per-sub-
        // question min-distance re-rank (loop every vector), so the chunk ranking
        // reflects the closest sub-question even under healthy pressure. Single
        // questions keep tier-B behavior: cheap re-rank only when the corpus ANN
        // was skipped (pressure / backlog).
        let mut semantic_rerank_applied = false;
        let should_rerank_chunks =
            (is_composed || !semantic_allowed) && !chunk_candidates.is_empty();
        if should_rerank_chunks {
            for qvec in &question_vectors {
                if let Ok(qvec_literal) = crate::postgres::vector::vector_literal(qvec) {
                    self.fill_chunk_semantic_distances(&mut chunk_candidates, &qvec_literal);
                    semantic_rerank_applied = true;
                }
            }
            if semantic_rerank_applied {
                runtime.semantic_search_used = true;
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
        let mut relevant_soll_entities = if should_join_soll
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
        // REQ-AXO-901757 slice B3b — fuse the semantic SOLL arm. ANN over SOLL
        // description embeddings surfaces governing intent that the graph-
        // traceability join misses (no anchor / weak lexical overlap), which is
        // exactly the NL-question case retrieve_context serves. Gated on the same
        // join eligibility + semantic budget as the chunk lane, so `semantic=off`
        // / pressure-critical / `include_soll=false` all still skip it.
        if should_join_soll
            && semantic_allowed
            && !runtime.should_skip_soll_join(route, rationale_requested)
        {
            if let Some(qvec) = question_vector.as_ref() {
                let ann_entities = self.collect_soll_entities_via_ann(qvec, project, top_k);
                if !ann_entities.is_empty() {
                    Self::merge_soll_entities(&mut relevant_soll_entities, ann_entities);
                    diagnostics.soll_entities_selected = relevant_soll_entities.len();
                }
            }
        }
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
            &terms_for_reasoning,
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

        // REQ-AXO-902018 tier A — fail-loud degradation notice (DEC-AXO-901642).
        let degradation_notice = Self::build_degradation_notice(
            runtime.degraded_reason.as_deref(),
            runtime.pressure,
            semantic_rerank_applied,
        );
        let mut packet = json!({
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
                "estimated_tokens": estimate_tokens(&[
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
        if let Some(notice) = &degradation_notice {
            if let Some(obj) = packet.as_object_mut() {
                obj.insert("degradation".to_string(), notice.clone());
            }
        }

        // REQ-AXO-901752 — SRS slice 2: detect legacy proximity from
        // artifacts returned in the evidence packet.
        let legacy_proximity_value =
            self.detect_packet_legacy_proximity(project, &direct_evidence, &supporting_chunks);

        let mut data = json!({
            "planner": {
                "route": route.as_str(),
                "project_scope": project.unwrap_or("*"),
                "project_scope_variants": project_scope_variants,
                "terms": terms_for_reasoning,
                // REQ-AXO-902023 tier C.2 — null unless the question was split.
                "composed_question": sub_questions
                    .as_ref()
                    .map(|parts| json!({ "sub_questions": parts })),
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
        if let Some(lp) = &legacy_proximity_value {
            data["legacy_proximity"] = lp.clone();
        }

        let evidence = self.render_evidence_packet(&data["packet"], route);
        let evidence = evidence_by_mode(&evidence, mode);
        let scope = project
            .map(|value| format!("project:{value}"))
            .unwrap_or_else(|| "workspace:*".to_string());
        // REQ-AXO-902018 tier A — banner the degradation at the HEAD of the human
        // report, not buried in excluded_because (fail-loud, PIL-AXO-002).
        let degradation_banner = degradation_notice
            .as_ref()
            .map(|n| {
                format!(
                    "> ⚠️ **Retrieval degraded ({}):** {} {}\n\n",
                    n["class"].as_str().unwrap_or("DEGRADED"),
                    n["impact"].as_str().unwrap_or_default(),
                    n["remediation"].as_str().unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let report = format!(
            "### Context Retrieval: {}\n\n{}{}",
            question,
            degradation_banner,
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
                confidence_label(
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
    /// REQ-AXO-902097 (demande Nexus) — opt-in entailment veto over retrieved
    /// passages. When `args.veto` is present, each code-band chunk is judged by the
    /// NLI cross-encoder against the `question`; contradicting chunks are flagged
    /// (`contradiction_detected` + `entailment` + `contradiction`) and, if
    /// `veto.filter=true`, dropped. Rétro-compatible (no `veto` → chunks unchanged)
    /// and bounded (`VETO_MAX_JUDGEMENTS`) so latency stays usable. NLI unavailable
    /// → chunks left as-is (best-effort hardening, never blocks retrieval).
    fn apply_entailment_veto(&self, chunks: Vec<Value>, args: &Value) -> Vec<Value> {
        const VETO_MAX_JUDGEMENTS: usize = 12;
        let Some(veto) = args.get("veto") else {
            return chunks;
        };
        let threshold = veto
            .get("entailment_threshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.5) as f32;
        let filter = veto.get("filter").and_then(Value::as_bool).unwrap_or(false);
        let question = args.get("question").and_then(Value::as_str).unwrap_or("");
        if question.is_empty() {
            return chunks;
        }
        let mut out: Vec<Value> = Vec::with_capacity(chunks.len());
        let mut judged = 0usize;
        for mut chunk in chunks {
            let text = chunk
                .get("content")
                .or_else(|| chunk.get("text"))
                .or_else(|| chunk.get("snippet"))
                .or_else(|| chunk.get("summary"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if text.is_empty() || judged >= VETO_MAX_JUDGEMENTS {
                out.push(chunk);
                continue;
            }
            match crate::nli::judge_global(text, question) {
                Ok(scores) => {
                    judged += 1;
                    let contradicts = scores.contradiction >= threshold;
                    if let Some(obj) = chunk.as_object_mut() {
                        obj.insert("contradiction_detected".to_string(), json!(contradicts));
                        obj.insert("entailment".to_string(), json!(scores.entailment));
                        obj.insert("contradiction".to_string(), json!(scores.contradiction));
                    }
                    if filter && contradicts {
                        continue; // drop the contradicting passage
                    }
                    out.push(chunk);
                }
                Err(_) => out.push(chunk), // NLI unavailable → leave as-is
            }
        }
        out
    }

    pub(crate) fn axon_retrieve_context_layered(&self, args: &Value) -> Option<Value> {
        let started_at = Instant::now();
        // REQ-AXO-901927 — the layered tool's intent_band (SOLL concepts /
        // decisions / requirements) is a FIRST-CLASS output. Force the SOLL
        // join so it is populated even when the planner picks the plain
        // `hybrid` route and the question carries no "why" language — otherwise
        // `should_join_soll` defaulted to false and the intent band came back
        // empty for questions that DO have a governing REQ/Decision. An
        // explicit caller `include_soll=false` is still respected.
        let mut soll_args = args.clone();
        if let Some(obj) = soll_args.as_object_mut() {
            obj.entry("include_soll".to_string())
                .or_insert(Value::Bool(true));
        }
        let inner = self.axon_retrieve_context(&soll_args)?;

        // Propagate input-validation errors verbatim (same shape, isError=true).
        if inner.get("isError").and_then(|value| value.as_bool()) == Some(true) {
            return Some(inner);
        }

        let inner_data = inner.get("data").cloned().unwrap_or_else(|| json!({}));
        let packet = inner_data
            .get("packet")
            .cloned()
            .unwrap_or_else(|| json!({}));

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
        if let Some(entities) = packet
            .get("relevant_soll_entities")
            .and_then(|value| value.as_array())
        {
            for entity in entities {
                let row = json!({
                    "id": entity.get("id").cloned().unwrap_or(Value::Null),
                    "title": entity.get("title").cloned().unwrap_or(Value::Null),
                    "summary": entity.get("description").cloned().unwrap_or(Value::Null),
                    "status": entity.get("status").cloned().unwrap_or(Value::Null),
                });
                let kind = entity
                    .get("entity_type")
                    .or_else(|| entity.get("type"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
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
        }))
        .unwrap_or_default();
        let intent_tokens_pre = estimate_tokens(&[&intent_text_full]);

        // Truncate intent rows in priority order: requirements > decisions > concepts.
        let (
            intent_concepts_kept,
            intent_decisions_kept,
            intent_requirements_kept,
            intent_tokens_post,
            intent_overflowed,
        ) = Self::truncate_intent_band(
            intent_concepts,
            intent_decisions,
            intent_requirements,
            intent_budget,
        );

        // code_band ← packet.direct_evidence + packet.supporting_chunks (chunks reused).
        let mut code_chunks_full: Vec<Value> = Vec::new();
        if let Some(evidence) = packet
            .get("direct_evidence")
            .and_then(|value| value.as_array())
        {
            code_chunks_full.extend(evidence.iter().cloned());
        }
        if let Some(supporting) = packet
            .get("supporting_chunks")
            .and_then(|value| value.as_array())
        {
            code_chunks_full.extend(supporting.iter().cloned());
        }
        let code_tokens_pre =
            estimate_tokens(&[&serde_json::to_string(&code_chunks_full).unwrap_or_default()]);
        let (code_chunks, code_tokens_post, code_overflowed) =
            Self::truncate_chunks_band(code_chunks_full, code_budget);
        // REQ-AXO-902097 — opt-in entailment veto over the code band: flag (and
        // optionally drop) passages that CONTRADICT the question, via the NLI
        // cross-encoder. Rétro-compatible: no `veto` arg → chunks unchanged.
        let code_chunks = self.apply_entailment_veto(code_chunks, args);

        // recent_band — REQ-AXO-264 A6 v1: populate via `git log --since=24h`
        // on the resolved project path. Each commit yields a {file, ts,
        // subject} row per changed file. cwd hint goes into `current_focus`.
        // Falls back to a structured empty band when no project root is
        // resolvable (LLM clients still get a stable contract).
        let project_root = std::env::var("AXON_PROJECT_ROOT").ok().or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        });
        let mut recent_band = Self::collect_recent_band(project_root.as_deref());
        let recent_tokens_pre = recent_band
            .get("tokens_used")
            .and_then(|t| t.as_u64())
            .unwrap_or(0) as usize;
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
            // REQ-AXO-91524 (MIL-AXO-019 Tier A) — tri-modal envelope.
            // `retrieve_context_layered` wraps `retrieve_context` (which
            // already exposes its own RRF tri-modal surface via
            // REQ-AXO-91489) and re-organises the output into intent /
            // code / recent bands constrained by token budgets. The
            // "layered" semantics here is band-budget, NOT graph-layer
            // BFS (cf. `ist_snapshot::algorithms::bfs_layers` which
            // would be a different feature). Surfaces inherit from the
            // inner retrieve_context envelope.
            "surfaces_used": ["retrieve_context_rrf", "token_budget_bands"],
            "total_available": total_tokens,
            "next_call_hint": "increase bands.<band>.max_tokens or call retrieve_context directly for unbudgeted view"
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
    // REQ-AXO-219 — layered-retrieval band helpers (layered_band_max_tokens,
    // truncate_intent_band, truncate_chunks_band, truncate_recent_band,
    // collect_recent_band) moved to the `retrieval_bands` submodule (god-file
    // split). Still associated fns on McpServer; `Self::…` call sites unchanged.

    // REQ-AXO-219 — NL question-analysis helpers (plan_retrieval_route,
    // looks_like_exact_lookup, question_terms, split_composed_question,
    // split_on_interrogative_coordinator, question_path_hints, …) moved to the
    // `question_analysis` submodule to shrink this god-file (APoSD deep-module).
    // They remain associated fns on McpServer, so `Self::…` call sites here are
    // unchanged.

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

    // REQ-AXO-219 — retrieval routing + SQL-fragment helpers (term_match_sql,
    // path_match_sql, has_rationale_language, route_prefers_operational_code,
    // prefer_project_intent) moved to the `retrieval_routing` submodule.

    // REQ-AXO-219 — retrieval URI/doc scoring helpers (project_intent_doc_weight,
    // canonical_project_doc_weight, workspace_noise_penalty, uri_penalty_reason,
    // chunk_penalty_reason) moved to the `retrieval_scoring` submodule (god-file
    // split). Still associated fns on McpServer; `Self::…` call sites unchanged.

    // REQ-AXO-219 — find_entry_candidates moved to the `entry_candidates`
    // submodule (god-file split, &self phase). `self.find_entry_candidates` call
    // site unchanged.

    // REQ-AXO-219 — repo-literal candidate helpers (project_repo_root,
    // is_strong_identifier_term, repo_literal_file_rank,
    // should_consider_repo_literal_path, snippet_around_term) moved to the
    // `repo_literal` submodule. Still associated fns on McpServer; `Self::…`
    // call sites unchanged.

    // REQ-AXO-219 — repo_literal_fallback_candidates moved to the
    // `entry_candidates` submodule (god-file split, &self phase).
    // `self.repo_literal_fallback_candidates` call site unchanged.

    // REQ-AXO-219 — find_symbol_candidates, find_exact_symbol_candidates,
    // find_file_candidates moved to the `entry_candidates` submodule (god-file
    // split, &self phase). `self.…` call sites unchanged.

    /// REQ-AXO-901952 — RAM-only forward CONTAINS (file → contained symbols).
    /// `file_project_pairs` carries `(file_path, project_code)` so the lookup
    /// scopes to the file's own per-project snapshot (derive-project pattern),
    /// never the legacy unscoped `ist.Edge` SQL. Returns `(symbol_id, file_path)`
    /// — same shape the superseded SQL emitted as `(target_id, source_id)`.
    /// Cold snapshot → that file is skipped (best-effort: retrieve_context
    /// still has FTS + vector arms), never a silent PG fallback.
    // REQ-AXO-219 — resolve_file_symbol_bindings moved to the `entry_candidates`
    // submodule (god-file split, &self phase). Call site unchanged.

    /// REQ-AXO-901952 — RAM-only reverse CONTAINS (symbol → containing file).
    /// Replaces the per-row `(SELECT ce.source_id FROM ist.Edge … CONTAINS)`
    /// SQL fallback used when `Chunk.file_path` is NULL. Resolves from the
    /// row's own project snapshot; empty when cold / absent (display-only
    /// enrichment, so a miss is non-fatal — never a silent PG fallback).
    // REQ-AXO-219 — resolve_containing_file_ram moved to the `entry_candidates`
    // submodule (god-file split, &self phase). Call site unchanged.

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
        // REQ-AXO-901937 / DEC-AXO-901632 — open-question routes order primarily
        // by semantic relevance (cosine distance ASC) so the semantically-correct
        // entrypoint beats a bare lexical name match; the lexical/structural score
        // is the secondary key. Activates only when at least one candidate clears
        // the relevance threshold (else lexical noise would reshuffle the order),
        // and never for precise routes (ExactLookup / Wiring / Impact), which keep
        // lexical anchor primacy. Degraded semantic lane → all distances `None` →
        // graceful fall-back to the historical lexical sort.
        const ENTRY_SEMANTIC_RELEVANCE_MAX: f64 = 0.5;
        // Docs, tests, benchmarks and scripts all embed closer to a natural-
        // language question than production code identifiers (markdown headings,
        // `test_*` assertions and prose mirror the question's vocabulary), so
        // pure semantic distance would crown a test or a working-note section as
        // the "primary entrypoint" of a *code* retrieval tool — observed live:
        // `test_soll_relation_schema_resolves_pair_by_ids` beat the production
        // `insert_validated_relation` (REQ-AXO-901937 criterion 1). Treat any
        // non-production-code provenance as a SECONDARY entrypoint: when a
        // production-code candidate is itself relevant it ranks first; otherwise
        // (no relevant code) the best secondary candidate may still anchor the
        // packet. Provenance reuses the canonical `evidence_provenance_for_uri`
        // classifier (single source of truth, GUI-PRO-013).
        let is_secondary_entry = |c: &EntryCandidate| {
            matches!(c.kind.as_str(), "section" | "document" | "doc")
                || !matches!(Self::evidence_provenance_for_uri(&c.uri), "code_chunk")
        };
        let semantic_primary = matches!(route, RetrievalRoute::Hybrid | RetrievalRoute::SollHybrid)
            && candidates
                .iter()
                .any(|c| c.semantic_distance.map_or(false, |d| d < ENTRY_SEMANTIC_RELEVANCE_MAX));
        if semantic_primary {
            let primary_relevant = candidates.iter().any(|c| {
                !is_secondary_entry(c)
                    && c.semantic_distance.map_or(false, |d| d < ENTRY_SEMANTIC_RELEVANCE_MAX)
            });
            candidates.sort_by(|left, right| {
                if primary_relevant {
                    // false (production code) sorts before true (doc/test/bench/script)
                    let ordering = is_secondary_entry(left).cmp(&is_secondary_entry(right));
                    if ordering != std::cmp::Ordering::Equal {
                        return ordering;
                    }
                }
                let ld = left.semantic_distance.unwrap_or(f64::INFINITY);
                let rd = right.semantic_distance.unwrap_or(f64::INFINITY);
                ld.partial_cmp(&rd)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        right
                            .score
                            .partial_cmp(&left.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| left.uri.cmp(&right.uri))
            });
            if let Some(first) = candidates.first_mut() {
                first.reasons.push("semantic_primary_order".to_string());
            }
        } else {
            candidates.sort_by(|left, right| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.uri.cmp(&right.uri))
            });
        }
    }

    /// REQ-AXO-901937 / DEC-AXO-901632 — fill each *symbol* entry candidate's
    /// semantic distance to the question vector, in one batched `IN (...)` query.
    ///
    /// The distance is the MIN cosine distance over the candidate symbol's
    /// embedded chunks (`ist.Chunk.source_id = symbol_id` → `ist.ChunkEmbedding`).
    /// `ist.Symbol.embedding` is NOT populated in the canonical pipeline (only
    /// chunks are embedded — verified empirically on dev session 78), so a
    /// `s.embedding <=> qvec` query returns nothing; the live signal lives on the
    /// chunks. File / repo-literal candidates carry no symbol chunks and are
    /// skipped (left `None`). Robust to the JSON bridge returning the distance as
    /// either a number or a numeric string.
    fn fill_entry_semantic_distances(&self, candidates: &mut [EntryCandidate], qvec_literal: &str) {
        let ids: Vec<String> = candidates
            .iter()
            .filter(|c| !matches!(c.kind.as_str(), "file" | "repo_literal"))
            .map(|c| c.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let in_list = ids
            .iter()
            .map(|id| format!("'{}'", Self::escape_sql(id)))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT c.source_id, MIN((ce.embedding <=> {qvec})::float8) AS dist \
             FROM ist.Chunk c \
             JOIN ist.ChunkEmbedding ce ON ce.chunk_id = c.id \
             WHERE c.source_id IN ({in_list}) AND c.source_type = 'symbol' \
             GROUP BY c.source_id",
            qvec = qvec_literal,
        );
        let raw = self
            .graph_store
            .query_json(&sql)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut dist_by_id: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for row in rows {
            let id = match row.first().and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            let dist = row.get(1).and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            });
            if let Some(dist) = dist {
                dist_by_id.insert(id, dist);
            }
        }
        for candidate in candidates.iter_mut() {
            if let Some(dist) = dist_by_id.get(&candidate.id) {
                // REQ-AXO-902023 tier C.2 — keep the MIN across repeated calls so a
                // composed question (one fill per sub-question vector) ranks each
                // candidate by its CLOSEST sub-question. Single-call: existing None
                // → `dist`, identical to the prior overwrite.
                candidate.semantic_distance =
                    Some(candidate.semantic_distance.map_or(*dist, |ex| ex.min(*dist)));
            }
        }
    }

    /// REQ-AXO-902018 tier B (DEC-AXO-901642, ROOT) — fill each chunk candidate's
    /// cosine distance to the question from its EXISTING `ist.ChunkEmbedding` row.
    /// This is the cheap, separable half of the semantic signal the binary
    /// pressure gate used to discard along with the expensive corpus-wide ANN: a
    /// single IN-list lookup over the ≤ `top_k*5` already-found chunk ids (indexed
    /// PK, no corpus scan). Running it under pressure re-ranks the structural +
    /// FTS candidates so the right answer surfaces first, instead of a lexical-only
    /// ranking and a silent miss. `rerank_chunk_candidates` already rewards
    /// `semantic_distance`, so filling it is sufficient.
    fn fill_chunk_semantic_distances(&self, candidates: &mut [ChunkCandidate], qvec_literal: &str) {
        if candidates.is_empty() {
            return;
        }
        let in_list = candidates
            .iter()
            .map(|c| format!("'{}'", Self::escape_sql(&c.chunk_id)))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT ce.chunk_id, (ce.embedding <=> {qvec})::float8 AS dist \
             FROM ist.ChunkEmbedding ce \
             WHERE ce.chunk_id IN ({in_list})",
            qvec = qvec_literal,
        );
        let raw = self
            .graph_store
            .query_json(&sql)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut dist_by_id: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for row in rows {
            let id = match row.first().and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            let dist = row.get(1).and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            });
            if let Some(dist) = dist {
                dist_by_id.insert(id, dist);
            }
        }
        for candidate in candidates.iter_mut() {
            if let Some(dist) = dist_by_id.get(&candidate.chunk_id) {
                // REQ-AXO-902023 tier C.2 — MIN across sub-question vectors (see
                // fill_entry_semantic_distances). Single-call behavior unchanged.
                candidate.semantic_distance =
                    Some(candidate.semantic_distance.map_or(*dist, |ex| ex.min(*dist)));
            }
        }
    }

    /// REQ-AXO-901937 / DEC-AXO-901632 — expand the entry-candidate pool with the
    /// symbols whose chunks are semantically closest to the question.
    ///
    /// The lexical pool (`find_symbol_candidates`) is keyed on name / file-path
    /// substring matches under an arbitrary `LIMIT`, so the semantically-correct
    /// entrypoint can be absent from the pool entirely (a method the reranker
    /// never sees can never win). This arm runs the HNSW ANN path
    /// (`query_ann_json`) over `ist.ChunkEmbedding`, maps the closest chunks back
    /// to their owning symbols, and appends any not already present — each
    /// carrying its best-chunk cosine distance so the semantic-primary sort can
    /// rank it. Pure semantic candidates get `lexical_hits = 0`, `exact = false`.
    fn add_semantic_entry_candidates(
        &self,
        candidates: &mut Vec<EntryCandidate>,
        qvec_literal: &str,
        project: Option<&str>,
        ann_pool: usize,
        limit: usize,
    ) {
        let project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]);
        let sql = format!(
            "WITH ann AS ( \
                 SELECT ce.chunk_id, (ce.embedding <=> {qvec}) AS dist \
                 FROM ist.ChunkEmbedding ce \
                 ORDER BY ce.embedding <=> {qvec} \
                 LIMIT {ann_pool} \
             ) \
             SELECT c.source_id, s.name, s.kind, COALESCE(c.project_code, 'unknown'), \
                    COALESCE(c.file_path, ''), MIN(a.dist)::float8 AS dist \
             FROM ann a \
             JOIN ist.Chunk c ON c.id = a.chunk_id AND c.source_type = 'symbol' \
             JOIN ist.Symbol s ON s.id = c.source_id \
             WHERE TRUE{project_filter} \
             GROUP BY c.source_id, s.name, s.kind, c.project_code, c.file_path \
             ORDER BY dist ASC \
             LIMIT {limit}",
            qvec = qvec_literal,
        );
        let ef_search = ann_pool.max(40) as u32;
        let raw = self
            .graph_store
            .query_ann_json(&sql, ef_search)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let existing: std::collections::HashSet<String> =
            candidates.iter().map(|c| c.id.clone()).collect();
        for row in rows {
            let id = match row.first().and_then(|v| v.as_str()) {
                Some(id) if !existing.contains(id) => id.to_string(),
                _ => continue,
            };
            let name = row.get(1).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let kind = row.get(2).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let project_code = row
                .get(3)
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let uri = row.get(4).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let dist = row.get(5).and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            });
            candidates.push(EntryCandidate {
                id,
                name,
                kind,
                project_code,
                uri,
                lexical_hits: 0,
                exact_match: false,
                score: 0.0,
                reasons: vec!["semantic_entry_candidate".to_string()],
                semantic_distance: dist,
            });
        }
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

    /// REQ-AXO-901883 — build the SOTA ANN-first hybrid SELECT for the semantic
    /// chunk lane so pgvector chooses `chunk_embedding_hnsw_idx`.
    ///
    /// The old single OR-hybrid pass (Chunk JOIN ChunkEmbedding,
    /// `WHERE (lexical OR path OR entry OR cosine < 0.55) ORDER BY cosine LIMIT k`)
    /// forced a Seq Scan + exact-distance Sort over `ist.ChunkEmbedding` — pgvector
    /// never selects HNSW for that shape. We split it into:
    ///   `ann`  — a clean `ORDER BY embedding <=> q LIMIT ann_pool` on
    ///            `ist.ChunkEmbedding` (the HNSW path; this query is run through
    ///            `GraphStore::query_ann_json`, which scopes
    ///            `SET LOCAL enable_seqscan = off` + `SET LOCAL hnsw.ef_search`).
    ///            `model_id` is deliberately NOT filtered inside the `ORDER BY`
    ///            (single model today → moot, and a `WHERE` there can defeat HNSW
    ///            post-filtering); it is enforced on the outer `sem` join.
    ///   `sem`  — the ANN candidates that pass the cosine threshold, joined to
    ///            `ist.Chunk`, project-filtered at the outer join.
    ///   `lex`  — the non-vector arm (entry_anchor / same_file / file_path /
    ///            lexical), preserved verbatim with a NULL distance.
    /// The two arms are UNIONed, deduped (`ROW_NUMBER` per id), source-tag
    /// prioritised (the original CASE), then ordered anchored-first / ascending
    /// distance and limited — so the hybrid composition + source-tag CASE survive.
    ///
    /// `project_filter` is the identical substring on the `sem` and `lex`
    /// `c.project_code` predicates so the repo-root fallback's
    /// `query.replace(project_filter, "")` strips every occurrence.
    ///
    /// NB: inside the `ann` CTE the embedding table is aliased `ce`, so the
    /// distance projection references `ce.embedding` — exactly what `cosine_expr`
    /// holds (`(ce.embedding <=> lit)`). The `a` alias exists ONLY in `sem`, where
    /// the already-computed `a.dist` is read, never recomputed (the alias bug that
    /// broke the first cut: a `ce.embedding -> a.embedding` rewrite leaked into the
    /// `ann` projection, where `a` does not exist → `missing FROM-clause entry`).
    #[allow(clippy::too_many_arguments)]
    fn build_semantic_chunk_query(
        cosine_expr: &str,
        qvec_literal: &str,
        ann_pool: usize,
        project_filter: &str,
        entry_id_match: &str,
        entry_uri_match: &str,
        lexical_predicate: &str,
        lexical_uri_match: &str,
        path_match: &str,
        limit: usize,
    ) -> String {
        format!(
            "WITH ann AS ( \
                 SELECT ce.chunk_id, ce.embedding, ({cosine_expr}) AS dist \
                 FROM ist.ChunkEmbedding ce \
                 ORDER BY ce.embedding <=> {qvec} \
                 LIMIT {ann_pool} \
             ), \
             sem AS ( \
                 SELECT c.id, c.source_id, c.project_code, c.content, c.content_hash, \
                        c.file_path, c.chunk_part_index, c.chunk_part_count, c.chunk_path, \
                        a.dist AS dist \
                 FROM ann a \
                 JOIN ist.ChunkEmbedding ce2 ON ce2.chunk_id = a.chunk_id AND ce2.model_id = '{model_id}' \
                 JOIN ist.Chunk c ON c.id = a.chunk_id AND ce2.source_hash = c.content_hash \
                 WHERE a.dist < 0.55{project_filter} \
             ), \
             lex AS ( \
                 SELECT c.id, c.source_id, c.project_code, c.content, c.content_hash, \
                        c.file_path, c.chunk_part_index, c.chunk_part_count, c.chunk_path, \
                        NULL::float8 AS dist \
                 FROM ist.Chunk c \
                 WHERE (({entry_id_match}) OR ({entry_uri_match}) OR ({lexical_predicate}) OR ({lexical_uri_match}) OR ({path_match})){project_filter} \
             ), \
             merged AS ( \
                 SELECT * FROM sem \
                 UNION \
                 SELECT * FROM lex \
             ), \
             ranked AS ( \
                 SELECT m.*, \
                        CASE \
                            WHEN ({entry_id_match_m}) THEN 0 \
                            WHEN ({entry_uri_match_m}) THEN 1 \
                            WHEN ({path_match_m}) THEN 2 \
                            WHEN ({lexical_predicate_m}) AND m.dist IS NOT NULL THEN 3 \
                            WHEN m.dist IS NOT NULL THEN 4 \
                            ELSE 5 \
                        END AS src_rank, \
                        ROW_NUMBER() OVER (PARTITION BY m.id ORDER BY m.dist ASC NULLS LAST) AS rn \
                 FROM merged m \
             ) \
             SELECT r.id, r.source_id, COALESCE(r.project_code, 'unknown'), \
                    COALESCE(r.file_path, ''), \
                    r.content, COALESCE(r.chunk_part_index, 1), COALESCE(r.chunk_part_count, 1), COALESCE(r.chunk_path, '1/1'), \
                    CASE r.src_rank \
                        WHEN 0 THEN 'entry_anchor' \
                        WHEN 1 THEN 'same_file' \
                        WHEN 2 THEN 'file_path' \
                        WHEN 3 THEN 'lexical+semantic' \
                        WHEN 4 THEN 'semantic' \
                        ELSE 'lexical' \
                    END, \
                    r.dist \
             FROM ranked r \
             WHERE r.rn = 1 \
             ORDER BY r.src_rank ASC, r.dist ASC NULLS LAST \
             LIMIT {limit}",
            qvec = qvec_literal,
            cosine_expr = cosine_expr,
            ann_pool = ann_pool,
            model_id = CHUNK_MODEL_ID,
            entry_id_match_m = entry_id_match.replace("c.source_id", "m.source_id"),
            entry_uri_match_m = entry_uri_match.replace("c.file_path", "m.file_path"),
            path_match_m = path_match.replace("c.file_path", "m.file_path"),
            lexical_predicate_m = lexical_predicate.replace("c.content", "m.content"),
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn find_chunk_candidates(
        &self,
        project: Option<&str>,
        question: &str,
        precomputed_question_vector: Option<&[f32]>,
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
        // as a row column. pgvector `<=>` is the canonical
        // cosine-distance operator with `'[..]'::vector(N)` literals.
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
                .map(|uri| format!("c.file_path = '{}'", Self::escape_sql(uri)))
                .collect::<Vec<_>>()
                .join(" OR ")
        };
        let lexical_predicate = Self::term_match_sql(terms, "c.content");
        let path_match = Self::path_match_sql(path_hints, "c.file_path");
        let lexical_uri_match = Self::term_match_sql(terms, "c.file_path");

        let semantic = if semantic_allowed {
            // REQ-AXO-901937 — reuse the question vector already embedded for the
            // entry-ranking lane when present (at most one embed per call); only
            // fall back to a fresh `batch_embed` if the up-front embed was skipped.
            if let Some(precomputed) = precomputed_question_vector {
                runtime.semantic_search_used = true;
                Some(precomputed.to_vec())
            } else {
                match crate::embedder::batch_embed(vec![question.to_string()]) {
                    Ok(vectors) => {
                        runtime.semantic_search_used = true;
                        vectors.into_iter().next()
                    }
                    Err(err) => {
                        excluded_because.push("semantic_chunk_search_unavailable".to_string());
                        excluded_because.push(format!(
                            "semantic_chunk_search_error:{}",
                            truncate(&err.to_string(), 120)
                        ));
                        None
                    }
                }
            }
        } else {
            excluded_because.push("semantic_chunk_search_skipped".to_string());
            None
        };

        if Self::route_prefers_operational_code(route)
            && (!entry_ids.is_empty() || !entry_uris.is_empty())
        {
            // REQ-AXO-901952 — derive each file's project from its own entry
            // candidate so the RAM forward-CONTAINS lookup scopes correctly,
            // even when the retrieval is unscoped (project == None).
            let file_project_pairs = entry_candidates
                .iter()
                .filter(|candidate| !candidate.uri.is_empty() && !candidate.project_code.is_empty())
                .map(|candidate| (candidate.uri.clone(), candidate.project_code.clone()))
                .collect::<Vec<_>>();
            let file_bindings = self.resolve_file_symbol_bindings(&file_project_pairs);
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
                    let chunk_part_index = parse_usize_value(row.get(4)?).unwrap_or(1).max(1);
                    let chunk_part_count = parse_usize_value(row.get(5)?).unwrap_or(1).max(1);
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

        let project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]);

        // REQ-AXO-901883 — keep the raw `'[..]'::vector` literal (`qvec_literal`)
        // for the ANN `ORDER BY embedding <=> qvec` so pgvector can match the
        // HNSW index; `cosine_expr` wraps it as the distance projection.
        let (cosine_expr, qvec_literal) = if let Some(embedding) = semantic.as_ref() {
            match crate::postgres::vector::vector_literal(embedding) {
                Ok(lit) => (Some(format!("(ce.embedding <=> {lit})")), Some(lit)),
                Err(err) => {
                    excluded_because.push(format!(
                        "pg_semantic_vector_literal_error:{}",
                        truncate(&err.to_string(), 120)
                    ));
                    return Vec::new();
                }
            }
        } else {
            (None, None)
        };
        // ANN candidate pool = a few × the final limit so the lexical/anchor
        // arm + the cosine-threshold filter still leave enough semantic hits.
        let ann_pool = (limit.max(1) * 5).clamp(40, 400);
        let qvec_literal = qvec_literal.unwrap_or_default();

        let query = if let Some(cosine_expr) = cosine_expr.as_ref() {
            Self::build_semantic_chunk_query(
                cosine_expr,
                &qvec_literal,
                ann_pool,
                &project_filter,
                &entry_id_match,
                &entry_uri_match,
                &lexical_predicate,
                &lexical_uri_match,
                &path_match,
                limit,
            )
        } else {
            format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), COALESCE(c.file_path, ''), c.content, \
                        COALESCE(c.chunk_part_index, 1), COALESCE(c.chunk_part_count, 1), COALESCE(c.chunk_path, '1/1'), \
                        CASE \
                            WHEN ({entry_id_match}) THEN 'entry_anchor' \
                            WHEN ({entry_uri_match}) THEN 'same_file' \
                            WHEN ({path_match}) THEN 'file_path' \
                            ELSE 'lexical' \
                        END, \
                        NULL \
                 FROM Chunk c \
                 WHERE (({entry_id_match}) OR ({entry_uri_match}) OR ({lexical_predicate}) OR ({lexical_uri_match}) OR ({path_match})){project_filter} \
                 LIMIT {limit}",
            )
        };

        // REQ-AXO-901883 — the semantic (ANN) branch must run through
        // `query_ann_json` (transaction-scoped SET LOCAL enable_seqscan=off +
        // hnsw.ef_search) so pgvector picks `chunk_embedding_hnsw_idx`; the
        // pure-lexical branch keeps the plain reader. ef_search ≈ ann_pool but
        // never below the pgvector floor.
        let is_semantic = cosine_expr.is_some();
        let ef_search = ann_pool.max(40) as u32;
        let run = |sql: &str| -> String {
            if is_semantic {
                self.graph_store
                    .query_ann_json(sql, ef_search)
                    .unwrap_or_else(|_| "[]".to_string())
            } else {
                self.graph_store
                    .query_json(sql)
                    .unwrap_or_else(|_| "[]".to_string())
            }
        };
        let raw = run(&query);
        let mut rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        if rows.is_empty() {
            // Repo-root fallback: drop project_code filter (every occurrence —
            // the ANN composition references it in both the `sem` and `lex`
            // CTEs), post-filter by repo_root prefix.
            if let Some(repo_root) = Self::project_repo_root(project) {
                let fallback_query = query.replace(
                    &Self::sql_project_filter_for_fields(project, &["c.project_code"]),
                    "",
                );
                let fallback_raw = run(&fallback_query);
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
                let mut uri = row.get(3)?.as_str().unwrap_or_default().to_string();
                // REQ-AXO-901952 — RAM-only file_path enrichment when the chunk
                // row carries no file_path (replaces the inline CONTAINS subquery).
                if uri.is_empty() {
                    uri = self.resolve_containing_file_ram(&project_code, &source_id);
                }
                let content = row.get(4)?.as_str()?.to_string();
                let chunk_part_index = parse_usize_value(row.get(5)?).unwrap_or(1).max(1);
                let chunk_part_count = parse_usize_value(row.get(6)?).unwrap_or(1).max(1);
                let chunk_path = row.get(7)?.as_str().unwrap_or("1/1").to_string();
                let match_reason = row.get(8)?.as_str()?.to_string();
                // REQ-AXO-901883 — the native reader renders FLOAT8 as a string
                // (`render_pg_value`); a bare `as_f64()` would drop every cosine
                // distance. `parse_f64_value` accepts the string form + the
                // `"null"` sentinel.
                let semantic_distance = row.get(9).and_then(parse_f64_value);
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
    /// Queries `ist.Chunk.content_tsv` (GIN-indexed; back-filled by the
    /// pgmq tsv_worker, REQ-AXO-901624) via `websearch_to_tsquery` so operators
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
        // Rollback knobs: legacy `AXON_IST_FTS_DISABLED` (slice 1)
        // stays for backwards-compat; new `AXON_HYBRID_RETRIEVAL_DISABLED`
        // (slice 2) is the canonical superset knob disabling the
        // whole hybrid path. Either one short-circuits FTS.
        let env_is_truthy = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| {
                    matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false)
        };
        if env_is_truthy("AXON_IST_FTS_DISABLED") || env_is_truthy("AXON_HYBRID_RETRIEVAL_DISABLED")
        {
            return Vec::new();
        }
        let trimmed = question.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let escaped_question = Self::escape_sql(trimmed);
        // `ist.Chunk` carries `file_path` directly — no need to
        // join the legacy `CONTAINS` table (which was retired by
        // MIL-AXO-017 slice 6 in favour of `ist.Edge` with
        // relation_type='CONTAINS'). Filter on `c.project_code` only.
        let project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]);
        // websearch_to_tsquery with the `english` dictionary matches
        // the DDL's content-body indexing (db/ddl/03_ist_schema.sql:128 +
        // 06_pgmq_tsv_async.sql build content_tsv with `english` for content body, `simple` for
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
             FROM ist.Chunk c \
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
                let mut uri = row.get(3)?.as_str().unwrap_or("").to_string();
                // REQ-AXO-901952 — RAM-only file_path enrichment when the chunk
                // row carries no file_path (replaces the inline CONTAINS subquery).
                if uri.is_empty() {
                    uri = self.resolve_containing_file_ram(&project_code, &source_id);
                }
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
            if !can_reuse_uri_for_multipart(candidate, seen_uris, selected_source_parts) {
                return false;
            }
            let snippet = truncate(&candidate.content, 220);
            let estimated = estimate_tokens(&[&snippet]);
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

    // REQ-AXO-219 — collect_structural_neighbors moved to the
    // `structural_neighbors` submodule (god-file split, &self phase). RAM-only CSR
    // expansion; `self.collect_structural_neighbors` call site unchanged.

    // REQ-AXO-219 — has_direct_soll_traceability + snapshot_has_direct_traceability
    // moved to the `soll_traceability` submodule (god-file split, &self phase).
    // `self.…` / `Self::…` call sites unchanged.

    // REQ-AXO-219 — the RAM SOLL-fusion trio (collect_soll_entities dispatcher,
    // collect_soll_traceability_ram, expand_concept_governing_entities_ram) moved
    // to the `soll_traceability` submodule (god-file split, &self phase). `self.…`
    // / `Self::…` call sites unchanged; the PG fallback collect_soll_entities_pg
    // stays here and resolves via descendant access.

    // REQ-AXO-219 — collect_soll_entities_pg moved to the `soll_collection`
    // submodule (god-file split, &self phase). PG traceability + lexical fallback;
    // `self.collect_soll_entities_pg` call site (in the RAM dispatcher) unchanged.

    // REQ-AXO-219 — expand_concept_governing_entities moved to the
    // `soll_collection` submodule (god-file split, &self phase). PG concept→
    // requirement/decision bridge; `self.expand_concept_governing_entities` call
    // sites unchanged.

    // REQ-AXO-219 — build_answer_sketch moved to the `evidence_packet` submodule
    // (god-file split, &self phase). `self.build_answer_sketch` call site unchanged.

    /// REQ-AXO-901752 — scan artifacts in the evidence packet for legacy
    /// SOLL proximity. Returns a serialized `LegacyProximity` if any
    /// artifact is linked to a superseded SOLL node.
    fn detect_packet_legacy_proximity(
        &self,
        project: Option<&str>,
        direct_evidence: &[Value],
        supporting_chunks: &[Value],
    ) -> Option<Value> {
        let project_code = project?;
        let snapshot = self.soll_cache().snapshot(project_code).ok()?;

        let mut artifact_refs: Vec<String> = Vec::new();
        for ev in direct_evidence {
            if let Some(uri) = ev.get("uri").and_then(|v| v.as_str()) {
                if !artifact_refs.iter().any(|r| r == uri) {
                    artifact_refs.push(uri.to_string());
                }
            }
            if let Some(sym) = ev.get("symbol_id").and_then(|v| v.as_str()) {
                if !artifact_refs.iter().any(|r| r == sym) {
                    artifact_refs.push(sym.to_string());
                }
            }
        }
        for chunk in supporting_chunks {
            if let Some(uri) = chunk.get("uri").and_then(|v| v.as_str()) {
                if !artifact_refs.iter().any(|r| r == uri) {
                    artifact_refs.push(uri.to_string());
                }
            }
            if let Some(cp) = chunk.get("chunk_path").and_then(|v| v.as_str()) {
                if !artifact_refs.iter().any(|r| r == cp) {
                    artifact_refs.push(cp.to_string());
                }
            }
        }

        let mut all_nodes: Vec<super::tools_srs::LegacyNode> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        for artifact in &artifact_refs {
            if let Some(prox) = super::tools_srs::detect_legacy_proximity(artifact, &snapshot) {
                for node in prox.nodes {
                    if seen_ids.insert(node.id.clone()) {
                        all_nodes.push(node);
                    }
                }
            }
        }

        if all_nodes.is_empty() {
            return None;
        }

        let direction = all_nodes
            .first()
            .map(|n| n.strategy.direction_hint())
            .unwrap_or("review legacy linkage")
            .to_string();
        let confidence = if all_nodes.iter().all(|n| n.successor.is_some()) {
            "high"
        } else {
            "medium"
        };
        Some(json!({
            "nodes": all_nodes.iter().map(|n| json!({
                "id": n.id,
                "strategy": n.strategy,
                "successor": n.successor,
                "superseded_at": n.superseded_at,
            })).collect::<Vec<_>>(),
            "direction": direction,
            "confidence": confidence,
        }))
    }

    // REQ-AXO-219 — build_direct_evidence moved to the `evidence_packet`
    // submodule (god-file split, &self method phase). `self.build_direct_evidence`
    // call site unchanged.

    // REQ-AXO-219 — build_why_these_items + build_missing_evidence moved to the
    // `evidence_packet` submodule (god-file split, &self phase). `self.…` call
    // sites unchanged.

    // REQ-AXO-219 — collect_soll_entities_via_ann moved to the `soll_retrieval`
    // submodule (god-file split, &self phase). `self.collect_soll_entities_via_ann`
    // call site unchanged.

    /// REQ-AXO-901757 slice B3b — fuse semantic ANN SOLL hits into the
    /// traceability-derived intent band. Union by `id`: a node already present
    /// from traceability keeps its (stronger) tier but gains the semantic
    /// `ranking_reason` so the dual signal is visible; genuinely new semantic
    /// nodes are appended. Traceability entities stay first (higher base score),
    /// so the downstream type-partitioned classification is unaffected for them.
    // REQ-AXO-219 — merge_soll_entities moved to the `evidence_classification`
    // submodule (god-file split). `Self::merge_soll_entities` call site unchanged.

    /// REQ-AXO-902018 slice A (tier A, DEC-AXO-901642) — fail-loud degradation
    /// REQ-AXO-902023 tier C.1 — does this pressure permit the corpus-wide
    /// semantic ANN? Single predicate shared by the corpus gate and the bounded
    /// wait, so both agree on what "recovered" means.
    // REQ-AXO-219 — semantic service-pressure helpers (semantic_corpus_pressure_ok,
    // parse_wait_for_semantic, resolve_pressure_with_wait, build_degradation_notice)
    // moved to the `semantic_pressure` submodule. Still associated fns on
    // McpServer; `Self::…` call sites unchanged.

    // REQ-AXO-219 — the evidence-classification cluster (classify_governing_entities,
    // evidence_provenance_for_uri, classify_direct_code_evidence,
    // classify_supporting_chunks_by_provenance, classify_supporting_code_context)
    // moved to the `evidence_classification` submodule (god-file split). Still
    // associated fns on McpServer; `Self::…` call sites unchanged.

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

    /// REQ-AXO-901976 critère #3 — a governing entity is *relevant* to the
    /// question when it shares at least one overlap signal with it:
    ///   - **anchor**: it was selected via direct symbol/file traceability
    ///     (`evidence_class == "soll_traceability"`); since the entrypoint is now
    ///     semantic-primary (DEC-AXO-901632), that relevance is transitive.
    ///   - **term**: its title contains a question term of length ≥ 4 (same
    ///     `len >= 4` convention as `collect_soll_entities`).
    /// Entities pulled in *only* via `soll_concept_bridge` with neither anchor
    /// nor term overlap (an off-topic sibling sharing a Concept) are NOT relevant
    /// and must not crown the packet `strong`. Semantic overlap (cosine
    /// question↔node) is deliberately deferred: embedding every SOLL node per
    /// call is the cost the author flagged — lexical+anchor is the measured first
    /// pass, mirroring the rank-based lexical/structural choice of DEC-AXO-901632.
    pub(super) fn governing_overlaps_question(entity: &Value, question_terms: &[String]) -> bool {
        if entity.get("evidence_class").and_then(|value| value.as_str())
            == Some("soll_traceability")
        {
            return true;
        }
        let title = entity
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if title.is_empty() {
            return false;
        }
        question_terms
            .iter()
            .filter(|term| term.len() >= 4)
            .any(|term| title.contains(&term.to_ascii_lowercase()))
    }

    // REQ-AXO-219 — build_rationale_quality moved to the `rationale_quality`
    // submodule (god-file split). Still an associated fn on McpServer; the
    // `Self::build_rationale_quality` call site is unchanged.

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
            "label": confidence_label(score),
        })
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


    // REQ-AXO-219 — pub(super) so the extracted retrieval submodules
    // (retrieval_routing) can reuse the same single-quote escaper.
    pub(super) fn escape_sql(value: &str) -> String {
        value.replace('\'', "''")
    }
}

#[cfg(test)]
mod tests;
