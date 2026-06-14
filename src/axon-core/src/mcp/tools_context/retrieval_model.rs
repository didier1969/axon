use crate::service_guard::{self, ServicePressure};
#[cfg(not(test))]
use serde_json::Value;
#[cfg(not(test))]
use std::collections::HashMap;
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};

use crate::mcp::McpServer;

pub(super) const VECTOR_QUEUE_BACKLOG_WARN: usize = 128;
pub(super) const VECTOR_QUEUE_BACKLOG_HARD_STOP: usize = 512;
#[cfg(not(test))]
pub(super) const RETRIEVE_CONTEXT_CACHE_TTL_MS: i64 = 60_000;

#[cfg(not(test))]
pub(super) type RetrieveContextCache = HashMap<String, (i64, Value)>;

#[cfg(not(test))]
pub(super) static RETRIEVE_CONTEXT_CACHE: OnceLock<Mutex<RetrieveContextCache>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetrievalRoute {
    ExactLookup,
    Wiring,
    Impact,
    SollHybrid,
    Hybrid,
}

impl RetrievalRoute {
    pub(super) fn as_str(self) -> &'static str {
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
pub(super) struct EntryCandidate {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) project_code: String,
    pub(super) uri: String,
    pub(super) lexical_hits: usize,
    pub(super) exact_match: bool,
    pub(super) score: f64,
    pub(super) reasons: Vec<String>,
    // REQ-AXO-901937 / DEC-AXO-901632 — cosine distance of this candidate's
    // Symbol embedding to the question vector (`s.embedding <=> qvec`), filled
    // before entry reranking when semantic capacity allows. `None` for file /
    // repo-literal candidates (no symbol embedding) and when the semantic lane
    // is degraded. Open-question routes (Hybrid / SollHybrid) order primarily by
    // this distance so the semantically-relevant entrypoint wins over a bare
    // lexical name match.
    pub(super) semantic_distance: Option<f64>,
}

#[derive(Clone, Debug)]
pub(super) struct ChunkCandidate {
    pub(super) chunk_id: String,
    pub(super) source_id: String,
    pub(super) project_code: String,
    pub(super) uri: String,
    pub(super) content: String,
    pub(super) match_reason: String,
    pub(super) lexical_hits: usize,
    pub(super) semantic_distance: Option<f64>,
    pub(super) chunk_part_index: usize,
    pub(super) chunk_part_count: usize,
    pub(super) chunk_path: String,
    pub(super) anchored_to_entry: bool,
    pub(super) same_file_as_entry: bool,
    pub(super) score: f64,
    pub(super) reasons: Vec<String>,
    // DEC-AXO-093 / REQ-AXO-324 slice 2 — when this candidate was
    // discovered via the FTS modality, this carries the raw
    // `ts_rank_cd` score (cover-density). `None` for all other
    // sources. Used by `rerank_chunk_candidates` to give FTS a
    // dedicated bonus band so it competes with anchored hits on
    // an equal footing, and by `select_supporting_chunks` to
    // reserve slots for FTS-discovered evidence even when
    // anchors exist.
    pub(super) fts_rank: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RetrievalDiagnostics {
    pub(super) symbol_candidates_considered: usize,
    pub(super) file_candidates_considered: usize,
    pub(super) chunk_candidates_considered: usize,
    pub(super) anchored_chunks_selected: usize,
    pub(super) unanchored_chunks_selected: usize,
    pub(super) multipart_chunks_selected: usize,
    pub(super) multipart_symbol_groups_selected: usize,
    pub(super) graph_neighbors_selected: usize,
    pub(super) soll_entities_selected: usize,
    // REQ-AXO-324 slice 2 — FTS modality observability.
    pub(super) fts_chunks_considered: usize,
    pub(super) fts_chunks_selected: usize,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RetrievalTimings {
    pub(super) planner_ms: u64,
    pub(super) entry_lookup_ms: u64,
    pub(super) runtime_guard_ms: u64,
    pub(super) chunk_lookup_ms: u64,
    pub(super) chunk_selection_ms: u64,
    pub(super) graph_expansion_ms: u64,
    pub(super) soll_join_ms: u64,
    pub(super) packet_assembly_ms: u64,
    pub(super) total_ms: u64,
}

#[derive(Clone, Debug)]
pub(super) struct RetrievalRuntimeState {
    pub(super) pressure: ServicePressure,
    pub(super) graph_projection_queue_depth: usize,
    pub(super) file_vectorization_queue_depth: usize,
    pub(super) semantic_search_used: bool,
    pub(super) degraded_reason: Option<String>,
}

impl RetrievalRuntimeState {
    pub(super) fn new(_server: &McpServer) -> Self {
        let pressure = service_guard::current_pressure();
        // REQ-AXO-901653 Slice 3b — queue helpers removed (tables dropped
        // post MIL-AXO-017 / REQ-AXO-289). Canonical pipeline_v2 path tracks
        // via Chunk + ChunkEmbedding directly.
        let (graph_projection_queue_queued, graph_projection_queue_inflight): (usize, usize) =
            (0, 0);
        let graph_projection_queue_depth =
            graph_projection_queue_queued + graph_projection_queue_inflight;
        let (file_vectorization_queue_queued, file_vectorization_queue_inflight): (usize, usize) =
            (0, 0);
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

    pub(super) fn allow_semantic_search(&mut self, has_strong_anchor: bool) -> bool {
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

    pub(super) fn should_skip_graph_expansion(&self) -> bool {
        self.pressure != ServicePressure::Healthy
    }

    pub(super) fn should_skip_soll_join(
        &self,
        route: RetrievalRoute,
        rationale_requested: bool,
    ) -> bool {
        match self.pressure {
            ServicePressure::Healthy => false,
            ServicePressure::Recovering => {
                !(rationale_requested || matches!(route, RetrievalRoute::SollHybrid))
            }
            ServicePressure::Degraded | ServicePressure::Critical => true,
        }
    }
}
