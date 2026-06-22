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
//!     - [`stage_a3::a3_enroll`] → [`crate::graph::GraphStore::upsert_graph`] :
//!       Symbol + Chunk (full content) + CONTAINS/CALLS/CALLS_NIF persisted
//!       to `ist.Edge` (REQ-AXO-295 / REQ-AXO-297 unified storage,
//!       AGE retired per MIL-AXO-017 / REQ-AXO-90005) + IndexedFile UPSERT
//!       in a single transaction. PG FTS is built OUT-OF-BAND: the
//!       [REQ-AXO-901624] pgmq `tsv_pending` worker (tsv_worker.rs) back-fills
//!       `ist.Chunk.content_tsv` after A3 (the GENERATED column was DROPped —
//!       db/ddl/06_pgmq_tsv_async.sql). Lexical retrieval works **without GPU**.
//! * **Pipeline B — GPU enrichment** :
//!     - [`stage_b1::b1_fetch_for_embedding`] — SELECT chunk content from
//!       PG by `chunk_id` (writer ctx — see read-after-write contract below).
//!     - [`stage_b2::b2_embed`] — drive a [`stage_b2::B2Embedder`]
//!       implementation; [`embedder_gpu::GpuB2Embedder`] wraps the canonical
//!       ORT/TensorRT BGE-Large 1024d session for production.
//!     - [`stage_b3::b3_persist_embedding`] → `upsert_chunk_embedding_v2`.
//!
//! A1's output blocking-sends to A2; A2 to A3. A3 persists chunk_ids to PG
//! and `embed_status='pending'` is the durable B queue. There is NO
//! cross-pipeline push channel and NO B1 worker pool (slice 4/5 SOTA,
//! REQ-AXO-901746) — `try_send` is RETIRED. Pipeline B is fed EXCLUSIVELY by
//! the sorted-drain feeder ([`crate::pipeline_runtime::spawn_vector_sorted_drain`],
//! DEC-AXO-901631), which SELECTs token-sorted pending chunks (content
//! included) and feeds B2 in order via the internal `b_chunks` mpsc
//! (cap [`INTERNAL_CHANNEL_CAP_DEFAULT`]).
//!
//! # Read-after-write contract (critical, see commit 294e09c)
//!
//! The sorted-drain SELECTs chunk content microseconds after A3 commits. Under
//! the legacy embedded test backend the reader ctx serves a stale snapshot
//! during this window, so the pull MUST read through the writer ctx
//! (`query_json_writer`). Under PG MVCC the deadpool makes the distinction
//! invisible; the writer-ctx call works under every backend.
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
pub mod in_flight;
pub mod indexed_file_cache;
pub mod metrics;
pub mod orchestrator;
pub mod project_resolver;
pub mod stage_a1;
pub mod stage_a2;
pub mod stage_a3;
pub mod stage_b1;
pub mod stage_b2;
pub mod stage_b3;
pub mod stage_health;
pub mod tsv_worker;
pub mod types;
pub mod worker_pool;

pub use channels::{PipelineChannelCaps, INTERNAL_CHANNEL_CAP_DEFAULT};
pub use embedder_gpu::GpuB2Embedder;
pub use indexed_file_cache::{IndexedFileCache, IndexedFileEntry};
pub use metrics::{StageMetrics, StageSnapshot};
pub use orchestrator::{
    spawn_pipeline_a, spawn_pipeline_a_with_cache, spawn_pipeline_b_full,
    spawn_pipeline_b_full_multi, PipelineAHandles, PipelineAWorkerCounts, PipelineBFullHandles,
    PipelineBWorkerCounts,
};
pub use project_resolver::{
    const_resolver, project_code_from_chunk_id, ProjectCodeResolver, ProjectRegistrySnapshot,
};
pub use stage_a1::a1_prepare;
pub use stage_a2::a2_transform;
pub use stage_a3::EnrolledFile;
pub use stage_b1::ChunkForEmbedding;
pub use stage_b2::{spawn_b2_batched_worker, B2Embedder, EmbeddedChunk, NoOpEmbedder};
pub use stage_b3::PersistedEmbedding;
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
        let _ = super::a1_prepare;
        let _ = super::a2_transform;
        let _ = super::stage_a3::a3_enroll;
        let _ = super::stage_b1::b1_fetch_for_embedding;
        let _ = super::stage_b2::b2_embed;
        let _ = super::stage_b3::b3_persist_embedding;
    }

    #[test]
    fn channel_caps_match_session_19_canonical_table() {
        // Numbers cited in CPT-AXO-054 + the module docstring.
        assert_eq!(super::INTERNAL_CHANNEL_CAP_DEFAULT, 1024);
        assert_eq!(super::channels::B2_BATCH_SIZE_DEFAULT, 64);
        assert_eq!(super::channels::B2_BATCH_TIMEOUT_MS_DEFAULT, 200);
    }
}
