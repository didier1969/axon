//! REQ-AXO-128 / DEC-AXO-061 / CPT-AXO-022 — Query-time CPU embedding for
//! non-indexer profiles (brain_only, indexer_graph).
//!
//! The brain process spawns an in-process query embedding worker that
//! reuses `SemanticWorkerPool::query_worker_loop` — the exact same loop
//! the indexer's semantic worker pool runs — but in a single-thread
//! configuration that builds a fastembed `TextEmbedding` in CPU mode
//! (no GPU subprocess required). The worker registers itself via
//! `register_query_embedding_sender`, so `batch_embed` flows through
//! the normal query-embedding channel without any special-case branch.
//!
//! This is what makes the brain deserve its name (CPT-AXO-022): the
//! persisted `Chunk.embedding` rows in IST become queryable via
//! DuckDB `array_cosine_distance` even in brain_only — the indexer's
//! vectorization budget is no longer dead weight from the brain's
//! perspective.
//!
//! Vector-space coherence is guaranteed because the indexer and the
//! brain both load the same fastembed model artifact (resolved by
//! `fastembed_model()` and the snapshot pinned in `embedding_contract`),
//! through the same `build_text_embedding_model` builder.
//!
//! Failure mode: if the model fails to load (missing snapshot, IO
//! error), the worker thread exits before its loop starts; the
//! receiver is dropped, the registered sender becomes a closed
//! channel, and `request_query_embedding` surfaces the canonical
//! "worker unavailable" error so callers get a clean structural
//! fallback instead of an indefinite block.

use crossbeam_channel::bounded;
use std::thread;
use tracing::info;

use super::{register_query_embedding_sender, SemanticWorkerPool};

/// Bounded query queue depth. Brain query traffic is single-digit
/// requests-per-second under normal LLM consumption — 8 slots
/// absorbs short bursts without backpressure-blocking the dispatcher.
const CPU_QUERY_QUEUE_DEPTH: usize = 8;

/// Spawn the CPU query embedding worker if the runtime profile does
/// not own a GPU subprocess (brain_only, indexer_graph). No-op for
/// indexer_vector / indexer_full where the SemanticWorkerPool spawns
/// its own GPU-backed worker via the canonical pipeline.
pub(crate) fn spawn_brain_query_worker_if_needed(
    mode: crate::runtime_mode::AxonRuntimeMode,
) {
    if mode.semantic_workers_enabled() {
        return;
    }
    info!(
        "REQ-AXO-128: spawning in-process CPU query embedding worker for {} profile",
        mode.as_str()
    );
    let (tx, rx) = bounded(CPU_QUERY_QUEUE_DEPTH);
    register_query_embedding_sender(tx);
    thread::Builder::new()
        .name("axon-cpu-query-embed".into())
        .spawn(move || SemanticWorkerPool::query_worker_loop(0, rx))
        .expect("failed to spawn CPU query embedding worker thread (REQ-AXO-128)");
}

#[cfg(test)]
#[path = "cpu_query_service_tests.rs"]
mod cpu_query_service_tests;
