use super::{
    build_token_aware_micro_batches, configured_embedding_max_length,
    cuda_execution_provider_dispatch, current_runtime_tuning_snapshot,
    current_runtime_tuning_state, embedding_lane_config_from_env, embedding_model_cache_dir,
    gpu_memory_soft_limit_mb, query_embedding_allowed_for, request_query_embedding,
    EmbeddingLaneConfig, QueryEmbeddingRequest,
};
use crate::embedding_contract::{fastembed_model, MAX_LENGTH};
use crate::service_guard::{ServicePressure, VectorRuntimeMetrics};
use crate::vector_control::{
    allowed_gpu_vector_workers, current_vector_batch_controller_diagnostics,
    current_vector_drain_state, graph_projection_allowed, reset_utility_first_scheduler_for_tests,
    semantic_policy, vector_claim_target, vector_embed_target_chunks, vector_ready_reserve_target,
    VectorBatchController, VectorBatchControllerObservation, VectorBatchControllerState,
    VectorDrainState, AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD,
    CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD,
};
use crossbeam_channel::unbounded;
use fastembed::{InitOptions, TextEmbedding};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

static ENV_TEST_GUARD: Mutex<()> = Mutex::new(());

fn lock_env_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner())
}

// REQ-AXO-291 — cross-module test serialization for tests that
// mutate `service_guard` global atomics (record_vector_*,
// reset_for_tests). The local `ENV_TEST_GUARD` only serializes
// env-var-affecting tests within this mod ; the crate-level
// `service_guard::lock_for_tests` synchronises across modules.
fn lock_service_guard() -> parking_lot::MutexGuard<'static, ()> {
    crate::service_guard::lock_for_tests()
}

// REQ-AXO-901978 (B3) — query→vector cache: hit, miss, idempotent put.
#[test]
fn query_vec_cache_stores_hit_miss_and_is_idempotent() {
    super::query_vec_cache_put("rc-cache-key-a".to_string(), vec![1.0, 2.0]);
    assert_eq!(
        super::query_vec_cache_get("rc-cache-key-a"),
        Some(vec![1.0, 2.0])
    );
    assert_eq!(super::query_vec_cache_get("rc-cache-key-absent"), None);
    // Re-put with the same key must NOT overwrite (contains_key early return).
    super::query_vec_cache_put("rc-cache-key-a".to_string(), vec![9.0]);
    assert_eq!(
        super::query_vec_cache_get("rc-cache-key-a"),
        Some(vec![1.0, 2.0])
    );
}

// REQ-AXO-901979 — query worker self-reported compute label (GPU/CPU truth).
#[test]
fn query_worker_compute_label_reflects_provider() {
    super::set_query_worker_compute_gpu(true);
    assert_eq!(super::query_worker_compute_label(), Some("GPU"));
    super::set_query_worker_compute_gpu(false);
    assert_eq!(super::query_worker_compute_label(), Some("CPU"));
    // Reset process-global state to `unknown` so this test does not pollute
    // status/embedding_status compute fallbacks in later serial tests.
    super::QUERY_WORKER_COMPUTE.store(0, std::sync::atomic::Ordering::Relaxed);
}

// REQ-AXO-901984 — runtime query-embed provider override: set/get + reload bump.
#[test]
fn query_embed_provider_override_set_get_and_bumps_reload() {
    let gen0 = super::query_reload_generation();
    assert_eq!(super::set_query_embed_provider_override("cpu"), Ok("cpu"));
    assert_eq!(super::query_embed_provider_override_label(), "cpu");
    assert!(super::query_reload_generation() > gen0);
    assert_eq!(super::set_query_embed_provider_override("GPU"), Ok("gpu"));
    assert_eq!(super::query_embed_provider_override_label(), "gpu");
    assert_eq!(super::set_query_embed_provider_override("auto"), Ok("auto"));
    assert!(super::set_query_embed_provider_override("bogus").is_err());
    // Reset the PROCESS-GLOBAL override so this test does not pollute others
    // (the suite runs single-threaded in one process; a leftover override is
    // read first by effective_provider_request_for_lane and broke
    // test_effective_provider_request_for_query_lane_respects_explicit_override).
    super::QUERY_EMBED_PROVIDER_OVERRIDE.store(0, std::sync::atomic::Ordering::Relaxed);
}

#[test]
fn query_embed_idle_timeout_parses_default_disable_and_override() {
    // REQ-AXO-901945 — default 20 min when unset / unparseable.
    assert_eq!(
        super::parse_query_embed_idle_timeout(None),
        Some(std::time::Duration::from_secs(1200))
    );
    assert_eq!(
        super::parse_query_embed_idle_timeout(Some("garbage".into())),
        Some(std::time::Duration::from_secs(1200))
    );
    // 0 disables idle-drop (model stays resident).
    assert_eq!(
        super::parse_query_embed_idle_timeout(Some("0".into())),
        None
    );
    // Explicit override honoured (with whitespace tolerance).
    assert_eq!(
        super::parse_query_embed_idle_timeout(Some("  300 ".into())),
        Some(std::time::Duration::from_secs(300))
    );
}

#[test]
fn test_semantic_policy_runs_when_system_is_healthy() {
    let _guard = lock_env_guard();
    let _guard_sg = lock_service_guard();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    crate::service_guard::record_vector_ready_queue_depth(8);
    crate::service_guard::record_vector_prepare_inflight_depth(2);
    crate::service_guard::record_vector_ready_queue_chunks(512);
    crate::service_guard::record_vector_prepare_inflight_chunks(128);
    let policy = semantic_policy(100, ServicePressure::Healthy);
    assert!(!policy.pause);
    assert_eq!(policy.profile, "balanced_drain");
    assert_eq!(policy.idle_sleep, Duration::from_millis(75));
}

#[test]
fn test_semantic_policy_prefers_aggressive_drain_under_high_healthy_backlog() {
    let _guard = lock_env_guard();
    let _guard_sg = lock_service_guard();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    crate::service_guard::record_vector_ready_queue_depth(8);
    crate::service_guard::record_vector_prepare_inflight_depth(2);
    crate::service_guard::record_vector_ready_queue_chunks(512);
    crate::service_guard::record_vector_prepare_inflight_chunks(128);
    let policy = semantic_policy(2_000, ServicePressure::Healthy);
    assert!(!policy.pause);
    assert_eq!(policy.profile, "aggressive_drain");
    assert_eq!(policy.idle_sleep, Duration::from_millis(40));
}

#[test]
fn test_graph_projection_allowed_under_queue_pressure_when_service_is_healthy() {
    reset_utility_first_scheduler_for_tests();
    assert!(graph_projection_allowed(
        2_000,
        ServicePressure::Healthy,
        0,
        false
    ));
}

#[test]
fn test_graph_projection_disallowed_when_service_is_not_healthy() {
    assert!(!graph_projection_allowed(
        100,
        ServicePressure::Recovering,
        0,
        false
    ));
    assert!(!graph_projection_allowed(
        100,
        ServicePressure::Degraded,
        0,
        false
    ));
    assert!(!graph_projection_allowed(
        100,
        ServicePressure::Critical,
        0,
        false
    ));
}

#[test]
fn test_graph_projection_ignores_large_vector_backlog_on_cpu_only_hosts() {
    reset_utility_first_scheduler_for_tests();
    assert!(graph_projection_allowed(
        100,
        ServicePressure::Healthy,
        CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD,
        false
    ));
}

#[test]
fn test_semantic_policy_pauses_when_live_service_is_critical() {
    let _guard = lock_env_guard();
    let _guard_sg = lock_service_guard();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    let policy = semantic_policy(100, ServicePressure::Critical);
    assert!(policy.pause);
    assert_eq!(policy.sleep, Duration::from_secs(2));
}

#[test]
fn test_semantic_policy_throttles_without_pausing_when_service_is_degraded() {
    let _guard = lock_env_guard();
    let _guard_sg = lock_service_guard();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    crate::service_guard::record_vector_ready_queue_depth(4);
    crate::service_guard::record_vector_prepare_inflight_depth(2);
    crate::service_guard::record_vector_ready_queue_chunks(512);
    crate::service_guard::record_vector_prepare_inflight_chunks(128);
    let policy = semantic_policy(100, ServicePressure::Degraded);
    assert!(!policy.pause);
    assert_eq!(policy.sleep, Duration::from_millis(50));
}

#[test]
fn test_semantic_policy_stays_throttled_while_service_recovers() {
    let _guard = lock_env_guard();
    let _guard_sg = lock_service_guard();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    crate::service_guard::record_vector_ready_queue_depth(4);
    crate::service_guard::record_vector_prepare_inflight_depth(2);
    crate::service_guard::record_vector_ready_queue_chunks(512);
    crate::service_guard::record_vector_prepare_inflight_chunks(128);
    let policy = semantic_policy(100, ServicePressure::Recovering);
    assert!(!policy.pause);
    assert_eq!(policy.sleep, Duration::from_millis(50));
    assert_eq!(policy.idle_sleep, Duration::from_millis(150));
}

#[test]
fn test_query_embedding_cpu_lane_gated_by_service_pressure() {
    // CPU-backed query worker competes with the brain for CPU → keep the
    // original SQL/MCP-pressure gate.
    assert!(query_embedding_allowed_for(
        ServicePressure::Healthy,
        false,
        false
    ));
    assert!(query_embedding_allowed_for(
        ServicePressure::Recovering,
        false,
        false
    ));
    assert!(!query_embedding_allowed_for(
        ServicePressure::Degraded,
        false,
        false
    ));
    assert!(!query_embedding_allowed_for(
        ServicePressure::Critical,
        false,
        false
    ));
}

#[test]
fn test_query_embedding_gpu_lane_ignores_cpu_pressure() {
    // REQ-AXO-901978 / NEX — a dedicated GPU embed is independent of the
    // CPU/SQL/MCP latency that drives ServicePressure. Even under Critical
    // CPU pressure the GPU embed runs (no self-reinforcing lexical fallback).
    assert!(query_embedding_allowed_for(
        ServicePressure::Critical,
        true,
        false
    ));
    assert!(query_embedding_allowed_for(
        ServicePressure::Degraded,
        true,
        false
    ));
    // …but it DOES back off under genuine GPU memory pressure (VRAM
    // contention with the indexer vectorization lane).
    assert!(!query_embedding_allowed_for(
        ServicePressure::Healthy,
        true,
        true
    ));
}

#[test]
fn test_apply_cpu_fallback_ort_runtime_env_restores_cpu_threads_when_autoconfigured() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_ORT_OMP_AUTOCONFIGURED", "true");
        std::env::set_var("OMP_NUM_THREADS", "1");
        std::env::set_var("OMP_WAIT_POLICY", "ACTIVE");
        std::env::set_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED", "true");
        std::env::set_var("AXON_ORT_INTRA_THREADS", "1");
    }

    let adjustment = super::apply_cpu_fallback_ort_runtime_env();

    let cpu_threads = std::env::var("OMP_NUM_THREADS")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert!(cpu_threads >= 1);
    assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "PASSIVE");
    assert_eq!(
        std::env::var("AXON_ORT_INTRA_THREADS").unwrap(),
        cpu_threads.to_string()
    );
    assert!(adjustment
        .applied_env
        .iter()
        .any(|(name, _)| name == "OMP_NUM_THREADS"));
    assert!(adjustment
        .applied_env
        .iter()
        .any(|(name, _)| name == "AXON_ORT_INTRA_THREADS"));

    unsafe {
        std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
        std::env::remove_var("OMP_NUM_THREADS");
        std::env::remove_var("OMP_WAIT_POLICY");
        std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
        std::env::remove_var("AXON_ORT_INTRA_THREADS");
    }
}

#[test]
fn test_apply_cpu_fallback_ort_runtime_env_preserves_explicit_openmp_configuration() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
        std::env::set_var("OMP_NUM_THREADS", "3");
        std::env::set_var("OMP_WAIT_POLICY", "ACTIVE");
        std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
        std::env::set_var("AXON_ORT_INTRA_THREADS", "5");
    }

    let adjustment = super::apply_cpu_fallback_ort_runtime_env();

    assert_eq!(std::env::var("OMP_NUM_THREADS").unwrap(), "3");
    assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "ACTIVE");
    assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "5");
    assert!(adjustment.applied_env.is_empty());

    unsafe {
        std::env::remove_var("OMP_NUM_THREADS");
        std::env::remove_var("OMP_WAIT_POLICY");
        std::env::remove_var("AXON_ORT_INTRA_THREADS");
    }
}

#[test]
fn test_apply_cpu_fallback_ort_runtime_env_reports_advisory_lane_sizing_when_autoconfigured() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED", "true");
        std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED", "true");
        std::env::set_var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED", "true");
        std::env::set_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED", "true");
        std::env::set_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED", "true");
        std::env::set_var("AXON_CHUNK_BATCH_SIZE", "64");
        std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE", "24");
        std::env::set_var("AXON_GRAPH_BATCH_SIZE", "8");
        std::env::set_var("AXON_VECTOR_WORKERS", "1");
        std::env::set_var("AXON_GRAPH_WORKERS", "0");
    }

    let adjustment = super::apply_cpu_fallback_ort_runtime_env();

    assert_eq!(std::env::var("AXON_CHUNK_BATCH_SIZE").unwrap(), "64");
    assert_eq!(
        std::env::var("AXON_FILE_VECTORIZATION_BATCH_SIZE").unwrap(),
        "24"
    );
    assert_eq!(std::env::var("AXON_GRAPH_BATCH_SIZE").unwrap(), "8");
    assert_eq!(std::env::var("AXON_VECTOR_WORKERS").unwrap(), "1");
    assert_eq!(std::env::var("AXON_GRAPH_WORKERS").unwrap(), "0");
    assert!(!adjustment.applied_before_pool_start);
    assert!(adjustment
        .advisory_lane_sizing
        .iter()
        .any(|(name, value)| name == "AXON_CHUNK_BATCH_SIZE" && value == "16"));
    assert!(adjustment
        .advisory_lane_sizing
        .iter()
        .any(|(name, value)| name == "AXON_FILE_VECTORIZATION_BATCH_SIZE" && value == "8"));
    assert!(adjustment
        .advisory_lane_sizing
        .iter()
        .any(|(name, value)| name == "AXON_GRAPH_BATCH_SIZE" && value == "4"));

    unsafe {
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
        std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
        std::env::remove_var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED");
        std::env::remove_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED");
        std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
        std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
        std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
        std::env::remove_var("AXON_GRAPH_BATCH_SIZE");
        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_GRAPH_WORKERS");
    }
}

#[test]
fn test_semantic_policy_pauses_while_interactive_priority_is_active() {
    let _guard = lock_env_guard();
    let _guard_sg = lock_service_guard();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    crate::service_guard::mcp_request_started();
    let policy = semantic_policy(100, ServicePressure::Healthy);
    crate::service_guard::mcp_request_finished();
    assert!(!policy.pause);
    assert_eq!(policy.profile, "interactive_guarded");
    assert_eq!(policy.sleep, Duration::from_millis(750));
}

#[test]
fn test_semantic_policy_respects_runtime_tuning_scale_pct() {
    let _guard = lock_env_guard();
    let _guard_sg = lock_service_guard();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    unsafe {
        std::env::set_var("AXON_SEMANTIC_SLEEP_SCALE_PCT", "200");
        std::env::set_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT", "200");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
    crate::service_guard::record_vector_ready_queue_depth(8);
    crate::service_guard::record_vector_prepare_inflight_depth(2);
    crate::service_guard::record_vector_ready_queue_chunks(512);
    crate::service_guard::record_vector_prepare_inflight_chunks(128);
    let policy = semantic_policy(100, ServicePressure::Healthy);
    assert_eq!(policy.profile, "balanced_drain");
    assert_eq!(policy.sleep, Duration::from_millis(50));
    assert_eq!(policy.idle_sleep, Duration::from_millis(150));

    unsafe {
        std::env::remove_var("AXON_SEMANTIC_SLEEP_SCALE_PCT");
        std::env::remove_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
}

#[test]
fn test_runtime_tuning_normalizes_queue_and_persist_bounds() {
    let _guard = lock_env_guard();
    super::apply_runtime_embedding_lane_adjustment(
        None,
        None,
        None,
        None,
        Some(128),
        Some(24),
        Some(32),
        None,
        None,
        None,
        None,
    );
    let runtime_tuning = current_runtime_tuning_state();
    assert_eq!(runtime_tuning.vector_ready_queue_depth, 128);
    assert_eq!(runtime_tuning.vector_persist_queue_bound, 24);
    assert_eq!(runtime_tuning.vector_max_inflight_persists, 24);

    unsafe {
        std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH");
        std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND");
        std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS");
        std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED");
        std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED");
        std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
}

#[test]
fn test_current_vector_drain_state_prefers_interactive_guard() {
    let state = current_vector_drain_state(5_000, ServicePressure::Healthy, true, "cuda", "cuda");
    assert_eq!(state, VectorDrainState::InteractiveGuarded);
}

#[test]
fn test_current_vector_drain_state_reports_gpu_scaling_blocked_on_cuda_fallback() {
    let state = current_vector_drain_state(256, ServicePressure::Healthy, false, "cuda", "cpu");
    assert_eq!(state, VectorDrainState::GpuScalingBlocked);
}

#[test]
fn test_current_vector_drain_state_prefers_drain_for_non_empty_backlog() {
    let state = current_vector_drain_state(8, ServicePressure::Healthy, false, "cpu", "cpu");
    assert_eq!(state, VectorDrainState::AggressiveDrain);
}

#[test]
fn test_current_vector_drain_state_prefers_quiet_when_backlog_is_empty() {
    let state = current_vector_drain_state(0, ServicePressure::Recovering, false, "cuda", "cuda");
    assert_eq!(state, VectorDrainState::QuietCruise);
}

#[test]
fn test_allowed_gpu_vector_workers_scales_only_for_meaningful_backlog() {
    assert_eq!(allowed_gpu_vector_workers(8, ServicePressure::Healthy), 2);
    assert_eq!(
        allowed_gpu_vector_workers(
            AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD,
            ServicePressure::Healthy
        ),
        6
    );
    assert_eq!(
        allowed_gpu_vector_workers(4_096, ServicePressure::Degraded),
        2
    );
}

#[test]
fn test_embedding_lane_config_from_env_supports_split_worker_counts() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_QUERY_EMBED_WORKERS", "2");
        std::env::set_var("AXON_VECTOR_WORKERS", "5");
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cpu");
        std::env::set_var("AXON_GRAPH_WORKERS", "0");
        std::env::set_var("AXON_CHUNK_BATCH_SIZE", "48");
        std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE", "24");
        std::env::set_var("AXON_GRAPH_BATCH_SIZE", "9");
        std::env::set_var("AXON_MAX_CHUNKS_PER_FILE", "12");
        std::env::set_var("AXON_MAX_EMBED_BATCH_BYTES", "65536");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
    let config = embedding_lane_config_from_env();
    unsafe {
        std::env::remove_var("AXON_QUERY_EMBED_WORKERS");
        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_GRAPH_WORKERS");
        std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
        std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
        std::env::remove_var("AXON_GRAPH_BATCH_SIZE");
        std::env::remove_var("AXON_MAX_CHUNKS_PER_FILE");
        std::env::remove_var("AXON_MAX_EMBED_BATCH_BYTES");
    }

    assert_eq!(config.query_workers, 2);
    assert_eq!(config.vector_workers, 5);
    assert_eq!(config.graph_workers, 0);
    assert_eq!(config.chunk_batch_size, 48);
    assert_eq!(config.file_vectorization_batch_size, 24);
    assert_eq!(config.graph_batch_size, 9);
    assert_eq!(config.max_chunks_per_file, 12);
    assert_eq!(config.max_embed_batch_bytes, 65536);
}

#[test]
fn test_embedding_lane_config_caps_gpu_vector_workers_without_unsafe_opt_in() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_VECTOR_WORKERS", "5");
        std::env::set_var("AXON_GPU_TOTAL_VRAM_MB_HINT", "24576");
        std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
    let config = embedding_lane_config_from_env();
    unsafe {
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_GPU_TOTAL_VRAM_MB_HINT");
    }

    assert_eq!(config.vector_workers, 3);
}

#[test]
fn test_embedding_lane_config_disables_graph_workers_with_canonical_flag() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "false");
        std::env::set_var("AXON_GRAPH_WORKERS", "5");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
    let config = embedding_lane_config_from_env();
    unsafe {
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
        std::env::remove_var("AXON_GRAPH_WORKERS");
    }

    assert_eq!(config.graph_workers, 0);
}

#[test]
fn test_embedding_lane_config_disables_vector_workers_when_runtime_mode_skips_semantic_workers() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_graph");
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_VECTOR_WORKERS", "5");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
    let config = embedding_lane_config_from_env();
    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_VECTOR_WORKERS");
    }

    assert_eq!(config.vector_workers, 0);
}

#[test]
fn test_apply_runtime_embedding_lane_adjustment_updates_live_batch_env_and_controller() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
        std::env::set_var("AXON_CHUNK_BATCH_SIZE", "48");
        std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE", "12");
        std::env::set_var("AXON_GRAPH_WORKERS", "5");
        std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
        std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
        std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
    }
    super::refresh_vector_batch_controller_from_env();

    super::apply_runtime_embedding_lane_adjustment(
        None,
        Some(2),
        Some(64),
        Some(16),
        Some(7),
        Some(3),
        Some(2),
        Some(72),
        Some(8_192),
        Some(80),
        Some(150),
    );

    let config = embedding_lane_config_from_env();
    let diagnostics = current_vector_batch_controller_diagnostics(&config);
    assert_eq!(config.chunk_batch_size, 64);
    assert_eq!(config.file_vectorization_batch_size, 16);
    assert_eq!(config.graph_workers, 2);
    let runtime_tuning = current_runtime_tuning_state();
    let runtime_snapshot = current_runtime_tuning_snapshot();
    assert_eq!(runtime_tuning.graph_workers, 2);
    assert_eq!(runtime_tuning.vector_ready_queue_depth, 7);
    assert_eq!(runtime_tuning.vector_persist_queue_bound, 3);
    assert_eq!(runtime_tuning.vector_max_inflight_persists, 2);
    assert_eq!(runtime_tuning.embed_micro_batch_max_items, 72);
    assert_eq!(runtime_tuning.embed_micro_batch_max_total_tokens, 8_192);
    assert_eq!(runtime_tuning.semantic_sleep_scale_pct, 80);
    assert_eq!(runtime_tuning.semantic_idle_sleep_scale_pct, 150);
    assert!(runtime_snapshot.version >= 2);
    assert_eq!(diagnostics.target_embed_batch_chunks, 64);
    assert_eq!(diagnostics.target_files_per_cycle, 16);
    assert_eq!(
        std::env::var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
        "false"
    );
    assert_eq!(
        std::env::var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
        "false"
    );
    assert_eq!(
        std::env::var("AXON_GRAPH_WORKERS_AUTOCONFIGURED").unwrap(),
        "false"
    );
    assert_eq!(
        std::env::var("AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED").unwrap(),
        "false"
    );
    assert_eq!(
        std::env::var("AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED").unwrap(),
        "false"
    );
    assert_eq!(
        std::env::var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED").unwrap(),
        "false"
    );
    assert_eq!(
        std::env::var("AXON_SEMANTIC_SLEEP_SCALE_PCT_AUTOCONFIGURED").unwrap(),
        "false"
    );
    assert_eq!(
        std::env::var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT_AUTOCONFIGURED").unwrap(),
        "false"
    );

    unsafe {
        std::env::set_var("AXON_CHUNK_BATCH_SIZE", "96");
    }
    let config_after_env_override = embedding_lane_config_from_env();
    let runtime_tuning_after_env_override = current_runtime_tuning_state();
    assert_eq!(config_after_env_override.chunk_batch_size, 64);
    assert_eq!(runtime_tuning_after_env_override.chunk_batch_size, 64);

    unsafe {
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
        std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
        std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
        std::env::remove_var("AXON_GRAPH_WORKERS");
        std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH");
        std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND");
        std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS");
        std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS");
        std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS");
        std::env::remove_var("AXON_SEMANTIC_SLEEP_SCALE_PCT");
        std::env::remove_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT");
        std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
        std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
        std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
        std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED");
        std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED");
        std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED");
        std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS_AUTOCONFIGURED");
        std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS_AUTOCONFIGURED");
        std::env::remove_var("AXON_SEMANTIC_SLEEP_SCALE_PCT_AUTOCONFIGURED");
        std::env::remove_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT_AUTOCONFIGURED");
    }
}

#[test]
fn test_embedding_lane_config_allows_gpu_vector_worker_oversubscription_with_opt_in() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_VECTOR_WORKERS", "5");
        std::env::set_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION", "true");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
    let config = embedding_lane_config_from_env();
    unsafe {
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
    }

    assert_eq!(config.vector_workers, 5);
}

#[test]
fn test_configured_embedding_max_length_defaults_to_model_cap() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::remove_var("AXON_EMBED_MAX_LENGTH");
    }

    assert_eq!(configured_embedding_max_length(), MAX_LENGTH);
}

#[test]
fn test_configured_embedding_max_length_honors_lower_override_and_caps_high_override() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_EMBED_MAX_LENGTH", "384");
    }
    assert_eq!(configured_embedding_max_length(), 384);

    unsafe {
        std::env::set_var("AXON_EMBED_MAX_LENGTH", "2048");
    }
    assert_eq!(configured_embedding_max_length(), MAX_LENGTH);

    unsafe {
        std::env::remove_var("AXON_EMBED_MAX_LENGTH");
    }
}

#[test]
fn test_build_token_aware_micro_batches_respects_bucket_item_and_token_limits() {
    let micro_batches = build_token_aware_micro_batches(&[24, 28, 31, 95, 100, 220], 64, 2, 160);

    assert_eq!(
        micro_batches,
        vec![vec![0, 1], vec![2], vec![3], vec![4], vec![5]]
    );
}

#[test]
fn test_build_token_aware_micro_batches_keeps_neighbors_together_when_budget_allows() {
    let micro_batches = build_token_aware_micro_batches(&[40, 43, 47, 111, 118, 121], 64, 3, 384);

    assert_eq!(micro_batches, vec![vec![0, 1, 2], vec![3, 4, 5]]);
}

#[test]
fn test_cuda_execution_provider_dispatch_is_strict() {
    let dispatch = cuda_execution_provider_dispatch();
    let rendered = format!("{dispatch:?}");

    assert!(
            rendered.contains("error_on_failure: true"),
            "CUDA EP dispatch should fail loudly so Axon can distinguish real CUDA from silent CPU fallback: {rendered}"
        );
}

#[test]
fn test_cuda_execution_provider_dispatch_omits_tf32_option_by_default() {
    unsafe {
        std::env::remove_var("AXON_CUDA_ALLOW_TF32");
    }
    let dispatch = cuda_execution_provider_dispatch();
    let rendered = format!("{dispatch:?}");
    assert!(
        !rendered.contains("use_tf32"),
        "CUDA EP dispatch should not force use_tf32=0 by default: {rendered}"
    );
}

#[test]
fn test_cuda_memory_limit_bytes_defaults_to_soft_limit_policy() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::remove_var("AXON_CUDA_MEMORY_LIMIT_MB");
        std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
        std::env::set_var("AXON_OPT_MAX_VRAM_USED_MB", "6144");
    }
    assert_eq!(super::cuda_memory_limit_bytes(), 6_144 * 1024 * 1024);
    unsafe {
        std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
    }
}

#[test]
fn test_parse_nvidia_smi_memory_csv_parses_expected_format() {
    let snapshot = super::parse_nvidia_smi_memory_csv("8192, 2242, 5779").unwrap();
    assert_eq!(snapshot.total_mb, 8192);
    assert_eq!(snapshot.used_mb, 2242);
    assert_eq!(snapshot.free_mb, 5779);
}

#[test]
fn test_gpu_memory_pressure_active_uses_soft_limit() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB", "3000");
    }
    assert!(!super::gpu_memory_pressure_active(
        super::GpuMemorySnapshot {
            total_mb: 8192,
            used_mb: 2242,
            free_mb: 5779,
        }
    ));
    assert!(super::gpu_memory_pressure_active(
        super::GpuMemorySnapshot {
            total_mb: 8192,
            used_mb: 3644,
            free_mb: 4377,
        }
    ));
    unsafe {
        std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
    }
}

#[test]
fn test_parse_nvidia_smi_utilization_csv_normalizes_percentages() {
    let parsed = super::parse_nvidia_smi_utilization_csv("31, 2").expect("parsed");
    assert!((parsed.gpu_utilization_ratio - 0.31).abs() < f64::EPSILON);
    assert!((parsed.memory_utilization_ratio - 0.02).abs() < f64::EPSILON);
}

#[test]
#[ignore = "manual runtime probe for FastEmbed CUDA init parity"]
fn manual_fastembed_cuda_init_matches_direct_ort_probe() {
    let cache_dir = super::embedding_model_cache_dir();
    let options = InitOptions::new(fastembed_model())
        .with_cache_dir(cache_dir)
        .with_show_download_progress(false)
        .with_max_length(MAX_LENGTH)
        .with_execution_providers(vec![cuda_execution_provider_dispatch()]);

    let model = TextEmbedding::try_new(options);
    assert!(
        model.is_ok(),
        "FastEmbed CUDA init should succeed under the same ORT runtime as the direct probe: {:?}",
        model.err()
    );
}

#[test]
fn test_ort_cuda_provider_library_path_uses_ort_dylib_directory() {
    unsafe {
        std::env::set_var(
            "ORT_DYLIB_PATH",
            "/nix/store/test-onnxruntime/lib/libonnxruntime.so",
        );
    }
    let provider_path = super::ort_cuda_provider_library_path();
    unsafe {
        std::env::remove_var("ORT_DYLIB_PATH");
    }

    assert_eq!(
        provider_path,
        Some(PathBuf::from(
            "/nix/store/test-onnxruntime/lib/libonnxruntime_providers_cuda.so"
        ))
    );
}

#[test]
fn test_ort_cuda_provider_library_available_is_false_without_provider_binary() {
    let tempdir = tempfile::tempdir().unwrap();
    let ort_dir = tempdir.path().join("lib");
    std::fs::create_dir_all(&ort_dir).unwrap();
    std::fs::write(ort_dir.join("libonnxruntime.so"), b"placeholder").unwrap();

    unsafe {
        std::env::set_var(
            "ORT_DYLIB_PATH",
            ort_dir.join("libonnxruntime.so").display().to_string(),
        );
    }
    let available = super::ort_cuda_provider_library_available();
    unsafe {
        std::env::remove_var("ORT_DYLIB_PATH");
    }

    assert!(!available);
}

#[test]
fn test_cpu_provider_effective_label_exposes_missing_cuda_provider() {
    assert_eq!(
        super::cpu_provider_effective_label(true, true, false),
        "cpu_missing_cuda_provider"
    );
    assert_eq!(
        super::cpu_provider_effective_label(true, false, false),
        "cpu"
    );
    assert_eq!(
        super::cpu_provider_effective_label(false, true, false),
        "cpu"
    );
    assert_eq!(super::cpu_provider_effective_label(true, true, true), "cpu");
}

#[test]
fn test_effective_provider_request_for_query_lane_uses_canonical_provider_not_forced_cpu() {
    // ec917e64 — the brain query lane no longer forces CPU when the
    // global provider is CUDA; both ORT sessions cohabit via CUDA
    // time-sharing, so the query lane returns the canonical provider
    // for the mode (like the bulk lanes). The old assertion (== "cpu")
    // encoded the removed legacy safeguard and failed deterministically.
    // Asserting equality with the canonical request keeps this
    // hardware-independent (RuntimeProfile::detect() varies by host).
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::remove_var("AXON_QUERY_EMBED_PROVIDER");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR");
    }

    let canonical = crate::runtime_mode::canonical_embedding_provider_request_for_mode(
        crate::runtime_mode::AxonRuntimeMode::from_env(),
        crate::runtime_capacity_profile::RuntimeProfile::detect().gpu_present,
    )
    .to_ascii_lowercase();
    let provider = super::effective_provider_request_for_lane("query");

    unsafe {
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
    }

    assert_eq!(
        provider, canonical,
        "query lane must use the canonical provider, not the removed legacy CPU force (ec917e64)"
    );
}

#[test]
fn test_effective_provider_request_for_query_lane_respects_explicit_override() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_QUERY_EMBED_PROVIDER", "cuda");
    }

    let provider = super::effective_provider_request_for_lane("query");

    unsafe {
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_QUERY_EMBED_PROVIDER");
    }

    assert_eq!(provider, "cuda");
}

#[test]
fn test_effective_provider_request_for_graph_lane_respects_explicit_override() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_GRAPH_EMBED_PROVIDER", "cpu");
    }

    let provider = super::effective_provider_request_for_lane("graph");

    unsafe {
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_GRAPH_EMBED_PROVIDER");
    }

    assert_eq!(provider, "cpu");
}

#[test]
fn test_embedding_model_cache_dir_defaults_outside_workspace() {
    // REQ-AXO-099 Phase 4 — env_test_lock + EnvVarGuard so the
    // mutated env vars are restored on Drop (panic-safe). The
    // prior `std::env::remove_var("HOME")` at the end of this
    // test was unconditionally clearing HOME — DuckDB's INSTALL
    // json then could not find the extension cache, causing 18
    // unrelated tests to fail in the suite. This is the root
    // cause documented in REQ-AXO-099 Phase 4.
    let _lock = crate::test_support::env_test_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _g_fastembed = crate::test_support::EnvVarGuard::unset("FASTEMBED_CACHE_DIR");
    let _g_xdg = crate::test_support::EnvVarGuard::unset("XDG_CACHE_HOME");
    let _g_home = crate::test_support::EnvVarGuard::set("HOME", "/tmp/axon-home");

    let cache_dir = embedding_model_cache_dir();

    assert_eq!(
        cache_dir,
        PathBuf::from("/tmp/axon-home/.cache/axon/fastembed")
    );
}

#[test]
fn test_cuda_tf32_enabled_defaults_to_true() {
    // REQ-AXO-262 — TF32 default flipped ON 2026-05-11. Ampere+ tensor
    // cores deliver 1.5–2× speedup on matmul with negligible accuracy
    // loss for embedding inference.
    let _guard = lock_env_guard();
    unsafe {
        std::env::remove_var("AXON_CUDA_ALLOW_TF32");
    }

    assert!(super::cuda_tf32_enabled());
}

#[test]
fn test_cuda_tf32_enabled_honors_explicit_disable() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_CUDA_ALLOW_TF32", "false");
    }
    assert!(!super::cuda_tf32_enabled());

    unsafe {
        std::env::set_var("AXON_CUDA_ALLOW_TF32", "0");
    }
    assert!(!super::cuda_tf32_enabled());

    unsafe {
        std::env::remove_var("AXON_CUDA_ALLOW_TF32");
    }
}

#[test]
fn test_vector_embed_target_chunks_prefers_larger_batches_when_mcp_is_idle() {
    let lane_config = EmbeddingLaneConfig {
        query_workers: 1,
        vector_workers: 2,
        graph_workers: 0,
        chunk_batch_size: 64,
        file_vectorization_batch_size: 24,
        graph_batch_size: 8,
        max_chunks_per_file: 64,
        max_embed_batch_bytes: 512 * 1024,
    };

    assert_eq!(vector_embed_target_chunks(&lane_config, true), 64);
    assert_eq!(vector_embed_target_chunks(&lane_config, false), 64);
}

#[test]
fn test_fetch_segments_for_file_returns_all_segments_in_line_order() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-b', 'symbol', 'sym-b', 'PRJ', '/tmp/segments.rs', 'function', 'B', 'hash-b', 20, 30), \
                 ('chunk-a', 'symbol', 'sym-a', 'PRJ', '/tmp/segments.rs', 'function', 'A', 'hash-a', 5, 10), \
                 ('chunk-c', 'symbol', 'sym-c', 'PRJ', '/tmp/segments.rs', 'function', 'C', 'hash-c', 31, 40)",
            )
            .unwrap();

    let chunks = store.fetch_segments_for_file("/tmp/segments.rs").unwrap();
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].0, "chunk-a");
    assert_eq!(chunks[1].0, "chunk-b");
    assert_eq!(chunks[2].0, "chunk-c");
}

#[test]
fn test_vector_claim_target_expands_when_ready_reserve_is_missing() {
    assert_eq!(vector_claim_target(24, 0.0, 24, 0.0, 10, 0, 4_096), 146);
    assert_eq!(vector_claim_target(24, 1.0, 24, 3.0, 10, 0, 4_096), 146);
    assert_eq!(vector_claim_target(24, 2.2, 24, 6.6, 10, 0, 4_096), 146);
    assert_eq!(vector_claim_target(3, 10.0, 12, 30.0, 8, 6, 128), 10);
}

#[test]
fn test_vector_ready_reserve_target_adds_safety_stock_when_supply_is_thin_and_low_density() {
    let reserve = vector_ready_reserve_target(32, 2_048, 24, 128, 2, 0, 36.0, 2_000);
    assert!(reserve > 32);
}

#[test]
fn test_vector_ready_reserve_target_stays_close_to_floor_when_supply_is_healthy() {
    let reserve = vector_ready_reserve_target(32, 256, 24, 96, 34, 4, 92.0, 300);
    assert!(reserve >= 32);
    assert!(reserve <= 40);
}

#[test]
fn test_bootstrap_runtime_tuning_does_not_cap_inflight_persists_to_legacy_eight() {
    let _guard = lock_env_guard();
    unsafe {
        std::env::set_var("AXON_VECTOR_PERSIST_QUEUE_BOUND", "32");
        std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS");
    }

    let snapshot = super::refresh_runtime_tuning_snapshot_from_env();

    assert_eq!(snapshot.state.vector_persist_queue_bound, 32);
    assert_eq!(snapshot.state.vector_max_inflight_persists, 32);

    unsafe {
        std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND");
    }
    super::refresh_runtime_tuning_snapshot_from_env();
}

#[test]
fn test_gpu_memory_soft_limit_mb_falls_back_to_operator_budget() {
    let _guard = lock_env_guard();
    std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
    std::env::set_var("AXON_OPT_MAX_VRAM_USED_MB", "6144");
    assert_eq!(gpu_memory_soft_limit_mb(), 6144);
    std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
}

fn controller_test_config() -> EmbeddingLaneConfig {
    EmbeddingLaneConfig {
        query_workers: 1,
        vector_workers: 1,
        graph_workers: 0,
        chunk_batch_size: 32,
        file_vectorization_batch_size: 8,
        graph_batch_size: 8,
        max_chunks_per_file: 64,
        max_embed_batch_bytes: 512 * 1024,
    }
}

fn controller_observation(
    upstream_file_pressure: usize,
    interactive_active: bool,
    gpu_memory_pressure: bool,
    embed_calls_total: u64,
    chunks_embedded_total: u64,
    files_touched_total: u64,
    embed_ms_total: u64,
) -> VectorBatchControllerObservation {
    controller_observation_with_runtime(
        upstream_file_pressure,
        interactive_active,
        gpu_memory_pressure,
        embed_calls_total,
        chunks_embedded_total,
        files_touched_total,
        embed_ms_total,
        0,
        0,
    )
}

fn controller_observation_with_runtime(
    upstream_file_pressure: usize,
    interactive_active: bool,
    gpu_memory_pressure: bool,
    embed_calls_total: u64,
    chunks_embedded_total: u64,
    files_touched_total: u64,
    embed_ms_total: u64,
    ready_queue_depth_current: u64,
    gpu_idle_wait_ms_total: u64,
) -> VectorBatchControllerObservation {
    VectorBatchControllerObservation {
        upstream_file_pressure,
        front_chunk_supply: ready_queue_depth_current as usize,
        interactive_active,
        gpu_memory_pressure,
        metrics: VectorRuntimeMetrics {
            embed_ms_total,
            batches_total: embed_calls_total,
            chunks_embedded_total,
            embed_calls_total,
            files_touched_total,
            ready_queue_depth_current,
            ready_queue_chunks_current: ready_queue_depth_current,
            gpu_idle_wait_ms_total,
            ..VectorRuntimeMetrics::default()
        },
    }
}

#[test]
fn test_vector_batch_controller_grows_targets_when_idle_backlog_is_underfed() {
    let _guard = lock_env_guard();
    super::refresh_runtime_tuning_snapshot_from_env();
    let mut controller = VectorBatchController::new(&controller_test_config());
    let diagnostics = controller.observe(
        10_000,
        controller_observation(4_096, false, false, 4, 64, 4, 20_480),
    );

    assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
    assert_eq!(diagnostics.target_embed_batch_chunks, 80);
    assert_eq!(diagnostics.target_files_per_cycle, 24);
    assert_eq!(diagnostics.adjustments_total, 1);
}

#[test]
fn test_vector_batch_controller_shrinks_targets_when_interactive_priority_activates() {
    let mut controller = VectorBatchController::new(&controller_test_config());
    controller.observe(
        10_000,
        controller_observation(4_096, false, false, 4, 64, 4, 20_480),
    );

    let diagnostics = controller.observe(
        21_000,
        controller_observation(4_096, true, false, 8, 320, 10, 40_960),
    );

    assert_eq!(
        diagnostics.state,
        VectorBatchControllerState::InteractiveGuarded
    );
    assert_eq!(diagnostics.target_embed_batch_chunks, 32);
    assert_eq!(diagnostics.target_files_per_cycle, 8);
    assert_eq!(diagnostics.adjustments_total, 2);
}

#[test]
fn test_vector_batch_controller_respects_bounds() {
    let mut controller = VectorBatchController::new(&controller_test_config());
    let mut diagnostics = current_vector_batch_controller_diagnostics(&controller_test_config());
    for idx in 1..12 {
        diagnostics = controller.observe(
            10_000 + idx * 11_000,
            controller_observation(
                8_192,
                false,
                false,
                idx * 4,
                idx * 64,
                idx * 4,
                idx * 20_480,
            ),
        );
    }

    assert_eq!(diagnostics.target_embed_batch_chunks, 128);
    assert_eq!(diagnostics.target_files_per_cycle, 32);
}

#[test]
fn test_vector_batch_controller_holds_targets_inside_cooldown() {
    let mut controller = VectorBatchController::new(&controller_test_config());
    let first = controller.observe(
        10_000,
        controller_observation(4_096, false, false, 4, 128, 4, 20_480),
    );
    let second = controller.observe(
        11_000,
        controller_observation(4_096, false, false, 8, 256, 8, 40_960),
    );

    assert_eq!(
        first.target_embed_batch_chunks,
        second.target_embed_batch_chunks
    );
    assert_eq!(first.target_files_per_cycle, second.target_files_per_cycle);
    assert_eq!(first.adjustments_total, second.adjustments_total);
}

#[test]
fn test_vector_batch_controller_preserves_last_completed_window_during_warmup() {
    let mut controller = VectorBatchController::new(&controller_test_config());
    let matured = controller.observe(
        10_000,
        controller_observation(4_096, false, false, 4, 128, 4, 20_480),
    );
    let warmup = controller.observe(
        12_000,
        controller_observation(4_096, false, false, 4, 128, 4, 20_480),
    );

    assert_eq!(matured.window_embed_calls, 4);
    assert_eq!(warmup.window_embed_calls, 4);
    assert_eq!(warmup.window_chunks, 128);
    assert_eq!(warmup.window_files_touched, 4);
}

#[test]
fn test_vector_batch_controller_uses_idle_target_during_warmup_when_backlog_is_meaningful() {
    let mut controller = VectorBatchController::new(&controller_test_config());

    let diagnostics = controller.observe(
        10_000,
        controller_observation(4_096, false, false, 0, 0, 0, 0),
    );

    assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
    assert_eq!(diagnostics.reason, "warming_window");
    assert_eq!(diagnostics.target_embed_batch_chunks, 32);
    assert_eq!(diagnostics.target_files_per_cycle, 8);
    assert_eq!(diagnostics.adjustments_total, 0);
}

#[test]
fn test_vector_batch_controller_uses_small_gpu_warmup_target_before_first_successful_embed() {
    let _guard = lock_env_guard();
    std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
    std::env::set_var("AXON_GPU_WARMUP_EMBED_BATCH_CHUNKS", "8");
    std::env::set_var("AXON_GPU_WARMUP_FILES_PER_CYCLE", "1");

    let mut controller = VectorBatchController::new(&controller_test_config());

    let diagnostics = controller.observe(
        10_000,
        controller_observation(4_096, false, false, 0, 0, 0, 0),
    );

    assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
    assert_eq!(diagnostics.reason, "warming_window");
    assert_eq!(diagnostics.target_embed_batch_chunks, 16);
    assert_eq!(diagnostics.target_files_per_cycle, 4);
    assert_eq!(diagnostics.adjustments_total, 0);

    std::env::remove_var("AXON_GPU_WARMUP_EMBED_BATCH_CHUNKS");
    std::env::remove_var("AXON_GPU_WARMUP_FILES_PER_CYCLE");
    std::env::remove_var("AXON_EMBEDDING_PROVIDER");
}

#[test]
fn test_vector_batch_controller_steps_down_after_embed_efficiency_regresses() {
    let _guard = lock_env_guard();
    super::refresh_runtime_tuning_snapshot_from_env();
    let mut controller = VectorBatchController::new(&controller_test_config());
    controller.observe(
        10_000,
        controller_observation(4_096, false, false, 4, 64, 4, 20_480),
    );
    controller.observe(
        21_000,
        controller_observation(4_096, false, false, 8, 512, 16, 61_440),
    );

    let diagnostics = controller.observe(
        32_000,
        controller_observation(4_096, false, false, 12, 768, 24, 138_240),
    );

    assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
    assert_eq!(diagnostics.target_embed_batch_chunks, 80);
    assert_eq!(diagnostics.target_files_per_cycle, 32);
    assert_eq!(diagnostics.adjustments_total, 3);
}

#[test]
fn test_single_gpu_worker_cruise_mode_waits_for_second_regression_before_step_down() {
    let _guard = lock_env_guard();
    std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
    std::env::set_var("AXON_VECTOR_WORKERS", "1");
    super::refresh_runtime_tuning_snapshot_from_env();

    let mut controller = VectorBatchController::new(&controller_test_config());
    controller.observe(
        10_000,
        controller_observation_with_runtime(4_096, false, false, 4, 64, 4, 20_480, 2_048, 0),
    );
    controller.observe(
        21_000,
        controller_observation_with_runtime(4_096, false, false, 8, 512, 16, 61_440, 2_048, 0),
    );

    let first_regression = controller.observe(
        32_000,
        controller_observation_with_runtime(4_096, false, false, 12, 896, 24, 138_240, 2_048, 0),
    );
    let second_regression = controller.observe(
        43_000,
        controller_observation_with_runtime(4_096, false, false, 16, 1_280, 32, 215_040, 2_048, 0),
    );

    assert_ne!(first_regression.reason, "embed_efficiency_regressed");
    assert_eq!(first_regression.target_embed_batch_chunks, 80);
    assert_eq!(first_regression.target_files_per_cycle, 24);
    assert_eq!(second_regression.reason, "embed_efficiency_regressed");
    assert_eq!(second_regression.target_embed_batch_chunks, 56);
    assert_eq!(second_regression.target_files_per_cycle, 24);

    std::env::remove_var("AXON_VECTOR_WORKERS");
    std::env::remove_var("AXON_EMBEDDING_PROVIDER");
}

#[test]
fn test_single_gpu_worker_cruise_mode_grows_more_aggressively_when_ready_queue_starves() {
    let _guard = lock_env_guard();
    std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
    std::env::set_var("AXON_VECTOR_WORKERS", "1");

    let mut controller = VectorBatchController::new(&controller_test_config());
    let diagnostics = controller.observe(
        10_000,
        controller_observation_with_runtime(4_096, false, false, 4, 64, 4, 20_480, 0, 0),
    );

    assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
    assert_eq!(diagnostics.reason, "ready_queue_starved");
    assert_eq!(diagnostics.target_embed_batch_chunks, 104);
    assert_eq!(diagnostics.target_files_per_cycle, 24);

    std::env::remove_var("AXON_VECTOR_WORKERS");
    std::env::remove_var("AXON_EMBEDDING_PROVIDER");
}

#[test]
fn test_single_gpu_worker_cruise_mode_reduces_chunk_target_when_ready_queue_starves_with_low_density(
) {
    let _guard = lock_env_guard();
    std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
    std::env::set_var("AXON_VECTOR_WORKERS", "1");

    let mut controller = VectorBatchController::new(&controller_test_config());
    controller.observe(
        10_000,
        controller_observation_with_runtime(4_096, false, false, 4, 64, 4, 20_480, 0, 0),
    );
    let diagnostics = controller.observe(
        21_000,
        controller_observation_with_runtime(4_096, false, false, 8, 136, 16, 56_000, 0, 0),
    );

    assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
    assert_eq!(diagnostics.reason, "ready_queue_starved_low_density");
    assert_eq!(diagnostics.target_embed_batch_chunks, 32);
    assert_eq!(diagnostics.target_files_per_cycle, 32);

    std::env::remove_var("AXON_VECTOR_WORKERS");
    std::env::remove_var("AXON_EMBEDDING_PROVIDER");
}

#[test]
fn test_persistent_low_density_underfeed_opens_file_window_more_aggressively() {
    let _guard = lock_env_guard();
    std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
    std::env::set_var("AXON_VECTOR_WORKERS", "1");
    super::refresh_runtime_tuning_snapshot_from_env();

    let lane_config = EmbeddingLaneConfig {
        chunk_batch_size: 48,
        file_vectorization_batch_size: 16,
        ..controller_test_config()
    };
    let mut controller = VectorBatchController::new(&lane_config);
    let first = controller.observe(
        10_000,
        controller_observation_with_runtime(4_096, false, false, 4, 64, 4, 20_480, 0, 0),
    );
    let second = controller.observe(
        21_000,
        controller_observation_with_runtime(4_096, false, false, 8, 156, 16, 56_000, 0, 0),
    );
    let third = controller.observe(
        32_000,
        controller_observation_with_runtime(4_096, false, false, 12, 228, 28, 84_000, 0, 0),
    );

    assert_eq!(first.reason, "ready_queue_starved");
    assert_eq!(second.reason, "ready_queue_starved_low_density");
    assert_eq!(third.reason, "ready_queue_starved_low_density");
    assert_eq!(second.target_embed_batch_chunks, 48);
    assert_eq!(third.target_embed_batch_chunks, 48);
    assert!(third.target_files_per_cycle > second.target_files_per_cycle);
    assert_eq!(second.target_files_per_cycle, 56);
    assert_eq!(third.target_files_per_cycle, 64);

    std::env::remove_var("AXON_VECTOR_WORKERS");
    std::env::remove_var("AXON_EMBEDDING_PROVIDER");
}

#[test]
fn test_vector_batch_controller_does_not_expand_file_window_when_chunk_density_is_already_good() {
    let cpu_config = EmbeddingLaneConfig {
        query_workers: 1,
        vector_workers: 1,
        graph_workers: 0,
        chunk_batch_size: 16,
        file_vectorization_batch_size: 8,
        graph_batch_size: 4,
        max_chunks_per_file: 64,
        max_embed_batch_bytes: 512 * 1024,
    };
    let mut controller = VectorBatchController::new(&cpu_config);

    let diagnostics = controller.observe(
        10_000,
        controller_observation_with_runtime(4_096, false, false, 4, 116, 4, 88_000, 12, 0),
    );

    assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
    assert_eq!(diagnostics.target_embed_batch_chunks, 64);
    assert_eq!(diagnostics.target_files_per_cycle, 24);
    assert_eq!(diagnostics.reason, "ready_queue_starved");
    assert_eq!(diagnostics.adjustments_total, 1);
}

#[test]
fn test_vector_batch_controller_shrinks_aggressively_under_gpu_memory_pressure() {
    let mut controller = VectorBatchController::new(&controller_test_config());
    controller.observe(
        10_000,
        controller_observation(4_096, false, false, 4, 64, 4, 20_480),
    );

    let diagnostics = controller.observe(
        21_000,
        controller_observation(4_096, false, true, 8, 128, 8, 40_960),
    );

    assert_eq!(diagnostics.reason, "gpu_memory_pressure");
    assert_eq!(
        diagnostics.state,
        VectorBatchControllerState::GpuMemoryGuarded
    );
    assert_eq!(diagnostics.target_embed_batch_chunks, 16);
    assert_eq!(diagnostics.target_files_per_cycle, 4);
    assert_eq!(diagnostics.adjustments_total, 2);
}

#[test]
fn test_vector_batch_controller_surfaces_gpu_memory_pressure_during_warmup() {
    let mut controller = VectorBatchController::new(&controller_test_config());

    let diagnostics = controller.observe(
        10_000,
        controller_observation(4_096, false, true, 1, 8, 1, 1_024),
    );

    assert_eq!(
        diagnostics.state,
        VectorBatchControllerState::GpuMemoryGuarded
    );
    assert_eq!(diagnostics.reason, "gpu_memory_pressure");
    assert_eq!(diagnostics.target_embed_batch_chunks, 16);
    assert_eq!(diagnostics.target_files_per_cycle, 4);
}

#[test]
fn test_request_query_embedding_round_trips_through_registered_worker() {
    let (tx, rx) = unbounded::<QueryEmbeddingRequest>();
    std::thread::spawn(move || {
        let request = rx.recv().unwrap();
        request.reply.send(Ok(vec![vec![0.42, 0.24]])).unwrap();
    });

    let embeddings = request_query_embedding(&tx, vec!["hello".to_string()]).unwrap();
    assert_eq!(embeddings, vec![vec![0.42, 0.24]]);
}

#[test]
fn test_request_query_embedding_returns_worker_disconnect_error() {
    let (tx, rx) = unbounded::<QueryEmbeddingRequest>();
    drop(rx);

    let err = request_query_embedding(&tx, vec!["hello".to_string()]).unwrap_err();
    assert!(err
        .to_string()
        .contains("MCP real-time embedding worker unavailable"));
}
