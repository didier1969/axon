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

use super::lifecycle::process_state as embedder_state;

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

    /// Called by every embed_batch invocation (and by NOTIFY listener
    /// when work arrives). Updates `last_used_ms` and flips state to
    /// Ready if currently sleeping. Returns true iff a real wake
    /// happened (state changed from Sleeping to Ready).
    pub fn request_wake(&self) -> bool {
        self.last_used_ms.store(now_unix_ms(), Ordering::Release);
        let prev = self.phase.swap(EmbedderPhase::Ready as u8, Ordering::AcqRel);
        if prev == EmbedderPhase::Sleeping as u8 {
            self.wake_count.fetch_add(1, Ordering::AcqRel);
            true
        } else {
            false
        }
    }

    /// Called by the watchdog when idle threshold + empty pending set
    /// conditions are met. Returns true iff a real sleep happened
    /// (state changed from Ready to Sleeping).
    pub fn request_sleep(&self) -> bool {
        let prev = self
            .phase
            .swap(EmbedderPhase::Sleeping as u8, Ordering::AcqRel);
        if prev == EmbedderPhase::Ready as u8 {
            self.sleep_count.fetch_add(1, Ordering::AcqRel);
            true
        } else {
            false
        }
    }

    /// Predicate consulted by the watchdog tick. Pure compute on
    /// atomics + the runtime pending set — no I/O, no locks held.
    pub fn should_sleep(&self, now_ms: i64, t_idle: Duration, t_grace: Duration) -> bool {
        if matches!(self.phase(), EmbedderPhase::Sleeping) {
            return false;
        }
        if !embedder_state().is_empty() {
            return false;
        }
        let last = self.last_used_ms.load(Ordering::Acquire);
        let elapsed_ms = (now_ms - last).max(0);
        let threshold_ms = t_idle.as_millis().min(i64::MAX as u128) as i64;
        let grace_ms = t_grace.as_millis().min(i64::MAX as u128) as i64;
        // Both T_idle and T_grace must have elapsed since the last
        // activity. `T_grace` is the lower bound on time spent in
        // Ready so bursty wake events don't trigger an immediate
        // re-sleep.
        elapsed_ms >= threshold_ms && elapsed_ms >= grace_ms
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

/// Spawn the watchdog. Wakes once per `tick`, evaluates `should_sleep`,
/// and flips to Sleeping when conditions are met. The `on_sleep`
/// callback runs after a successful transition — typically the
/// `GpuB2Embedder::release_session` call that actually frees VRAM.
/// Pass a no-op closure (`|| {}`) when only the phase flip is wanted
/// (Slice 3A observability without the resource drop).
///
/// `t_idle` typical 5 min ; `t_grace` typical 2 s ; `tick` typical
/// 15 s. Reasonable defaults are baked into the caller (DEC-AXO-086).
pub fn spawn_idle_watchdog<F>(tick: Duration, t_idle: Duration, t_grace: Duration, on_sleep: F)
where
    F: Fn() + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now = now_unix_ms();
            if process_lifecycle().should_sleep(now, t_idle, t_grace)
                && process_lifecycle().request_sleep()
            {
                on_sleep();
                tracing::info!(
                    target: "embedder::lifecycle",
                    t_idle_ms = t_idle.as_millis() as u64,
                    "embedder transitioned Ready -> Sleeping (idle threshold reached, pending=0)"
                );
            }
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
    fn request_sleep_flips_state_and_increments_counter() {
        let l = EmbedderLifecycle::new();
        assert!(l.request_sleep());
        assert_eq!(l.phase(), EmbedderPhase::Sleeping);
        assert_eq!(l.sleep_count(), 1);
        // Already sleeping → no-op.
        assert!(!l.request_sleep());
        assert_eq!(l.sleep_count(), 1);
    }

    #[test]
    fn request_wake_flips_state_and_increments_counter() {
        let l = EmbedderLifecycle::new();
        l.request_sleep();
        assert!(l.request_wake());
        assert_eq!(l.phase(), EmbedderPhase::Ready);
        assert_eq!(l.wake_count(), 1);
        // Already ready → no-op for the counter (but last_used_ms
        // still bumps — that's the contract).
        assert!(!l.request_wake());
        assert_eq!(l.wake_count(), 1);
    }

    #[test]
    fn should_sleep_requires_empty_pending_set() {
        // Clear state at test start since process_state is global.
        embedder_state().hydrate_from_db_rows(Vec::<String>::new());
        let l = EmbedderLifecycle::new();
        // Force last_used into the past.
        l.last_used_ms.store(0, Ordering::Release);
        // Empty pending + old activity → eligible.
        assert!(l.should_sleep(
            10_000_000,
            Duration::from_millis(1_000),
            Duration::from_millis(0)
        ));
        // Mark a chunk pending → no longer eligible.
        embedder_state().mark_pending("test-chunk");
        assert!(!l.should_sleep(
            10_000_000,
            Duration::from_millis(1_000),
            Duration::from_millis(0)
        ));
        // Cleanup.
        embedder_state().mark_embedded("test-chunk");
    }

    #[test]
    fn should_sleep_honours_t_idle_threshold() {
        embedder_state().hydrate_from_db_rows(Vec::<String>::new());
        let l = EmbedderLifecycle::new();
        l.last_used_ms.store(1_000_000, Ordering::Release);
        // Now = 1_000_500 → elapsed 500 ms.
        // T_idle = 1_000 ms → not yet eligible.
        assert!(!l.should_sleep(
            1_000_500,
            Duration::from_millis(1_000),
            Duration::from_millis(0)
        ));
        // T_idle = 400 ms → eligible.
        assert!(l.should_sleep(
            1_000_500,
            Duration::from_millis(400),
            Duration::from_millis(0)
        ));
    }

    #[test]
    fn should_sleep_is_false_when_already_sleeping() {
        embedder_state().hydrate_from_db_rows(Vec::<String>::new());
        let l = EmbedderLifecycle::new();
        l.request_sleep();
        l.last_used_ms.store(0, Ordering::Release);
        assert!(!l.should_sleep(
            10_000_000,
            Duration::from_millis(1_000),
            Duration::from_millis(0)
        ));
    }

    #[test]
    fn phase_as_str_matches_repr() {
        assert_eq!(EmbedderPhase::Ready.as_str(), "ready");
        assert_eq!(EmbedderPhase::Sleeping.as_str(), "sleeping");
    }
}
