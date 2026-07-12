//! Embedder lifecycle state machine (REQ-AXO-90009 Slice 3, DEC-AXO-086).
//!
//! Sits above [`crate::embedder::lifecycle::EmbedderRuntimeState`] and
//! decides when to put the GPU embedder session to sleep / wake it up
//! based on activity.
//!
//! Two states :
//!   * `Ready` — session loaded, VRAM + RAM cost paid.
//!   * `Sleeping` — session dropped, VRAM + most heap reclaimed. A wake
//!     call reloads via TensorRT engine cache (~ 1-3 s warm, ~ 10 s cold).
//!
//! Transitions :
//!   * `Ready -> Sleeping` : runtime pending set empty AND
//!     `time_since_last_use >= T_idle` (default 5 min).
//!   * `Sleeping -> Ready` : `request_wake()` (called by the embed
//!     batch entry or a NOTIFY/watcher event). `T_grace` of 2 s
//!     guards against immediate re-sleep on bursty wake events.
//!
//! This module ships the lifecycle controls + the watchdog ; the
//! actual GpuB2Embedder drop/recreate dance ships in a follow-up
//! that refactors the inner session into `Option<...>`. Until then,
//! transitions still publish state via `phase()` so observability
//! (`embedding_status`) reports what the system would do, paving the
//! way for the wiring commit.

use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;


/// Wire-compatible enum for telemetry / MCP heartbeat. Numeric repr
/// matches the underlying AtomicU8 so reads stay lock-free.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EmbedderPhase {
    Ready = 0,
    Sleeping = 1,
}

impl EmbedderPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Sleeping => "sleeping",
        }
    }

    fn from_repr(value: u8) -> Self {
        match value {
            1 => Self::Sleeping,
            _ => Self::Ready,
        }
    }
}

/// Sleep/wake controller. Holds the atomic state + the last-used
/// timestamp (epoch ms). Sleep decision is taken by the watchdog
/// task ; wake is requested by the embed entry point or a NOTIFY
/// listener that has just enqueued work.
pub struct EmbedderLifecycle {
    phase: AtomicU8,
    last_used_ms: AtomicI64,
    wake_count: AtomicI64,
    sleep_count: AtomicI64,
}

impl EmbedderLifecycle {
    pub fn new() -> Self {
        Self {
            phase: AtomicU8::new(EmbedderPhase::Ready as u8),
            last_used_ms: AtomicI64::new(now_unix_ms()),
            wake_count: AtomicI64::new(0),
            sleep_count: AtomicI64::new(0),
        }
    }

    pub fn phase(&self) -> EmbedderPhase {
        EmbedderPhase::from_repr(self.phase.load(Ordering::Acquire))
    }

    pub fn last_used_ms(&self) -> i64 {
        self.last_used_ms.load(Ordering::Acquire)
    }

    pub fn wake_count(&self) -> i64 {
        self.wake_count.load(Ordering::Acquire)
    }

    pub fn sleep_count(&self) -> i64 {
        self.sleep_count.load(Ordering::Acquire)
    }

    /// REQ-AXO-902220 — record embedder activity (a non-empty embed batch).
    /// The idle watchdog keys its drop decision PURELY on this timestamp,
    /// never on a backlog counter: a chronically-non-empty queue (a language
    /// with no embedding model, repeat-fail chunks) must not wedge the GPU
    /// awake forever — that is the 901931 failure-mode reincarnated at the
    /// queue layer (advisor review, s101).
    pub fn mark_used(&self) {
        self.last_used_ms.store(now_unix_ms(), Ordering::Release);
    }

    /// REQ-AXO-902220 — the ORT session was just rebuilt after an idle drop.
    /// Flips `Sleeping → Ready`, counts the wake, and refreshes `last_used`
    /// so the watchdog cannot immediately re-sleep a session that just woke.
    /// Called UNDER the embedder's inner lock (see `GpuB2Embedder`).
    pub fn mark_ready_woke(&self) {
        self.phase
            .store(EmbedderPhase::Ready as u8, Ordering::Release);
        self.wake_count.fetch_add(1, Ordering::AcqRel);
        self.last_used_ms.store(now_unix_ms(), Ordering::Release);
    }

    /// REQ-AXO-902220 — the ORT session was just dropped (VRAM released).
    /// Flips `Ready → Sleeping` and counts the sleep. Called UNDER the
    /// embedder's inner lock so `phase()` never disagrees with whether the
    /// session is actually resident.
    pub fn mark_sleeping(&self) {
        self.phase
            .store(EmbedderPhase::Sleeping as u8, Ordering::Release);
        self.sleep_count.fetch_add(1, Ordering::AcqRel);
    }

    /// REQ-AXO-902220 — pure idle-drop decision (activity-time gate).
    /// Returns true iff the session is resident (`Ready`) AND no non-empty
    /// embed batch has run for `t_idle`. `now_ms` is injected so the gate is
    /// deterministically unit-testable. Leak-proof: reads only the activity
    /// clock, so no backlog residue can keep it awake.
    pub fn should_drop(&self, t_idle: Duration, now_ms: i64) -> bool {
        if self.phase() != EmbedderPhase::Ready {
            return false;
        }
        let idle_ms = now_ms.saturating_sub(self.last_used_ms());
        idle_ms >= t_idle.as_millis().min(i64::MAX as u128) as i64
    }

    /// REQ-AXO-902220 — [`should_drop`] evaluated against the wall clock.
    /// Used by the runtime watchdog; the injectable variant backs the tests.
    pub fn should_drop_now(&self, t_idle: Duration) -> bool {
        self.should_drop(t_idle, now_unix_ms())
    }
}

impl Default for EmbedderLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-level singleton (same pattern as
/// [`crate::embedder::lifecycle::process_state`]). Any module that
/// embeds (B2 entry) or observes (`embedding_status`, brain telemetry)
/// shares the same lifecycle controller.
pub fn process_lifecycle() -> &'static Arc<EmbedderLifecycle> {
    static LIFE: OnceLock<Arc<EmbedderLifecycle>> = OnceLock::new();
    LIFE.get_or_init(|| Arc::new(EmbedderLifecycle::new()))
}

/// Snapshot of the local `EmbedderLifecycle` state for cross-process
/// publication via `axon.EmbedderLifecycleHeartbeat`
/// (REQ-AXO-91572 option B). Captured at heartbeat tick by the
/// indexer ; consumed by the brain `embedding_status` MCP tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleHeartbeatSnapshot {
    pub phase: EmbedderPhase,
    pub last_used_ms: i64,
    pub wake_count: i64,
    pub sleep_count: i64,
    pub pending_count: i64,
    pub heartbeat_ms: i64,
    /// DEC-AXO-901626 — observed compute verdict for THIS process, captured
    /// by self-observation (`crate::observed_gpu::observed_self_compute`).
    /// "GPU" | "CPU".
    pub compute: String,
    /// How `compute` was determined: "nvidia_smi" | "unknown".
    pub compute_source: String,
    /// DEC-AXO-901626 — release identity (`AXON_BUILD_ID`) of the
    /// publishing process. `None` when the env var is unset.
    pub build_id: Option<String>,
    /// REQ-AXO-902047 slice 1b — B3 (embedding persist) stage health, so the
    /// brain's `embedding_status` surfaces the real PG error + systemic-failure
    /// verdict in one MCP call without log access (the REQ-AXO-902046 incident
    /// took gdb + 4 h to diagnose because the error was process-local).
    pub b3: crate::pipeline::stage_health::StageHealthSnapshot,
}

impl LifecycleHeartbeatSnapshot {
    /// Capture the process-singleton state. The compute verdict is the one
    /// non-trivial read: a single self-`nvidia-smi` probe (timeout-bounded),
    /// run on the heartbeat cadence (~5 s), never in a hot loop.
    pub fn capture() -> Self {
        let lc = process_lifecycle();
        let (compute, compute_source) = crate::observed_gpu::observed_self_compute();
        Self {
            phase: lc.phase(),
            last_used_ms: lc.last_used_ms(),
            wake_count: lc.wake_count(),
            sleep_count: lc.sleep_count(),
            pending_count: super::lifecycle::process_state().pending_count() as i64,
            heartbeat_ms: now_unix_ms(),
            compute: compute.to_string(),
            compute_source: compute_source.to_string(),
            build_id: std::env::var("AXON_BUILD_ID")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            b3: crate::pipeline::stage_health::b3_health().snapshot(),
        }
    }
}

/// Spawn the heartbeat-publisher loop. Captures the local lifecycle
/// state every `tick` and forwards it via `publish` (typically an
/// UPSERT into `axon.EmbedderLifecycleHeartbeat`).
///
/// Decoupled from `spawn_idle_watchdog` so the writer cadence + the
/// sleep threshold can be tuned independently (REQ-AXO-91572 option
/// B). `tick` typical 5 s.
pub fn spawn_lifecycle_heartbeat_publisher<F>(tick: Duration, mut publish: F)
where
    F: FnMut(LifecycleHeartbeatSnapshot) + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            publish(LifecycleHeartbeatSnapshot::capture());
        }
    });
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_lifecycle_starts_ready() {
        let l = EmbedderLifecycle::new();
        assert_eq!(l.phase(), EmbedderPhase::Ready);
        assert_eq!(l.wake_count(), 0);
        assert_eq!(l.sleep_count(), 0);
    }

    #[test]
    fn phase_as_str_matches_repr() {
        assert_eq!(EmbedderPhase::Ready.as_str(), "ready");
        assert_eq!(EmbedderPhase::Sleeping.as_str(), "sleeping");
    }

    // REQ-AXO-902220 — idle-drop transition + activity-time gate.

    #[test]
    fn should_drop_false_when_recently_used() {
        let l = EmbedderLifecycle::new();
        // 1 s of idle against a 20 s threshold → keep resident.
        let now = l.last_used_ms() + 1_000;
        assert!(!l.should_drop(Duration::from_secs(20), now));
    }

    #[test]
    fn should_drop_true_after_t_idle_elapsed_and_ready() {
        let l = EmbedderLifecycle::new();
        // Just past the 20 s threshold and still Ready → drop.
        let now = l.last_used_ms() + 20_001;
        assert!(l.should_drop(Duration::from_secs(20), now));
    }

    #[test]
    fn should_drop_false_when_already_sleeping() {
        let l = EmbedderLifecycle::new();
        l.mark_sleeping();
        assert_eq!(l.phase(), EmbedderPhase::Sleeping);
        // Even 10 min idle: nothing to drop when already asleep.
        let now = l.last_used_ms() + 10 * 60_000;
        assert!(!l.should_drop(Duration::from_secs(20), now));
    }

    #[test]
    fn sleep_then_wake_roundtrip_counts_transitions() {
        let l = EmbedderLifecycle::new();
        assert_eq!(l.phase(), EmbedderPhase::Ready);
        assert_eq!(l.sleep_count(), 0);
        assert_eq!(l.wake_count(), 0);

        l.mark_sleeping();
        assert_eq!(l.phase(), EmbedderPhase::Sleeping);
        assert_eq!(l.sleep_count(), 1);

        l.mark_ready_woke();
        assert_eq!(l.phase(), EmbedderPhase::Ready);
        assert_eq!(l.wake_count(), 1);
    }

    #[test]
    fn mark_used_resets_the_idle_clock() {
        let l = EmbedderLifecycle::new();
        let t0 = l.last_used_ms();
        // Idle far past the threshold at t0.
        assert!(l.should_drop(Duration::from_secs(20), t0 + 30_000));
        // A batch runs now → activity clock refreshes → gate flips false.
        l.mark_used();
        let now = l.last_used_ms() + 100;
        assert!(
            !l.should_drop(Duration::from_secs(20), now),
            "fresh activity must reset the idle clock"
        );
    }

    #[test]
    fn drain_regime_never_sleeps_a_busy_gpu() {
        // Simulate a sustained drain: every batch bumps `last_used`. As long
        // as the last batch is within T_idle, should_drop stays false — the
        // watchdog cannot touch DEC-AXO-901631's throughput regime.
        let l = EmbedderLifecycle::new();
        for _ in 0..5 {
            l.mark_used();
            let now = l.last_used_ms() + 500; // 0.5 s between batches
            assert!(!l.should_drop(Duration::from_secs(20), now));
        }
    }
}
