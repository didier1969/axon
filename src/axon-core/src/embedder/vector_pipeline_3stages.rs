//! REQ-AXO-270 Phase 1 skeleton — 3-stage vector pipeline.
//!
//! Replaces the single-loop `vector_lane_worker` (DEC-AXO-070) with
//! three independent stages connected by bounded channels:
//!
//!   1. Producer  — claim FVQ rows, fetch chunks, tokenize  → `PreparedBatch`
//!   2. Embedder  — tight ORT GPU loop                       → `EmbeddedBatch`
//!   3. Persister — bulk INSERT + mark_done                  → `PersistedBatch`
//!
//! Phase 1 ships ONLY the skeleton: env flag, factory dispatch, stage
//! stubs that emit a single `tracing::warn!`, per-stage heartbeats.
//! Phase 2 fills in the real claim/embed/persist logic; Phase 3 benches.
//!
//! Default behavior unchanged: when `AXON_VECTOR_PIPELINE_STAGES` is
//! unset or set to `1`, the worker keeps using DEC-AXO-070 single-loop.
//! Only `3` activates the Phase 1 stub path.

use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

use crate::graph::GraphStore;
use crate::service_guard;

/// REQ-AXO-270 AC1.2 — env-driven pipeline mode selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VectorPipelineMode {
    /// DEC-AXO-070 single-loop worker (default).
    SingleLoop,
    /// REQ-AXO-270 3-stage pipeline (Phase 1 = stubs only).
    ThreeStages,
}

const ENV_FLAG: &str = "AXON_VECTOR_PIPELINE_STAGES";

/// AC1.2 — read `AXON_VECTOR_PIPELINE_STAGES`. Unrecognised values
/// fall back to `SingleLoop` so a typo cannot silently disable the
/// production lane.
pub(crate) fn vector_pipeline_mode_from_env() -> VectorPipelineMode {
    match std::env::var(ENV_FLAG)
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("3") => VectorPipelineMode::ThreeStages,
        _ => VectorPipelineMode::SingleLoop,
    }
}

/// AC1.1 — producer-stage output. Phase 1 placeholder.
#[allow(dead_code)]
pub(crate) struct PreparedBatch {
    pub(crate) _phase1_marker: (),
}

/// AC1.1 — embedder-stage output. Phase 1 placeholder.
#[allow(dead_code)]
pub(crate) struct EmbeddedBatch {
    pub(crate) _phase1_marker: (),
}

/// AC1.1 — persister-stage output. Phase 1 placeholder.
#[allow(dead_code)]
pub(crate) struct PersistedBatch {
    pub(crate) _phase1_marker: (),
}

/// AC1.4 — producer stub. Emits one warning + heartbeat.
#[allow(dead_code)]
pub(crate) fn producer_stage_stub(worker_idx: usize) {
    service_guard::record_vector_pipeline_producer_heartbeat();
    warn!(
        "Vector pipeline [{}/producer]: REQ-AXO-270 Phase 1 stub — not yet implemented",
        worker_idx
    );
}

/// AC1.4 — embedder stub. Emits one warning + heartbeat.
#[allow(dead_code)]
pub(crate) fn embedder_stage_stub(worker_idx: usize) {
    service_guard::record_vector_pipeline_embedder_heartbeat();
    warn!(
        "Vector pipeline [{}/embedder]: REQ-AXO-270 Phase 1 stub — not yet implemented",
        worker_idx
    );
}

/// AC1.4 — persister stub. Emits one warning + heartbeat.
#[allow(dead_code)]
pub(crate) fn persister_stage_stub(worker_idx: usize) {
    service_guard::record_vector_pipeline_persister_heartbeat();
    warn!(
        "Vector pipeline [{}/persister]: REQ-AXO-270 Phase 1 stub — not yet implemented",
        worker_idx
    );
}

/// AC1.3 — factory dispatch entry. Phase 1 stub: emits warnings then
/// parks the worker in a heartbeat sleep loop. Parking (rather than
/// returning) prevents axonctl from interpreting the missing thread
/// as a crash and restarting it in a hot loop.
pub(crate) fn run_vector_pipeline_3stages(worker_idx: usize, _graph_store: Arc<GraphStore>) {
    warn!(
        "Vector pipeline [{}]: REQ-AXO-270 Phase 1 skeleton active — no chunks will be embedded. \
         Unset AXON_VECTOR_PIPELINE_STAGES (or set it to 1) to revert to DEC-AXO-070 single-loop.",
        worker_idx
    );
    producer_stage_stub(worker_idx);
    embedder_stage_stub(worker_idx);
    persister_stage_stub(worker_idx);

    loop {
        service_guard::record_vector_worker_heartbeat();
        service_guard::record_vector_pipeline_producer_heartbeat();
        service_guard::record_vector_pipeline_embedder_heartbeat();
        service_guard::record_vector_pipeline_persister_heartbeat();
        std::thread::sleep(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{env_test_lock, EnvVarGuard};

    #[test]
    fn vector_pipeline_mode_defaults_to_single_loop_when_env_unset() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::unset(ENV_FLAG);
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::SingleLoop,
            "unset env var must keep DEC-AXO-070 single-loop as the default"
        );
    }

    #[test]
    fn vector_pipeline_mode_three_stages_when_env_set_to_3() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "3");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::ThreeStages
        );
    }

    #[test]
    fn vector_pipeline_mode_explicit_1_returns_single_loop() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "1");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::SingleLoop
        );
    }

    #[test]
    fn vector_pipeline_mode_falls_back_to_single_loop_on_unknown_env() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "garbage");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::SingleLoop,
            "typo / unrecognised value must NOT silently disable the production lane"
        );
    }

    #[test]
    fn vector_pipeline_mode_falls_back_to_single_loop_on_two_stages() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "2");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::SingleLoop,
            "only 1 (default) and 3 (REQ-AXO-270) are recognised stage counts"
        );
    }

    #[test]
    fn vector_pipeline_mode_trims_whitespace() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "  3 ");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::ThreeStages
        );
    }

    /// AC1.5 wiring check — each stage stub must move its dedicated
    /// per-stage heartbeat forward in `service_guard`.
    #[test]
    fn stage_stubs_advance_per_stage_heartbeats() {
        let before_p = service_guard::vector_pipeline_producer_heartbeat_at_ms();
        let before_e = service_guard::vector_pipeline_embedder_heartbeat_at_ms();
        let before_pe = service_guard::vector_pipeline_persister_heartbeat_at_ms();

        // Sleep one ms so `now_ms()` ticks past `before_*` even on the
        // fastest hosts; the heartbeat write uses the wall clock.
        std::thread::sleep(Duration::from_millis(2));

        producer_stage_stub(0);
        embedder_stage_stub(0);
        persister_stage_stub(0);

        assert!(
            service_guard::vector_pipeline_producer_heartbeat_at_ms() >= before_p
                && service_guard::vector_pipeline_producer_heartbeat_at_ms() != 0,
            "producer stub must publish its heartbeat"
        );
        assert!(
            service_guard::vector_pipeline_embedder_heartbeat_at_ms() >= before_e
                && service_guard::vector_pipeline_embedder_heartbeat_at_ms() != 0,
            "embedder stub must publish its heartbeat"
        );
        assert!(
            service_guard::vector_pipeline_persister_heartbeat_at_ms() >= before_pe
                && service_guard::vector_pipeline_persister_heartbeat_at_ms() != 0,
            "persister stub must publish its heartbeat"
        );
    }
}
