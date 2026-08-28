//! REQ-AXO-902547 / DEC-AXO-901676 — isolated query-time embedding for
//! non-indexer profiles (brain_only, indexer_graph).
//!
//! The brain process supervises a dedicated query-embedding subprocess.
//! The child alone owns fastembed/ORT and may use the GPU; stopping it is
//! therefore a deterministic way to release the model, native arenas and
//! VRAM after the idle deadline. The supervisor registers the ordinary
//! query-embedding channel, so `batch_embed` keeps the same caller contract.
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
//! Failure mode: a missing model, child crash or broken IPC is surfaced to the
//! caller; the supervisor retries once with a fresh child and never leaves ORT
//! resident inside the Brain process.

use crossbeam_channel::bounded;
use tracing::info;

use super::{query_embed_service, register_query_embedding_sender};

/// Bounded query queue depth. Brain query traffic is single-digit
/// requests-per-second under normal LLM consumption — 8 slots
/// absorbs short bursts without backpressure-blocking the dispatcher.
const CPU_QUERY_QUEUE_DEPTH: usize = 8;

/// Spawn the isolated query service if this profile has no semantic pipeline.
/// ORT never enters the Brain process; child exit is the deterministic arena
/// reclamation primitive.
pub(crate) fn spawn_brain_query_worker_if_needed(mode: crate::runtime_mode::AxonRuntimeMode) {
    if mode.semantic_workers_enabled() {
        return;
    }
    info!(
        "REQ-AXO-902547: spawning isolated query embedding service for {} profile",
        mode.as_str()
    );
    let (tx, rx) = bounded(CPU_QUERY_QUEUE_DEPTH);
    register_query_embedding_sender(tx);
    query_embed_service::spawn_supervisor(rx)
        .expect("failed to spawn isolated query embedding supervisor (REQ-AXO-902547)");
}

#[cfg(test)]
#[path = "cpu_query_service_tests.rs"]
mod cpu_query_service_tests;
