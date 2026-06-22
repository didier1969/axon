//! Production GPU wrapper for the B2 embedder slot.
//!
//! Bridges the canonical [`crate::embedder::OrtGpuFirstTextEmbedding`]
//! (ORT + TensorRT BGE-Large 1024d) to the
//! [`super::stage_b2::B2Embedder`] trait so [`super::spawn_pipeline_b_full`]
//! can drive a real GPU lane in production without leaking ORT types
//! into the public pipeline surface.
//!
//! Mutex semantics: the trait's `embed_batch(&self, ...)` is sync and
//! returns ownership of `Vec<Vec<f32>>` per call. The underlying
//! `embed_texts` needs `&mut OrtGpuFirstTextEmbedding`, so we wrap the
//! model in a `std::sync::Mutex`. Lock contention only matters when B2
//! runs with >1 worker per physical GPU, which is the anti-pattern under
//! CUDA — 1 worker per GPU is the canonical sizing (`AXON_B2_WORKERS=1`).
//!
//! DEC-AXO-901631 — the session is loaded once and kept resident for the
//! lifetime of the worker (no sleep/wake). The single-GPU live↔dev
//! cohabitation (PIL-AXO-004) is handled at the process level — the dev
//! indexer stops entirely to free the GPU for live — so a per-session
//! VRAM reclaim is redundant and only added a 1-3 s wake stutter.

use std::sync::Mutex;

use anyhow::Result;

use crate::embedder::OrtGpuFirstTextEmbedding;

use super::stage_b2::B2Embedder;

/// Wraps a single [`OrtGpuFirstTextEmbedding`] instance behind the
/// [`B2Embedder`] trait. Spawn one per physical GPU; B2 worker count
/// stays at 1 by default (CPT-AXO-054 sizing).
///
/// # Safety
///
/// The wrapped ORT session contains a `NonNull<OrtMemoryInfo>` raw FFI
/// pointer that the auto-derived `Send` / `Sync` checks reject. We
/// assert thread-safety manually because: (1) the embedder is only
/// ever accessed through `&mut self` inside [`embed_batch`] under the
/// `Mutex`, so no two threads touch the FFI handles concurrently;
/// (2) CPT-AXO-054 sizes B2 at 1 worker per physical GPU, so the only
/// thread-crossing event is the move from the build thread into the
/// B2 worker task — the ORT session handles tolerate that move.
pub struct GpuB2Embedder {
    inner: Mutex<OrtGpuFirstTextEmbedding>,
    lane: String,
    worker_idx: usize,
}

// SAFETY: see GpuB2Embedder docstring — single-threaded FFI access
// enforced by Mutex, single B2 worker per GPU enforces non-aliasing.
unsafe impl Send for GpuB2Embedder {}
unsafe impl Sync for GpuB2Embedder {}

impl GpuB2Embedder {
    /// Build a CUDA-backed (TensorRT-preferred) embedder for the
    /// pipeline-v2 vector lane.
    ///
    /// `lane` is a short identifier captured by the embedder's
    /// telemetry (e.g. `"v2-b2"`). `worker_idx` distinguishes multiple
    /// embedder instances if the operator scales past 1 GPU.
    pub fn try_new_cuda(lane: &str, worker_idx: usize) -> Result<Self> {
        let model = OrtGpuFirstTextEmbedding::try_new(lane, worker_idx, true)?;
        Ok(Self {
            inner: Mutex::new(model),
            lane: lane.to_string(),
            worker_idx,
        })
    }

    /// CPU-only fallback. Used when the operator opts out of GPU via
    /// `AXON_EMBEDDING_PROVIDER=cpu` (dev laptop, quiet-mode).
    pub fn try_new_cpu(lane: &str, worker_idx: usize) -> Result<Self> {
        let model = OrtGpuFirstTextEmbedding::try_new(lane, worker_idx, false)?;
        Ok(Self {
            inner: Mutex::new(model),
            lane: lane.to_string(),
            worker_idx,
        })
    }
}

impl B2Embedder for GpuB2Embedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut guard = self.inner.lock().map_err(|e| {
            anyhow::anyhow!(
                "GpuB2Embedder mutex poisoned (lane={}, worker={}): {e}",
                self.lane,
                self.worker_idx
            )
        })?;
        // DEC-AXO-901631 — one inference for the whole length-homogeneous
        // batch (sorted-drain guarantees the ordering ; no micro-batching).
        guard.embed_texts(texts)
    }
}
