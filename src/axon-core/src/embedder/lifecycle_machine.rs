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
}
