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
//! DEC-AXO-901631 — by default the session is loaded once and kept
//! resident for the lifetime of the worker (no sleep/wake), which keeps
//! the GPU saturated during a drain (≈4.5× embed throughput). The
//! single-GPU live↔dev cohabitation (PIL-AXO-004) is handled at the
//! process level — the dev indexer stops entirely to free the GPU.
//!
//! REQ-AXO-902220 — OPT-IN idle regime (default OFF, `AXON_EMBEDDER_IDLE_DROP`).
//! When enabled, [`spawn_idle_watchdog`] drops the resident session once the
//! GPU has been idle (no non-empty embed batch) for `T_idle`, returning its
//! VRAM to the device; [`GpuB2Embedder::embed_batch`] rebuilds it lazily from
//! the on-disk TensorRT engine cache (~1-3 s warm) on the next batch. This
//! adds ONLY the idle regime — during a drain `mark_used` fires each batch so
//! the watchdog never trips, leaving DEC-AXO-901631's throughput regime intact.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use tracing::info;

use crate::embedder::lifecycle_machine::process_lifecycle;
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
    /// REQ-AXO-902220 — `Option` so the idle watchdog can `take()` the ORT
    /// session (freeing VRAM) at rest and `embed_batch` can lazily rebuild it
    /// from the on-disk TensorRT engine cache on the next batch. `None` ==
    /// asleep (VRAM released) ; `Some` == resident. Under DEC-AXO-901631
    /// (idle-drop OFF) this stays `Some` for the worker's whole lifetime.
    inner: Mutex<Option<OrtGpuFirstTextEmbedding>>,
    lane: String,
    worker_idx: usize,
    /// REQ-AXO-902220 — CUDA/TensorRT provider (true) vs CPU EP (false),
    /// captured at construction so a post-idle reload restores the SAME
    /// backend the operator selected.
    use_gpu: bool,
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
            use_gpu: true,
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
            use_gpu: false,
        })
    }

    /// REQ-AXO-902220 — release the ORT session (frees VRAM) if resident.
    /// Idempotent: a no-op when already asleep. Flips the lifecycle phase to
    /// `Sleeping` UNDER the inner lock so `phase()` never disagrees with
    /// residency. Returns true iff a session was actually dropped (so the
    /// watchdog logs only real drops). Best-effort on lock poison.
    pub fn drop_session(&self) -> bool {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_some() {
            // The `OrtGpuFirstTextEmbedding` Drop impl tears down the
            // CUDA/TensorRT session → the VRAM arena returns to the device.
            *guard = None;
            process_lifecycle().mark_sleeping();
            true
        } else {
            false
        }
    }
}

impl B2Embedder for GpuB2Embedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            // Empty batch: no work, no wake, no activity bump — an idle GPU
            // stays droppable.
            return Ok(Vec::new());
        }
        let mut guard = self.inner.lock().map_err(|e| {
            anyhow::anyhow!(
                "GpuB2Embedder mutex poisoned (lane={}, worker={}): {e}",
                self.lane,
                self.worker_idx
            )
        })?;
        // REQ-AXO-902220 — wake-on-demand: the idle watchdog may have dropped
        // the session to reclaim VRAM. Rebuild it from the on-disk TensorRT
        // engine cache (~1-3 s warm, ~10 s cold) before embedding. Done under
        // the SAME lock as the drop so residency ⇔ phase stays consistent.
        if guard.is_none() {
            let model =
                OrtGpuFirstTextEmbedding::try_new(&self.lane, self.worker_idx, self.use_gpu)?;
            *guard = Some(model);
            process_lifecycle().mark_ready_woke();
        }
        // REQ-AXO-902220 — activity-time gate feed: mark the GPU used on every
        // non-empty batch. A sustained drain bumps this each batch (ms apart),
        // so the watchdog only ever sleeps a genuinely idle GPU.
        process_lifecycle().mark_used();
        // DEC-AXO-901631 — one inference for the whole length-homogeneous
        // batch (sorted-drain guarantees the ordering ; no micro-batching).
        guard
            .as_mut()
            .expect("session present after wake-on-demand rebuild")
            .embed_texts(texts)
    }
}

/// REQ-AXO-902220 — idle-drop opt-in. Default OFF: leaving it off keeps the
/// DEC-AXO-901631 always-resident behaviour (zero wake-stutter, max drain
/// throughput) for every deployment incl. the client package (MIL-AXO-043).
/// The operator flips `AXON_EMBEDDER_IDLE_DROP=1` on a workstation where
/// reclaiming idle VRAM for another GPU consumer matters more than the one-off
/// 1-3 s reload on the next indexing burst.
pub fn idle_drop_enabled() -> bool {
    matches!(
        std::env::var("AXON_EMBEDDER_IDLE_DROP")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("on")
    )
}

/// REQ-AXO-902220 — idle threshold (seconds) before the resident GPU session
/// is dropped. Default 20 s (operator-chosen, aggressive). Clamped to ≥1 s so
/// the gate always stays meaningful. Override via `AXON_EMBEDDER_IDLE_SECONDS`.
pub fn idle_drop_seconds() -> u64 {
    std::env::var("AXON_EMBEDDER_IDLE_SECONDS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|v| v.max(1))
        .unwrap_or(20)
}

/// REQ-AXO-902220 — process-level idle VRAM reclamation watchdog.
///
/// Ticks every `check_interval` and, when the shared
/// [`crate::embedder::lifecycle_machine::EmbedderLifecycle`] reports the GPU
/// has been idle (no non-empty embed batch) for `t_idle`, drops every resident
/// session so the VRAM returns to the device. The next batch rebuilds lazily
/// via wake-on-demand ([`GpuB2Embedder::embed_batch`]).
///
/// Adds the *idle* regime ONLY — during an active drain `mark_used` fires each
/// batch, so `should_drop` never trips and DEC-AXO-901631's throughput regime
/// is untouched. Spawn ONLY for real GPU sessions and ONLY when
/// [`idle_drop_enabled`] (default OFF).
///
/// Multi-worker note: all `GpuB2Embedder` instances share the ONE process
/// lifecycle singleton, so `last_used` is global — the watchdog drops every
/// session together on global idle, and each reloads independently on its next
/// batch.
pub fn spawn_idle_watchdog(
    embedders: Vec<Arc<GpuB2Embedder>>,
    t_idle: Duration,
    check_interval: Duration,
) {
    if embedders.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(check_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if process_lifecycle().should_drop_now(t_idle) {
                let mut dropped = 0usize;
                for embedder in &embedders {
                    if embedder.drop_session() {
                        dropped += 1;
                    }
                }
                if dropped > 0 {
                    info!(
                        dropped,
                        t_idle_s = t_idle.as_secs(),
                        "REQ-AXO-902220 idle watchdog: released {dropped} GPU embedder session(s) \
                         — VRAM reclaimed; next batch reloads from the TensorRT engine cache"
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ-AXO-902220 — config resolution (env is process-global; the suite
    // pins --test-threads=1, mirroring stage_b2's timeout test).

    #[test]
    fn idle_drop_disabled_by_default_and_env_matrix() {
        unsafe { std::env::remove_var("AXON_EMBEDDER_IDLE_DROP") };
        assert!(!idle_drop_enabled(), "default OFF preserves DEC-AXO-901631");

        for truthy in ["1", "true", "TRUE", "yes", "on"] {
            unsafe { std::env::set_var("AXON_EMBEDDER_IDLE_DROP", truthy) };
            assert!(idle_drop_enabled(), "{truthy} enables opt-in");
        }
        for falsy in ["0", "false", "no", ""] {
            unsafe { std::env::set_var("AXON_EMBEDDER_IDLE_DROP", falsy) };
            assert!(!idle_drop_enabled(), "{falsy:?} stays OFF");
        }
        unsafe { std::env::remove_var("AXON_EMBEDDER_IDLE_DROP") };
    }

    #[test]
    fn idle_drop_seconds_defaults_to_twenty_and_clamps_zero() {
        unsafe { std::env::remove_var("AXON_EMBEDDER_IDLE_SECONDS") };
        assert_eq!(idle_drop_seconds(), 20, "operator default");

        unsafe { std::env::set_var("AXON_EMBEDDER_IDLE_SECONDS", "300") };
        assert_eq!(idle_drop_seconds(), 300, "explicit override honoured");

        unsafe { std::env::set_var("AXON_EMBEDDER_IDLE_SECONDS", "0") };
        assert_eq!(idle_drop_seconds(), 1, "0 clamps to 1 s (gate stays meaningful)");

        unsafe { std::env::remove_var("AXON_EMBEDDER_IDLE_SECONDS") };
    }

    #[test]
    fn empty_watchdog_fleet_is_a_noop_and_does_not_panic() {
        // No GPU sessions (NoOp fallback path) → nothing to arm.
        spawn_idle_watchdog(Vec::new(), Duration::from_secs(20), Duration::from_secs(5));
    }
}
