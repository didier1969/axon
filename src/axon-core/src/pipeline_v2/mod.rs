//! Streaming pipeline v2 — REQ-AXO-289 / CPT-AXO-054 (session 19 canonical).
//!
//! # Topology
//!
//! Six independent stages over two pipelines that are throughput-independent:
//!
//! * **Pipeline A — CPU autoritative** (graph + chunks + FTS, one PG
//!   transaction per file):
//!     - [`stage_a1::a1_prepare`] — read file + sha256 + mtime → [`PreparedFile`]
//!     - [`stage_a2::a2_transform`] — tree-sitter parse → [`ParsedFile`]
//!     - [`stage_a3::a3_enroll`] → [`crate::graph::GraphStore::upsert_graph_v2`] :
//!       Symbol + Chunk (full content) + CONTAINS/CALLS/CALLS_NIF persisted
//!       to `public.Edge` (REQ-AXO-295 / REQ-AXO-297 unified storage,
//!       AGE retired per MIL-AXO-017 / REQ-AXO-90005) + IndexedFile UPSERT
//!       in a single transaction. PG FTS lights up automatically via the
//!       [REQ-AXO-292] `content_tsv` `GENERATED ALWAYS AS STORED` column
//!       on `public.Chunk`. Lexical retrieval works **without GPU**.
//! * **Pipeline B — GPU enrichment** :
//!     - [`stage_b1::b1_fetch_for_embedding`] — SELECT chunk content from
//!       PG by `chunk_id` (writer ctx — see read-after-write contract below).
//!     - [`stage_b2::b2_embed`] — drive a [`stage_b2::B2Embedder`]
//!       implementation; [`embedder_gpu::GpuB2Embedder`] wraps the canonical
//!       ORT/TensorRT BGE-Large 1024d session for production.
//!     - [`stage_b3::b3_persist_embedding`] → `upsert_chunk_embedding_v2`.
//!
//! A1's output blocking-sends to A2; A3 fan-outs chunk_ids to B1 via
//! non-blocking `try_send` (cap 10 000). [`stage_b1::b1_cold_start_poll`]
//! rattrape any chunk_ids the try_send buffer dropped, plus chunks from
//! before the v2 cut-over.
//!
//! # Read-after-write contract (critical, see commit 294e09c)
//!
//! B1 fetches chunk content microseconds after A3 commits — the
//! cross-pipeline try_send hand-off IS the steady-state regime, not a
//! rare race. Under the legacy embedded test backend the reader ctx
//! serves a stale snapshot during this window, so B1 MUST read through
//! the writer ctx (`query_json_writer`). Under PG MVCC the deadpool
//! makes the distinction invisible; the writer-ctx call works under
//! every backend.
//!
//! # Driving the pipeline
//!
//! For tests / ad-hoc benches:
//! [`orchestrator::spawn_pipeline_a`] + [`orchestrator::spawn_pipeline_b_full`]
//! return mpsc handles you can feed paths into and receipts out of.
//!
//! For end-to-end smoke / bench :
//! `cargo run --release --bin axon-bench-pipeline-v2 -- --source PATH --gpu`

pub mod channels;
pub mod embedder_gpu;
pub mod indexed_file_cache;
pub mod metrics;
pub mod orchestrator;
pub mod project_resolver;
pub mod notify_listener;
pub mod stage_a1;
pub mod stage_a2;
pub mod stage_a3;
pub mod stage_b1;
pub mod stage_b2;
pub mod stage_b3;
pub mod tsv_worker;
pub mod types;
pub mod worker_pool;

pub use channels::{
    PipelineChannelCaps, A3_TO_B1_BUFFER_CAP_DEFAULT, B1_COLDSTART_BATCH_SIZE_DEFAULT,
    INTERNAL_CHANNEL_CAP_DEFAULT,
};
pub use indexed_file_cache::{IndexedFileCache, IndexedFileEntry};
pub use metrics::{StageMetrics, StageSnapshot};
pub use orchestrator::{
    spawn_pipeline_a, spawn_pipeline_b_b1_only, spawn_pipeline_b_full, PipelineAHandles,
    PipelineAWorkerCounts, PipelineBFullHandles, PipelineBHandles, PipelineBWorkerCounts,
};
pub use stage_a1::a1_prepare;
pub use stage_a2::a2_transform;
pub use stage_a3::{a3_enroll, EnrolledFile};
pub use notify_listener::{
    spawn_chunk_pending_listener, spawn_chunk_pending_state_listener,
    spawn_pending_reconcile_loop,
};
pub use stage_b1::{b1_cold_start_poll, b1_fetch_for_embedding, B1InboxItem, ChunkForEmbedding};
pub use embedder_gpu::GpuB2Embedder;
pub use project_resolver::{const_resolver, project_code_from_chunk_id, ProjectCodeResolver};
pub use stage_b2::{b2_embed, spawn_b2_batched_worker, B2Embedder, EmbeddedChunk, NoOpEmbedder};
pub use stage_b3::{b3_persist_embedding, PersistedEmbedding};
pub use tsv_worker::{spawn_tsv_workers, DrainStats, TsvWorkerConfig};
pub use types::{ParsedFile, PreparedFile};
pub use worker_pool::spawn_stage_workers;

#[cfg(test)]
mod doc_invariants {
    //! Tests that lock the module-level docstring's structural claims
    //! against drift. If a future refactor renames or removes a stage,
    //! these compile-only assertions break and force the doc to track
    //! reality.

    #[test]
    fn six_stages_exported() {
        // Each stage's public entrypoint is named here; if a stage
        // disappears the import fails to compile. The docstring claims
        // six stages — assert by counting the entrypoints we re-export.
        let _ = super::a1_prepare;
        let _ = super::a2_transform;
        let _ = super::a3_enroll;
        let _ = super::b1_fetch_for_embedding;
        let _ = super::b2_embed;
        let _ = super::b3_persist_embedding;
        let _ = super::b1_cold_start_poll;
        // spawn_pipeline_a / spawn_pipeline_b_full are generic over
        // `impl Into<Arc<str>>` for the project_code argument — taking
        // a fn-pointer without parameters fails type inference. Their
        // presence is exercised by the orchestrator's E2E tests.
    }

    #[test]
    fn channel_caps_match_session_19_canonical_table() {
        // Numbers cited in CPT-AXO-054 + the module docstring.
        // REQ-AXO-91567 — B1_COLDSTART_BATCH_SIZE_DEFAULT bumped from
        // 256 → 4096 to drain larger boot-time backlogs in a single
        // poll. Other caps unchanged.
        assert_eq!(super::INTERNAL_CHANNEL_CAP_DEFAULT, 1024);
        assert_eq!(super::A3_TO_B1_BUFFER_CAP_DEFAULT, 10_000);
        assert_eq!(super::B1_COLDSTART_BATCH_SIZE_DEFAULT, 4096);
        assert_eq!(super::channels::B2_BATCH_SIZE_DEFAULT, 64);
        assert_eq!(super::channels::B2_BATCH_TIMEOUT_MS_DEFAULT, 200);
    }
}
