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
use tracing::{info, warn};

use crate::embedder::lifecycle_machine::process_lifecycle;
use crate::embedder::OrtGpuFirstTextEmbedding;

use super::embed_pressure::{b2_pressure, MIN_GPU_BATCH};
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
    /// REQ-AXO-902373 — overflow lane. A GPU that cannot allocate must degrade to
    /// SLOW, never to MISSING DATA: this CPU session is built lazily on the first
    /// VRAM allocation failure and then kept, so the batch is recomputed in RAM
    /// instead of marking 64 healthy chunks `failed`. Stays `None` on hosts that
    /// never hit VRAM pressure — no cost when unused.
    cpu_overflow: Mutex<Option<OrtGpuFirstTextEmbedding>>,
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
            cpu_overflow: Mutex::new(None),
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
            cpu_overflow: Mutex::new(None),
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

/// REQ-AXO-902373 — does this error mean "the GPU could not allocate"?
///
/// The 2026-08-20 incident surfaced as an ORT node failure whose message carries the
/// allocator frame, e.g. `Non-zero status code returned while running Gather node ...
/// bfc_arena.cc:358 void* onnxruntime::BFCArena::Alloc`. The node name varies with
/// whichever operator asked for memory first, so matching on the node is useless —
/// match the ALLOCATOR instead.
///
/// Deliberately conservative: an unrecognised error still propagates. Treating a
/// genuine model error as VRAM pressure would silently move real failures to the CPU
/// lane and hide them.
pub(crate) fn is_gpu_allocation_failure(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("bfc_arena")
        || m.contains("bfcarena")
        || m.contains("out of memory")
        || m.contains("cuda_error_out_of_memory")
        || m.contains("failed to allocate memory")
}

impl GpuB2Embedder {
    /// REQ-AXO-902373 — recompute a batch on the CPU after the GPU refused to
    /// allocate. Slow (seconds vs milliseconds) and that is the point: the operator
    /// asked for VRAM pressure to cost TIME, not coverage.
    fn embed_batch_on_cpu(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut guard = match self.cpu_overflow.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_none() {
            let lane = format!("{}-cpu-overflow", self.lane);
            *guard = Some(OrtGpuFirstTextEmbedding::try_new(
                &lane,
                self.worker_idx,
                false,
            )?);
        }
        guard
            .as_mut()
            .expect("cpu overflow session present after lazy build")
            .embed_texts(texts)
    }
}

impl GpuB2Embedder {
    /// REQ-AXO-902387 — one inference on the resident session, no resizing and no
    /// fallback. The session lock is taken and released HERE so that a caller that
    /// goes on to split the batch, or to recompute it on the CPU, never holds it
    /// during the (much longer) slow path.
    fn embed_slice_resident(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
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

    /// REQ-AXO-902387 — embed one slice, HALVING it and retrying on the GPU when
    /// the arena refuses its size.
    ///
    /// This replaces "arena full → straight to CPU" (REQ-AXO-902373). That version
    /// lost no chunk and cost a factor of ~100 in throughput, silently: on
    /// 2026-08-20 every 64-chunk batch asked ORT for a 1.44 GiB arena extension,
    /// failed, and ran on the CPU while the GPU sat idle. The discriminating
    /// observation was that SMALL batches from the live feed embedded fine on the
    /// SAME arena — so the problem is per-batch demand, not the budget.
    ///
    /// The CPU lane stays as the floor: it is taken only when a single-text batch
    /// cannot allocate, which is no longer a sizing problem.
    fn embed_slice_resizing(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        embed_resizing_with(
            texts,
            b2_pressure(),
            &|slice| self.embed_slice_resident(slice),
            &|slice| self.embed_batch_on_cpu(slice),
            // `drop_session` frees the saturated arena; the next
            // `embed_slice_resident` rebuilds from the on-disk TensorRT engine
            // cache (~1-3 s warm). Returns false when nothing was resident.
            &|| self.drop_session(),
        )
    }
}

/// REQ-AXO-902387 — the resizing loop, free of ORT so it can be FALSIFIED.
///
/// The acceptance criterion is "a deliberately tiny VRAM budget never fails a
/// batch — it resizes it". That property is untestable while the loop is welded to
/// a live CUDA session, and an untestable criterion is one nobody checks. Here the
/// GPU and CPU lanes are closures, so a test can make the GPU refuse anything above
/// N and assert that every text still comes back, in order.
///
/// `record_gpu_batch` is called only for a batch the GPU actually served; a batch
/// that had to be split records a `resize` and is counted through its halves.
fn embed_resizing_with<F, C, R>(
    texts: &[String],
    pressure: &super::embed_pressure::EmbedPressure,
    embed_gpu: &F,
    embed_cpu: &C,
    recycle: &R,
) -> Result<Vec<Vec<f32>>>
where
    F: Fn(&[String]) -> Result<Vec<Vec<f32>>>,
    C: Fn(&[String]) -> Result<Vec<Vec<f32>>>,
    R: Fn() -> bool,
{
    // ONE recycle token for the whole call tree, not one per branch: a batch that
    // splits would otherwise rebuild the session once per half, and a genuinely
    // full device would loop through rebuilds instead of falling back. Caught by
    // `a_genuinely_full_device_recycles_once_then_gives_up_to_cpu`.
    let token = std::cell::Cell::new(true);
    embed_resizing_inner(texts, pressure, embed_gpu, embed_cpu, recycle, &token)
}

/// `recycle_token` is SHARED across the whole split tree and spent by the first
/// recycle, so a genuinely exhausted device cannot rebuild the session repeatedly.
fn embed_resizing_inner<F, C, R>(
    texts: &[String],
    pressure: &super::embed_pressure::EmbedPressure,
    embed_gpu: &F,
    embed_cpu: &C,
    recycle: &R,
    recycle_token: &std::cell::Cell<bool>,
) -> Result<Vec<Vec<f32>>>
where
    F: Fn(&[String]) -> Result<Vec<Vec<f32>>>,
    C: Fn(&[String]) -> Result<Vec<Vec<f32>>>,
    R: Fn() -> bool,
{
    match embed_gpu(texts) {
        Ok(vectors) => {
            pressure.record_gpu_batch(texts.len());
            Ok(vectors)
        }
        Err(err) if is_gpu_allocation_failure(&err.to_string()) => {
            if texts.len() <= MIN_GPU_BATCH {
                // REQ-AXO-902387 — the arena is exhausted, not merely tight: at one
                // text per inference there is nothing left to resize. Measured
                // 2026-08-20 19:27, with batches already down to 8:
                //   `bfc_arena.cc:358 ... Available memory of 19968 is smaller than
                //    requested bytes of 1572864`
                // 19.5 KB free. ORT's BFC arena grows MONOTONICALLY and never
                // returns memory to the device, so once it saturates no batch size
                // helps — only a NEW session does. `drop_session` + the lazy rebuild
                // in `embed_batch` (REQ-AXO-902220) already do exactly that for the
                // idle case; here the same lever is pulled under pressure.
                // Bounded to once per batch: on a genuinely full device this must
                // fall through to the CPU, not rebuild in a loop.
                if recycle_token.get() && recycle() {
                    recycle_token.set(false);
                    pressure.record_session_recycle();
                    info!(
                        error = %err,
                        "GPU arena exhausted — recycling the ORT session to return its \
                         VRAM, then retrying on GPU (REQ-AXO-902387). A rising \
                         session_recycles count means the budget is too tight for the \
                         load: free VRAM or raise AXON_GPU_RESERVE_MB."
                    );
                    return embed_resizing_inner(
                        texts,
                        pressure,
                        embed_gpu,
                        embed_cpu,
                        recycle,
                        recycle_token,
                    );
                }
                pressure.record_cpu_batch();
                warn!(
                    batch = texts.len(),
                    error = %err,
                    "GPU cannot allocate a single-text batch even after recycling the \
                     session — recomputing on CPU (slower, but no chunk is lost). The \
                     device is genuinely out of memory: free VRAM or raise \
                     AXON_GPU_RESERVE_MB. Watch b2_cpu_fallback_ratio in \
                     `embedding_status`."
                );
                return embed_cpu(texts);
            }
            let half = texts.len() / 2;
            pressure.record_resize(half);
            info!(
                from = texts.len(),
                to = half,
                "GPU refused this batch size — halving and retrying on GPU \
                 (REQ-AXO-902387). The cap is remembered for subsequent batches."
            );
            let mut out = embed_resizing_inner(
                &texts[..half],
                pressure,
                embed_gpu,
                embed_cpu,
                recycle,
                recycle_token,
            )?;
            out.extend(embed_resizing_inner(
                &texts[half..],
                pressure,
                embed_gpu,
                embed_cpu,
                recycle,
                recycle_token,
            )?);
            Ok(out)
        }
        Err(err) => Err(err),
    }
}

impl B2Embedder for GpuB2Embedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            // Empty batch: no work, no wake, no activity bump — an idle GPU
            // stays droppable.
            return Ok(Vec::new());
        }
        if !self.use_gpu {
            return self.embed_slice_resident(texts);
        }
        // Pre-split at the largest size the GPU is known to accept, so a learned
        // ceiling is applied BEFORE paying for a failure instead of rediscovering
        // it batch after batch.
        let cap = b2_pressure().effective_batch_cap(texts.len());
        if cap >= texts.len() {
            return self.embed_slice_resizing(texts);
        }
        let mut out = Vec::with_capacity(texts.len());
        for slice in texts.chunks(cap) {
            out.extend(self.embed_slice_resizing(slice)?);
        }
        Ok(out)
    }
}

/// REQ-AXO-902234 — RUNTIME idle-drop state, flipped without a restart.
///
/// `u8`: 0 = unset (fall back to the env seed), 1 = enabled, 2 = disabled. The
/// tri-state matters: it lets the control-row (written by the `idle_drop` MCP
/// tool, delivered to this process via `LISTEN embedder_control`) override the
/// env in BOTH directions, which a plain bool could not express.
///
/// Same shape as `embedder.rs::QUERY_EMBED_PROVIDER_OVERRIDE`, but fed by a PG
/// NOTIFY instead of an in-process call: the watchdog lives in `axon-indexer`
/// while the MCP tool lives in `axon-brain` (two processes).
static IDLE_DROP_OVERRIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// REQ-AXO-902234 — runtime `t_idle` seconds; 0 = unset (env seed applies).
static IDLE_SECONDS_OVERRIDE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// REQ-AXO-902234 — apply a control-plane update (called by the control
/// listener on `LISTEN embedder_control`, and at boot from the seeded row).
/// `seconds` is clamped to ≥1 s so the gate stays meaningful, mirroring the env
/// path.
pub fn apply_idle_drop_control(enabled: bool, seconds: u64) {
    IDLE_DROP_OVERRIDE.store(
        if enabled { 1 } else { 2 },
        std::sync::atomic::Ordering::Relaxed,
    );
    IDLE_SECONDS_OVERRIDE.store(seconds.max(1), std::sync::atomic::Ordering::Relaxed);
}

/// REQ-AXO-902220 — idle-drop opt-in. Default OFF: leaving it off keeps the
/// DEC-AXO-901631 always-resident behaviour (zero wake-stutter, max drain
/// throughput) for every deployment incl. the client package (MIL-AXO-043).
///
/// REQ-AXO-902234 — precedence: the RUNTIME override (control-row via NOTIFY)
/// wins when set; otherwise the `AXON_EMBEDDER_IDLE_DROP` env acts as the boot
/// SEED (operator decision D1 — the env stays the safety net on a fresh DB, so an
/// activation can never silently vanish the way it did before this REQ).
///
/// Read on EVERY watchdog tick, so a flip takes effect within one tick (5 s)
/// with no restart and no GPU teardown.
pub fn idle_drop_enabled() -> bool {
    match IDLE_DROP_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => idle_drop_enabled_from_env(),
    }
}

/// The env SEED half of [`idle_drop_enabled`] (REQ-AXO-902234 D1). Also what the
/// indexer seeds the control-row from at boot.
pub fn idle_drop_enabled_from_env() -> bool {
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
/// the gate always stays meaningful. REQ-AXO-902234: runtime override first,
/// `AXON_EMBEDDER_IDLE_SECONDS` as the boot seed.
pub fn idle_drop_seconds() -> u64 {
    match IDLE_SECONDS_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => idle_drop_seconds_from_env(),
        n => n,
    }
}

/// The env SEED half of [`idle_drop_seconds`] (REQ-AXO-902234 D1).
pub fn idle_drop_seconds_from_env() -> u64 {
    std::env::var("AXON_EMBEDDER_IDLE_SECONDS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|v| v.max(1))
        .unwrap_or(20)
}

#[cfg(test)]
pub(crate) fn reset_idle_drop_control_for_tests() {
    IDLE_DROP_OVERRIDE.store(0, std::sync::atomic::Ordering::Relaxed);
    IDLE_SECONDS_OVERRIDE.store(0, std::sync::atomic::Ordering::Relaxed);
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
/// REQ-AXO-902234 — the watchdog is now spawned unconditionally for real GPU
/// sessions and re-reads [`idle_drop_enabled`] + [`idle_drop_seconds`] on EVERY
/// tick. Two consequences, both wanted:
///   * a control-row flip takes effect within one `check_interval` (5 s) with NO
///     restart — hence no GPU teardown, the operation this REQ exists to avoid;
///   * `t_idle` is no longer frozen at spawn, so `idle_drop set seconds=…` is
///     live too.
/// When disabled the tick is a single atomic load — cheap enough to leave armed.
pub fn spawn_idle_watchdog(embedders: Vec<Arc<GpuB2Embedder>>, check_interval: Duration) {
    if embedders.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(check_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if !idle_drop_enabled() {
                continue;
            }
            let t_idle = Duration::from_secs(idle_drop_seconds());
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

    // REQ-AXO-902220 — config resolution. Env is process-global and the Rust harness runs
    // tests in PARALLEL THREADS within one process.
    //
    // REQ-AXO-902261 — the previous comment here claimed "the suite pins --test-threads=1".
    // **That was false**: there is no `.cargo/config.toml` in this repo and nothing sets
    // `--test-threads` anywhere (verified). These tests believed they were protected by a
    // guarantee that does not exist — the same shape as the "DBQ-A claim feeder drains the
    // backlog by construction" comment describing a feeder absent from the code
    // (REQ-AXO-902260). Serialised properly now, on the canonical lock.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::test_support::env_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn idle_drop_disabled_by_default_and_env_matrix() {
        let _env = env_guard();
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
        let _env = env_guard();
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
        // No GPU sessions (NoOp fallback path) → nothing to arm. Invariant kept
        // by REQ-AXO-902234's always-spawn change: the guard is `gpu_sessions`
        // being non-empty, NOT the policy flag.
        spawn_idle_watchdog(Vec::new(), Duration::from_secs(5));
    }

    /// REQ-AXO-902234 — the runtime override must beat the env in BOTH
    /// directions (that is why the atomic is a tri-state and not a bool).
    #[test]
    fn runtime_control_overrides_env_both_ways() {
        let _env = env_guard();
        reset_idle_drop_control_for_tests();
        // env says OFF …
        unsafe { std::env::remove_var("AXON_EMBEDDER_IDLE_DROP") };
        assert!(!idle_drop_enabled(), "env seed OFF applies while unset");

        // … control-row says ON → ON wins (the `idle_drop set enabled=true` path).
        apply_idle_drop_control(true, 42);
        assert!(idle_drop_enabled(), "runtime ON must beat an OFF env");
        assert_eq!(idle_drop_seconds(), 42, "runtime seconds must beat the env");

        // env says ON, control-row says OFF → OFF wins (the disable path that a
        // plain bool override could not express).
        unsafe { std::env::set_var("AXON_EMBEDDER_IDLE_DROP", "1") };
        apply_idle_drop_control(false, 7);
        assert!(!idle_drop_enabled(), "runtime OFF must beat an ON env");

        // back to unset → env seed governs again.
        reset_idle_drop_control_for_tests();
        assert!(idle_drop_enabled(), "cleared override falls back to the env seed");
        unsafe { std::env::remove_var("AXON_EMBEDDER_IDLE_DROP") };
    }

    #[test]
    fn runtime_control_clamps_zero_seconds() {
        reset_idle_drop_control_for_tests();
        apply_idle_drop_control(true, 0);
        assert_eq!(idle_drop_seconds(), 1, "0 s would make the gate meaningless");
        reset_idle_drop_control_for_tests();
    }

    /// REQ-AXO-902220 SHIP-GATE (advisor) — GPU-gated, `#[ignore]`d so the
    /// normal suite (no GPU) skips it. Run manually on a box with the GPU
    /// otherwise idle (e.g. live indexer paused) + the ORT env set:
    ///
    /// ```text
    /// cargo test --lib -- --ignored --test-threads=1 gpu_vram_reclaimed_on_drop_and_restored_on_reload
    /// ```
    ///
    /// Proves the two properties a green suite CANNOT: (1) `drop_session()`
    /// actually returns VRAM to the device (ORT/TensorRT could otherwise
    /// retain the CUDA arena); (2) the lazy reload re-establishes a working
    /// session that embeds correctly. Reads TOTAL GPU `memory.used` because
    /// WSL2 does not expose per-PID VRAM — so the GPU must be otherwise idle.
    #[test]
    #[ignore = "requires a GPU + ORT model + an otherwise-idle GPU; run with --ignored"]
    fn gpu_vram_reclaimed_on_drop_and_restored_on_reload() {
        use crate::embedding_contract::DIMENSION;

        // NVML, never the `nvidia-smi` CLI (operator rule; the crate already binds NVML
        // for exactly this). Spawning the CLI here was the last executable violation in
        // the tree, and the worst kind: on a host whose WSL2 GPU channel is wedged the
        // process enters uninterruptible D-state, so the test could not be killed — not
        // even with SIGKILL. Observed repeatedly on 2026-07-28 (REQ-AXO-902271).
        fn total_gpu_used_mib() -> u64 {
            crate::embedder::current_gpu_memory_snapshot()
                .map(|s| s.used_mb)
                .expect("NVML must answer for the ship-gate (GPU + driver required)")
        }
        let settle = || std::thread::sleep(std::time::Duration::from_millis(2000));

        let baseline = total_gpu_used_mib();

        let embedder = GpuB2Embedder::try_new_cuda("shipgate-902220", 0)
            .expect("GPU embedder init (ORT env + model required)");
        let v1 = embedder
            .embed_batch(&["fn hello() { let x = 1; }".to_string()])
            .expect("first embed");
        assert_eq!(v1.len(), 1);
        assert_eq!(v1[0].len(), DIMENSION, "canonical 1024d vector");
        settle();
        let loaded = total_gpu_used_mib();
        assert!(
            loaded > baseline + 1000,
            "resident GPU session must hold real VRAM: baseline={baseline} loaded={loaded} MiB \
             (is the GPU otherwise idle? live indexer paused?)"
        );

        // (1) Drop → VRAM must return to the device.
        assert!(embedder.drop_session(), "first drop_session returns true");
        assert!(!embedder.drop_session(), "second drop is an idempotent no-op");
        settle();
        let dropped = total_gpu_used_mib();
        assert!(
            dropped + 800 < loaded,
            "VRAM must be reclaimed after drop_session: loaded={loaded} dropped={dropped} MiB \
             (if it barely moved, ORT/TensorRT retained the CUDA arena — feature is moot)"
        );

        // (2) Reload on the next batch → valid vectors + VRAM back up.
        let v2 = embedder
            .embed_batch(&["fn reload() { let y = 2; }".to_string()])
            .expect("reload embed after drop");
        assert_eq!(v2[0].len(), DIMENSION, "reloaded session still emits 1024d");
        settle();
        let reloaded = total_gpu_used_mib();
        assert!(
            reloaded > baseline + 1000,
            "VRAM must return after wake-on-demand reload: baseline={baseline} reloaded={reloaded} MiB"
        );

        eprintln!(
            "REQ-AXO-902220 SHIP-GATE OK — baseline={baseline} loaded={loaded} \
             dropped={dropped} reloaded={reloaded} MiB"
        );
    }
}

#[cfg(test)]
mod req_902373_tests {
    use super::is_gpu_allocation_failure;
    use super::embed_resizing_with;
    use super::super::embed_pressure::EmbedPressure;
    use std::cell::RefCell;

    /// L'erreur ORT VERBATIM de l'incident du 2026-08-20, tronquée au cadre
    /// allocateur — c'est elle que la boucle doit reconnaître.
    const ARENA_FULL: &str = "Non-zero status code returned while running Add node. \
        Name:'/encoder/layer.0/attention/self/query/Add' Status Message: \
        bfc_arena.cc:358 void* onnxruntime::BFCArena::Alloc";

    /// Un GPU factice qui refuse tout lot strictement plus grand que `budget`,
    /// exactement comme une arène saturée. Enregistre les tailles tentées.
    fn gpu_with_budget(
        budget: usize,
        seen: &RefCell<Vec<usize>>,
    ) -> impl Fn(&[String]) -> anyhow::Result<Vec<Vec<f32>>> + '_ {
        move |texts: &[String]| {
            seen.borrow_mut().push(texts.len());
            if texts.len() > budget {
                anyhow::bail!("{ARENA_FULL}");
            }
            Ok(texts.iter().map(|t| vec![t.len() as f32]).collect())
        }
    }

    fn texts(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("texte-{i}")).collect()
    }

    #[test]
    fn a_tiny_budget_resizes_and_never_loses_a_chunk() {
        // Le critère d'acceptation central de REQ-AXO-902387, rendu falsifiable :
        // un budget minuscule ne doit JAMAIS faire échouer un lot.
        let seen = RefCell::new(Vec::new());
        let pressure = EmbedPressure::new();
        let batch = texts(64);
        let out = embed_resizing_with(
            &batch,
            &pressure,
            &gpu_with_budget(4, &seen),
            &|_| panic!("le lane CPU ne doit PAS être touché : 4 par lot passe sur GPU"),
            &|| panic!("aucun recyclage nécessaire : le retaillage suffit"),
        )
        .expect("un budget contraint retaille, il n'échoue pas");

        assert_eq!(out.len(), 64, "aucun morceau perdu");
        // Et dans l'ORDRE : le vecteur i doit correspondre au texte i.
        for (i, vector) in out.iter().enumerate() {
            assert_eq!(vector[0], batch[i].len() as f32, "morceau {i} désordonné");
        }
        let snap = pressure.snapshot();
        assert_eq!(snap.cpu_batches_total, 0, "aucune bascule CPU");
        assert!(snap.resizes > 0, "le lot a bien été retaillé");
        assert_eq!(snap.gpu_batch_cap, Some(4), "le plafond appris est retenu");
        assert_eq!(snap.verdict(), "healthy");
    }

    #[test]
    fn the_cpu_lane_is_the_floor_not_the_first_move() {
        // Le régime du 2026-08-20 : un lot de 64 partait DIRECTEMENT sur CPU.
        // Désormais le CPU n'est touché que si un lot d'UN SEUL texte échoue.
        let seen = RefCell::new(Vec::new());
        let pressure = EmbedPressure::new();
        let cpu_calls = RefCell::new(0usize);
        let out = embed_resizing_with(
            &texts(8),
            &pressure,
            &gpu_with_budget(0, &seen), // même un texte seul est refusé
            &|slice| {
                *cpu_calls.borrow_mut() += 1;
                Ok(slice.iter().map(|t| vec![t.len() as f32]).collect())
            },
            &|| false, // rien de résident à recycler
        )
        .expect("le CPU rattrape ce que le GPU refuse");

        assert_eq!(out.len(), 8);
        assert_eq!(*cpu_calls.borrow(), 8, "un appel CPU par texte, pas avant");
        // Le GPU a bien été RÉESSAYÉ en descendant, pas contourné d'emblée.
        assert!(
            seen.borrow().contains(&8) && seen.borrow().contains(&1),
            "la descente doit passer par 8 puis 1 : {:?}",
            seen.borrow()
        );
        assert_eq!(pressure.snapshot().verdict(), "critical");
    }

    #[test]
    fn an_exhausted_arena_is_recycled_before_falling_back_to_cpu() {
        // Le régime mesuré à 19:27 : « Available memory of 19968 is smaller than
        // requested bytes of 1572864 ». L'arène BFC ne rend jamais sa mémoire,
        // donc aucune taille de lot ne passe plus — seule une session neuve aide.
        let pressure = EmbedPressure::new();
        let recycled = RefCell::new(false);
        let out = embed_resizing_with(
            &texts(4),
            &pressure,
            &|slice: &[String]| {
                if *recycled.borrow() {
                    Ok(slice.iter().map(|t| vec![t.len() as f32]).collect())
                } else {
                    anyhow::bail!("{ARENA_FULL}")
                }
            },
            &|_| panic!("le CPU ne doit être touché QU'APRÈS un recyclage infructueux"),
            &|| {
                *recycled.borrow_mut() = true;
                true
            },
        )
        .expect("une session neuve rend l'arène et le lot passe");

        assert_eq!(out.len(), 4, "aucun morceau perdu");
        let snap = pressure.snapshot();
        assert_eq!(snap.session_recycles, 1, "un seul recyclage a suffi");
        assert_eq!(snap.cpu_batches_total, 0, "le CPU n'a pas été touché");
    }

    #[test]
    fn a_genuinely_full_device_recycles_once_then_gives_up_to_cpu() {
        // La borne : sur un appareil réellement plein, le recyclage ne doit PAS
        // boucler. Une tentative, puis le CPU.
        let pressure = EmbedPressure::new();
        let recycles = RefCell::new(0usize);
        let cpu_calls = RefCell::new(0usize);
        let out = embed_resizing_with(
            &texts(2),
            &pressure,
            &|_: &[String]| anyhow::bail!("{ARENA_FULL}"),
            &|slice: &[String]| {
                *cpu_calls.borrow_mut() += 1;
                Ok(slice.iter().map(|t| vec![t.len() as f32]).collect())
            },
            &|| {
                *recycles.borrow_mut() += 1;
                true
            },
        )
        .expect("le CPU reste le plancher");

        assert_eq!(out.len(), 2);
        assert_eq!(
            *recycles.borrow(),
            1,
            "un recyclage par lot, jamais une boucle de reconstruction"
        );
        assert_eq!(*cpu_calls.borrow(), 2);
        assert_eq!(pressure.snapshot().verdict(), "critical");
    }

    #[test]
    fn a_real_model_error_is_never_mistaken_for_vram_pressure() {
        // Garde-fou : une erreur de modèle doit REMONTER, jamais partir en CPU —
        // sinon un vrai bug se cache derrière un ralentissement.
        let pressure = EmbedPressure::new();
        let err = embed_resizing_with(
            &texts(8),
            &pressure,
            &|_| anyhow::bail!("input tensor rank mismatch: expected 2, got 3"),
            &|_| panic!("une erreur de modèle ne doit PAS toucher le lane CPU"),
            &|| panic!("une erreur de modèle ne doit PAS recycler la session"),
        )
        .expect_err("l'erreur doit remonter");
        assert!(err.to_string().contains("rank mismatch"));
        assert_eq!(
            pressure.snapshot().verdict(),
            "not_armed",
            "rien n'a été servi : la jauge reste non armée"
        );
    }


    /// The VERBATIM error from the 2026-08-20 incident. Kept literal: the guard has to
    /// recognise what the GPU actually emits, not a paraphrase of it.
    const REAL_INCIDENT_ERROR: &str = "failed ORT run_binding for embedding batch: \
Non-zero status code returned while running Gather node. \
Name:'/embeddings/word_embeddings/Gather' Status Message: \
/build/source/onnxruntime/core/framework/bfc_arena.cc:358 \
void* onnxruntime::BFCArena::Alloc(size_t) Failed to allocate memory";

    #[test]
    fn recognises_the_real_incident_error() {
        assert!(is_gpu_allocation_failure(REAL_INCIDENT_ERROR));
    }

    #[test]
    fn recognises_allocator_failures_whatever_node_asked_first() {
        // The node name varies with whichever operator requested memory first —
        // matching on it would be brittle, so we match the allocator frame.
        for msg in [
            "running Add node ... bfc_arena.cc:358 BFCArena::Alloc",
            "CUDA_ERROR_OUT_OF_MEMORY",
            "cudaMalloc failed: out of memory",
            "Failed to allocate memory for requested buffer",
        ] {
            assert!(is_gpu_allocation_failure(msg), "should match: {msg}");
        }
    }

    #[test]
    fn does_not_swallow_genuine_model_errors() {
        // Conservative on purpose: routing a real failure to the CPU lane would hide
        // it behind slowness instead of surfacing it.
        for msg in [
            "invalid input shape: expected 512 got 1414",
            "tokenizer failed to encode input",
            "ORT session not initialised",
            "engine cache is corrupt",
        ] {
            assert!(!is_gpu_allocation_failure(msg), "should NOT match: {msg}");
        }
    }
}
