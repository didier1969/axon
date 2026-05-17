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
//! `embed_texts_with_breakdown_ort` needs `&mut OrtGpuFirstTextEmbedding`,
//! so we wrap the model in a `std::sync::Mutex`. Lock contention only
//! matters when B2 runs with >1 worker per physical GPU, which is the
//! anti-pattern under CUDA — 1 worker per GPU is the canonical sizing
//! (`AXON_B2_WORKERS=1`).
//!
//! Sleep / wake (REQ-AXO-90009 Slice 3B, DEC-AXO-086) : the inner
//! session is stored as `Option<OrtGpuFirstTextEmbedding>`. When the
//! `EmbedderLifecycle` watchdog flips the phase to Sleeping after
//! `T_idle` of inactivity, `release_session` drops the session,
//! reclaiming ~5-7 GB VRAM + the host-side ORT/TensorRT caches.
//! The next `embed_batch` call wakes the embedder by reconstructing
//! the session — TensorRT engine cache on disk keeps the reload to
//! 1-3 s warm. The Mutex serialises wake reconstruction so two
//! concurrent batches can't race on session creation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use crate::embedder::lifecycle_machine::{
    process_lifecycle, spawn_idle_watchdog, EmbedderPhase,
};
use crate::embedder::{embed_texts_with_breakdown_ort, OrtGpuFirstTextEmbedding};

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
/// B2 worker task — the ORT session handles tolerate that move. The
/// existing `vector_worker_loop` legacy path relies on the same
/// invariant (`spawn` of an OS thread that owns the embedder).
pub struct GpuB2Embedder {
    inner: Mutex<Option<OrtGpuFirstTextEmbedding>>,
    lane: String,
    worker_idx: usize,
    use_cuda: bool,
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
            inner: Mutex::new(Some(model)),
            lane: lane.to_string(),
            worker_idx,
            use_cuda: true,
        })
    }

    /// CPU-only fallback. Used when the operator opts out of GPU via
    /// `AXON_EMBEDDING_PROVIDER=cpu` (dev laptop, quiet-mode).
    pub fn try_new_cpu(lane: &str, worker_idx: usize) -> Result<Self> {
        let model = OrtGpuFirstTextEmbedding::try_new(lane, worker_idx, false)?;
        Ok(Self {
            inner: Mutex::new(Some(model)),
            lane: lane.to_string(),
            worker_idx,
            use_cuda: false,
        })
    }

    /// REQ-AXO-90009 Slice 3B — drop the inner session, releasing
    /// VRAM + the host-side ORT/TensorRT caches. Idempotent ; if the
    /// session is already None this is a no-op. Called by the idle
    /// watchdog after the lifecycle machine transitions to Sleeping.
    pub fn release_session(&self) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_some() {
            *guard = None;
            tracing::info!(
                target: "pipeline_v2::embedder_gpu",
                lane = %self.lane,
                worker_idx = self.worker_idx,
                "GpuB2Embedder session released (sleeping ; VRAM reclaimed)"
            );
        }
    }

    /// REQ-AXO-90009 Slice 3B — Arc-aware helper that spawns the
    /// idle-watchdog tokio task and wires the on-sleep callback to
    /// `release_session` of this embedder. `Arc::downgrade` is used
    /// so a paused process can drop the embedder without keeping
    /// the watchdog alive past its lifetime.
    pub fn spawn_lifecycle_watchdog(
        embedder: &Arc<Self>,
        tick: Duration,
        t_idle: Duration,
        t_grace: Duration,
    ) {
        let weak = Arc::downgrade(embedder);
        spawn_idle_watchdog(tick, t_idle, t_grace, move || {
            if let Some(strong) = weak.upgrade() {
                strong.release_session();
            }
        });
    }

    /// Test-only inspector — true iff the inner session is loaded.
    #[cfg(test)]
    pub(crate) fn is_session_loaded(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }
}

impl B2Embedder for GpuB2Embedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("GpuB2Embedder mutex poisoned: {e}"))?;
        // REQ-AXO-90009 Slice 3B — wake-on-demand. If the watchdog
        // released the session during idle, reconstruct it here.
        // TensorRT engine cache on disk (configured in gpu_backend.rs)
        // keeps the warm reload to a few seconds. Concurrent calls
        // serialise on the Mutex so only one reconstruction runs.
        if guard.is_none() {
            tracing::info!(
                target: "pipeline_v2::embedder_gpu",
                lane = %self.lane,
                worker_idx = self.worker_idx,
                phase = ?process_lifecycle().phase(),
                "GpuB2Embedder waking from sleep ; reloading session"
            );
            let model = OrtGpuFirstTextEmbedding::try_new(&self.lane, self.worker_idx, self.use_cuda)?;
            *guard = Some(model);
        }
        // Bump last_used_ms and flip phase to Ready on every embed.
        // Cheap : a single AtomicI64 store + AtomicU8 swap.
        let _was_sleeping = process_lifecycle().request_wake();
        // SAFETY of unwrap : we just ensured guard is Some.
        let model = guard.as_mut().expect("session just ensured Some");
        let (embeddings, _tokenize_ms, _host_prepare_ms, _input_copy_ms, _inference_ms, _output_extract_ms) =
            embed_texts_with_breakdown_ort(&mut *model, texts)?;
        // Belt-and-braces : after a successful embed, the phase must
        // be Ready. (The `request_wake` above already set it.)
        debug_assert_eq!(process_lifecycle().phase(), EmbedderPhase::Ready);
        Ok(embeddings)
    }
}
