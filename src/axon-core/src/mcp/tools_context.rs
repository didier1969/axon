use crate::embedding_contract::{CHUNK_MODEL_ID, DIMENSION};
use crate::service_guard::{self, ServicePressure};
use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;

const DEFAULT_TOKEN_BUDGET: usize = 1400;
const DEFAULT_TOP_K: usize = 8;
const VECTOR_QUEUE_BACKLOG_WARN: usize = 128;
const VECTOR_QUEUE_BACKLOG_HARD_STOP: usize = 512;
#[allow(dead_code)]
const RETRIEVE_CONTEXT_CACHE_TTL_MS: i64 = 60_000;

#[allow(dead_code)]
type RetrieveContextCache = HashMap<String, (i64, Value)>;

#[allow(dead_code)]
static RETRIEVE_CONTEXT_CACHE: OnceLock<Mutex<RetrieveContextCache>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetrievalRoute {
    ExactLookup,
    Wiring,
    Impact,
    SollHybrid,
    Hybrid,
}

impl RetrievalRoute {
    fn as_str(self) -> &'static str {
        match self {
            Self::ExactLookup => "exact_lookup",
            Self::Wiring => "wiring",
            Self::Impact => "impact",
            Self::SollHybrid => "soll_hybrid",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Clone, Debug)]
struct EntryCandidate {
    id: String,
    name: String,
    kind: String,
    project_code: String,
    uri: String,
    lexical_hits: usize,
    exact_match: bool,
    score: f64,
    reasons: Vec<String>,
}

#[derive(Clone, Debug)]
struct ChunkCandidate {
    chunk_id: String,
    source_id: String,
    project_code: String,
    uri: String,
    content: String,
    match_reason: String,
    lexical_hits: usize,
    semantic_distance: Option<f64>,
    anchored_to_entry: bool,
    same_file_as_entry: bool,
    score: f64,
    reasons: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct RetrievalDiagnostics {
    symbol_candidates_considered: usize,
    file_candidates_considered: usize,
    chunk_candidates_considered: usize,
    anchored_chunks_selected: usize,
    unanchored_chunks_selected: usize,
    graph_neighbors_selected: usize,
    soll_entities_selected: usize,
}

#[derive(Clone, Debug, Default)]
struct RetrievalTimings {
    planner_ms: u64,
    entry_lookup_ms: u64,
    runtime_guard_ms: u64,
    chunk_lookup_ms: u64,
    chunk_selection_ms: u64,
    graph_expansion_ms: u64,
    soll_join_ms: u64,
    packet_assembly_ms: u64,
    total_ms: u64,
}

#[derive(Clone, Debug)]
struct RetrievalRuntimeState {
    pressure: ServicePressure,
    graph_projection_queue_depth: usize,
    file_vectorization_queue_depth: usize,
    semantic_search_used: bool,
    degraded_reason: Option<String>,
}

impl RetrievalRuntimeState {
    fn new(server: &McpServer) -> Self {
        let pressure = service_guard::current_pressure();
        let (graph_projection_queue_queued, graph_projection_queue_inflight) = server
            .graph_store
            .fetch_graph_projection_queue_counts()
            .unwrap_or((0, 0));
        let graph_projection_queue_depth =
            graph_projection_queue_queued + graph_projection_queue_inflight;
        let (file_vectorization_queue_queued, file_vectorization_queue_inflight) = server
            .graph_store
            .fetch_file_vectorization_queue_counts()
            .unwrap_or((0, 0));
        let file_vectorization_queue_depth =
            file_vectorization_queue_queued + file_vectorization_queue_inflight;

        Self {
            pressure,
            graph_projection_queue_depth,
            file_vectorization_queue_depth,
            semantic_search_used: false,
            degraded_reason: None,
        }
    }

    fn allow_semantic_search(&mut self, has_strong_anchor: bool) -> bool {
        match self.pressure {
            ServicePressure::Critical => {
                self.degraded_reason =
                    Some("semantic_chunk_search_skipped_due_to_pressure_critical".to_string());
                false
            }
            ServicePressure::Degraded => {
                self.degraded_reason =
                    Some("semantic_chunk_search_skipped_due_to_pressure_degraded".to_string());
                false
            }
            ServicePressure::Recovering => {
                if !has_strong_anchor {
                    self.degraded_reason = Some(
                        "semantic_chunk_search_skipped_while_recovering_without_strong_anchor"
                            .to_string(),
                    );
                    return false;
                }
                if self.file_vectorization_queue_depth > VECTOR_QUEUE_BACKLOG_WARN {
                    self.degraded_reason = Some(
                        "semantic_chunk_search_skipped_while_recovering_vector_backlog".to_string(),
                    );
                    return false;
                }
                true
            }
            ServicePressure::Healthy => {
                if self.file_vectorization_queue_depth > VECTOR_QUEUE_BACKLOG_HARD_STOP {
                    self.degraded_reason =
                        Some("semantic_chunk_search_skipped_due_to_vector_backlog".to_string());
                    return false;
                }
                true
            }
        }
    }

    fn should_skip_graph_expansion(&self) -> bool {
        self.pressure != ServicePressure::Healthy
    }

    fn should_skip_soll_join(&self) -> bool {
        self.pressure != ServicePressure::Healthy
    }
}

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
            return Some(json!({
                "content": [{"type": "text", "text": "retrieve_context requires a non-empty question"}],
                "isError": true
            }));
        }

        let mode = args.get("mode").and_then(|value| value.as_str());
        let project = args.get("project").and_then(|value| value.as_str());
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
        diagnostics.chunk_candidates_considered = chunk_candidates.len();
        self.rerank_chunk_candidates(
            &mut chunk_candidates,
            route,
            &terms_for_reasoning,
            &entry_candidates,
            &project_scope_variants,
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

        let rationale_requested = Self::has_rationale_language(question);
        let should_join_soll = include_soll.unwrap_or_else(|| {
            let has_direct_traceability =
                self.has_direct_soll_traceability(&entry_candidates, project);
            has_direct_traceability
                || (allow_unanchored_fallback
                    && (matches!(route, RetrievalRoute::SollHybrid) || rationale_requested))
        });
        let stage_started_at = Instant::now();
        let relevant_soll_entities = if should_join_soll && !runtime.should_skip_soll_join() {
            let entities = self.collect_soll_entities(&entry_candidates, project, top_k);
            diagnostics.soll_entities_selected = entities.len();
            entities
        } else if should_join_soll && runtime.should_skip_soll_join() {
            excluded_because.push("soll_join_skipped_due_to_pressure_guarded".to_string());
            Vec::new()
        } else {
            excluded_because.push("soll_join_skipped_for_route".to_string());
            Vec::new()
        };
        timings.soll_join_ms = stage_started_at.elapsed().as_millis() as u64;

        let stage_started_at = Instant::now();
        let direct_evidence = self.build_direct_evidence(&entry_candidates);
        let answer_sketch = self.build_answer_sketch(
            question,
            route,
            &entry_candidates,
            &supporting_chunks,
            &structural_neighbors,
            &relevant_soll_entities,
        );
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
            self.has_direct_soll_traceability(&entry_candidates, project),
            runtime.semantic_search_used,
            runtime.degraded_reason.as_deref(),
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
            "confidence": confidence,
            "missing_evidence": missing_evidence,
            "why_these_items": why_these_items,
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
                "graph_neighbors_selected": diagnostics.graph_neighbors_selected,
                "soll_entities_selected": diagnostics.soll_entities_selected,
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
                anchored_to_entry: true,
                same_file_as_entry: true,
                score: 4.0 + f64::from(base_rank.max(0)),
                reasons,
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
        let values = file_paths
            .iter()
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT c.target_id, c.source_id \
             FROM CONTAINS c \
             WHERE c.source_id IN ({values}){project_filter}",
            project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]),
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
                    let match_reason = row.get(4)?.as_str()?.to_string();
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
                        anchored_to_entry,
                        same_file_as_entry,
                        score: 0.0,
                        reasons: Vec::new(),
                    })
                })
                .collect::<Vec<_>>();
            if !anchored_candidates.is_empty() {
                return anchored_candidates;
            }
        }

        let query = if let Some(embedding) = semantic {
            let vector = format!("{embedding:?}");
            format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), COALESCE(f.path, ''), c.content, \
                        CASE \
                            WHEN ({entry_id_match}) THEN 'entry_anchor' \
                            WHEN ({entry_uri_match}) THEN 'same_file' \
                            WHEN ({path_match}) THEN 'file_path' \
                            WHEN ({lexical_predicate}) THEN 'lexical+semantic' \
                            ELSE 'semantic' \
                        END, \
                        array_cosine_distance(ce.embedding, CAST({vector} AS FLOAT[{DIMENSION}])) \
                 FROM Chunk c \
                 JOIN ChunkEmbedding ce ON ce.chunk_id = c.id AND ce.model_id = '{model_id}' AND ce.source_hash = c.content_hash \
                 LEFT JOIN CONTAINS rel ON rel.target_id = c.source_id \
                 LEFT JOIN File f ON f.path = rel.source_id \
                 WHERE (({entry_id_match}) OR ({entry_uri_match}) OR ({lexical_predicate}) OR ({lexical_uri_match}) OR ({path_match}) OR array_cosine_distance(ce.embedding, CAST({vector} AS FLOAT[{DIMENSION}])) < 0.55){project_filter} \
                 ORDER BY array_cosine_distance(ce.embedding, CAST({vector} AS FLOAT[{DIMENSION}])) ASC \
                 LIMIT {limit}",
                model_id = CHUNK_MODEL_ID,
                project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code", "f.project_code"]),
            )
        } else {
            format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), COALESCE(f.path, ''), c.content, \
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
                project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code", "f.project_code"]),
            )
        };

        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let mut rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        if rows.is_empty() {
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
                let match_reason = row.get(5)?.as_str()?.to_string();
                let semantic_distance = row.get(6).and_then(|value| value.as_f64());
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
                    anchored_to_entry,
                    same_file_as_entry,
                    score: 0.0,
                    reasons: Vec::new(),
                })
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            let (_, repo_chunks) = self.repo_literal_fallback_candidates(project, terms, limit);
            candidates.extend(repo_chunks);
        }
        candidates
    }

    fn rerank_chunk_candidates(
        &self,
        candidates: &mut [ChunkCandidate],
        route: RetrievalRoute,
        terms: &[String],
        entry_candidates: &[EntryCandidate],
        project_scope_variants: &[String],
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
            if Self::route_prefers_operational_code(route) {
                if let Some(reason) = Self::chunk_penalty_reason(candidate) {
                    score -= 2.0;
                    candidate.reasons.push(reason.to_string());
                }
            }
            if !candidate.anchored_to_entry
                && !candidate.same_file_as_entry
                && candidate.semantic_distance.is_some()
                && candidate.lexical_hits == 0
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
        let mut consumed_tokens = 0usize;
        let chunk_cap = top_k.min(4);
        let has_anchor = entry_candidates.iter().any(Self::is_strong_anchor);
        let prefers_operational_code = Self::route_prefers_operational_code(route);
        let mut broader_selected = 0usize;
        let mut non_operational_selected = 0usize;

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
                      consumed_tokens: &mut usize,
                      diagnostics: &mut RetrievalDiagnostics| {
            if selected.len() >= chunk_cap {
                return;
            }
            if !selected_ids.insert(candidate.chunk_id.clone()) {
                return;
            }
            if !candidate.anchored_to_entry && seen_uris.contains(&candidate.uri) {
                return;
            }
            let snippet = Self::truncate(&candidate.content, 220);
            let estimated = Self::estimate_tokens(&[&snippet]);
            if *consumed_tokens + estimated > token_budget / 2 {
                return;
            }
            *consumed_tokens += estimated;
            seen_uris.insert(candidate.uri.clone());
            if candidate.anchored_to_entry || candidate.same_file_as_entry {
                diagnostics.anchored_chunks_selected += 1;
            } else {
                diagnostics.unanchored_chunks_selected += 1;
            }
            selected.push(json!({
                "chunk_id": candidate.chunk_id,
                "source_id": candidate.source_id,
                "project_code": candidate.project_code,
                "uri": candidate.uri,
                "match_reason": candidate.match_reason,
                "evidence_class": "derived_chunk",
                "anchored_to_entry": candidate.anchored_to_entry,
                "same_file_as_entry": candidate.same_file_as_entry,
                "snippet": snippet,
                "score": candidate.score,
                "ranking_reasons": candidate.reasons,
            }));
        };

        for candidate in &anchored {
            ingest(
                candidate,
                &mut selected,
                &mut selected_ids,
                &mut seen_uris,
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
            if has_anchor && !anchored_selected {
                excluded_because.push("not_anchor_affine".to_string());
                continue;
            }
            if has_anchor && prefers_operational_code {
                if let Some(reason) = Self::chunk_penalty_reason(candidate) {
                    excluded_because.push(reason.to_string());
                    if reason != "test_file_penalty" && reason != "docs_file_penalty" {
                        excluded_because.push("non_operational_chunk_penalized".to_string());
                    }
                    continue;
                }
            }
            if broader_selected >= 1 {
                excluded_because.push("broader_semantic_dropped_due_to_anchor".to_string());
                continue;
            }
            if candidate.semantic_distance.is_some() && candidate.lexical_hits == 0 {
                excluded_because.push("generic_semantic_only".to_string());
            }
            if prefers_operational_code && Self::chunk_penalty_reason(candidate).is_some() {
                if non_operational_selected >= 1 {
                    excluded_because.push("non_operational_chunk_penalized".to_string());
                    continue;
                }
                non_operational_selected += 1;
            }
            ingest(
                candidate,
                &mut selected,
                &mut selected_ids,
                &mut seen_uris,
                &mut consumed_tokens,
                diagnostics,
            );
            broader_selected += 1;
        }

        if prefers_operational_code
            && !same_file.is_empty()
            && broader_selected > 0
            && diagnostics.anchored_chunks_selected > 0
        {
            excluded_because.push("same_file_preferred".to_string());
        }

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
        rows.into_iter()
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
            .collect()
    }

    fn build_answer_sketch(
        &self,
        question: &str,
        route: RetrievalRoute,
        entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value],
        structural_neighbors: &[Value],
        relevant_soll_entities: &[Value],
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
        if !relevant_soll_entities.is_empty() {
            let ids = relevant_soll_entities
                .iter()
                .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Relevant SOLL intent joined: {}.", ids));
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
        let chunks = packet
            .get("supporting_chunks")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let neighbors = packet
            .get("structural_neighbors")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let soll = packet
            .get("relevant_soll_entities")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let confidence = packet
            .get("confidence")
            .and_then(|value| value.get("label"))
            .and_then(|value| value.as_str())
            .unwrap_or("low");

        let mut rendered = format!(
            "**Planner route:** `{}`\n**Evidence confidence:** `{}`\n\n### Answer sketch\n{}\n",
            route.as_str(),
            confidence,
            answer_sketch
        );

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

        if !chunks.is_empty() {
            rendered.push_str("\n### Supporting chunks\n");
            for row in chunks.iter().take(4) {
                rendered.push_str(&format!(
                    "- `{}` [{}]: {}\n",
                    row.get("uri")
                        .and_then(|value| value.as_str())
                        .unwrap_or(""),
                    row.get("match_reason")
                        .and_then(|value| value.as_str())
                        .unwrap_or("match"),
                    row.get("snippet")
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

        if !soll.is_empty() {
            rendered.push_str("\n### Relevant SOLL entities\n");
            for row in soll.iter().take(2) {
                rendered.push_str(&format!(
                    "- `{}` ({}) {}\n",
                    row.get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("type")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    row.get("title")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                ));
            }
        }

        if let Some(diag) = packet.get("retrieval_diagnostics") {
            rendered.push_str("\n### Retrieval diagnostics\n");
            rendered.push_str(&format!(
                "- symbol candidates: {}\n- file candidates: {}\n- chunk candidates: {}\n- anchored chunks selected: {}\n- unanchored chunks selected: {}\n",
                diag.get("symbol_candidates_considered").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("file_candidates_considered").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("chunk_candidates_considered").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("anchored_chunks_selected").and_then(|value| value.as_u64()).unwrap_or(0),
                diag.get("unanchored_chunks_selected").and_then(|value| value.as_u64()).unwrap_or(0),
            ));
        }

        rendered
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
