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
        // No GPU sessions (NoOp fallback path) → nothing to arm. Invariant kept
        // by REQ-AXO-902234's always-spawn change: the guard is `gpu_sessions`
        // being non-empty, NOT the policy flag.
        spawn_idle_watchdog(Vec::new(), Duration::from_secs(5));
    }

    /// REQ-AXO-902234 — the runtime override must beat the env in BOTH
    /// directions (that is why the atomic is a tri-state and not a bool).
    #[test]
    fn runtime_control_overrides_env_both_ways() {
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

        fn total_gpu_used_mib() -> u64 {
            let out = std::process::Command::new("nvidia-smi")
                .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
                .output()
                .expect("nvidia-smi must be on PATH for the ship-gate");
            String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
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
