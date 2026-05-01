use crate::runtime_mode::graph_embeddings_enabled;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct RuntimeProfile {
    pub cpu_cores: usize,
    pub ram_total_gb: u64,
    pub ram_budget_gb: u64,
    pub ingestion_memory_budget_gb: u64,
    pub gpu_present: bool,
    pub recommended_workers: usize,
    pub max_blocking_threads: usize,
    pub queue_capacity: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingLaneSizing {
    pub query_workers: usize,
    pub vector_workers: usize,
    pub graph_workers: usize,
    pub chunk_batch_size: usize,
    pub file_vectorization_batch_size: usize,
    pub graph_batch_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelinePriorityLane {
    pub lane: &'static str,
    pub priority: &'static str,
    pub admission_requires: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimePriorityContractState {
    pub watcher_identification_backlog_gated: bool,
    pub graphing_after_enqueue_backlog_gated: bool,
    pub vectorization_after_graph_ready_backlog_gated: bool,
    pub vectorization_allowed_ahead_of_graph_backlog: bool,
    pub graph_backlog_present: bool,
    pub enforcement_state: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionControllerProfile {
    pub target_band: usize,
    pub reorder_point: usize,
    pub max_wip: usize,
    pub hold_window_ms: u64,
    pub forced_bulk_fill_threshold: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionControllerState {
    pub profile: AdmissionControllerProfile,
    pub blocking_authority: &'static str,
    pub admission_open: bool,
    pub bulk_fill_preferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphProductionState {
    pub blocking_authority: &'static str,
    pub graph_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorDownstreamState {
    pub blocking_authority: &'static str,
    pub vector_open: bool,
}

impl RuntimeProfile {
    pub fn detect() -> Self {
        let cpu_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let ram_total_gb = detect_total_ram_gb().unwrap_or(16);
        let ram_budget_gb = detect_ram_budget_gb(ram_total_gb);
        let ingestion_memory_budget_gb = detect_ingestion_memory_budget_gb(ram_budget_gb);
        let gpu_present = detect_gpu_presence();
        let sizing = recommend_sizing(cpu_cores, ram_total_gb, gpu_present);
        let max_workers = configured_max_worker_cap().unwrap_or(sizing.recommended_workers);
        let recommended_workers = sizing.recommended_workers.min(max_workers).max(1);

        Self {
            cpu_cores,
            ram_total_gb,
            ram_budget_gb,
            ingestion_memory_budget_gb,
            gpu_present,
            recommended_workers,
            max_blocking_threads: sizing.max_blocking_threads,
            queue_capacity: sizing.queue_capacity,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RecommendedSizing {
    recommended_workers: usize,
    max_blocking_threads: usize,
    queue_capacity: usize,
}

fn detect_total_ram_gb() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = meminfo
        .lines()
        .find(|line| line.starts_with("MemTotal:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?;
    Some((kb / 1024 / 1024).max(1))
}

fn configured_max_worker_cap() -> Option<usize> {
    std::env::var("MAX_AXON_WORKERS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|workers| *workers >= 1)
}

fn detect_ram_budget_gb(total_gb: u64) -> u64 {
    let total_gb = total_gb.max(1);
    let reserved_headroom_gb = ((total_gb as f64) * 0.25).ceil() as u64;
    total_gb
        .saturating_sub(reserved_headroom_gb.clamp(2, 16))
        .max(2)
}

fn detect_ingestion_memory_budget_gb(ram_budget_gb: u64) -> u64 {
    let ram_budget_gb = ram_budget_gb.max(2);
    let target_budget_gb = ((ram_budget_gb as f64) * 0.35).ceil() as u64;
    target_budget_gb
        .min(ram_budget_gb.saturating_sub(2).max(2))
        .max(2)
}

fn wsl_cuda_runtime_available(osrelease: &str, libcuda_present: bool) -> bool {
    libcuda_present && osrelease.to_ascii_lowercase().contains("microsoft")
}

fn detect_gpu_presence() -> bool {
    Path::new("/dev/dri/renderD128").exists()
        || Path::new("/dev/nvidia0").exists()
        || Path::new("/proc/driver/nvidia/version").exists()
        || wsl_cuda_runtime_available(
            &std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default(),
            Path::new("/usr/lib/wsl/lib/libcuda.so.1").exists(),
        )
}

fn recommend_sizing(cpu_cores: usize, ram_total_gb: u64, gpu_present: bool) -> RecommendedSizing {
    let cpu_cores = cpu_cores.max(2);
    let ram_budget_gb = detect_ram_budget_gb(ram_total_gb);
    let worker_cap_by_ram = ((ram_budget_gb as f64) / 1.5).floor() as usize;

    let base_workers = cpu_cores
        .saturating_sub(2)
        .max(2)
        .min(worker_cap_by_ram.max(2));
    let recommended_workers = if gpu_present {
        base_workers.min(12)
    } else {
        base_workers
    };

    let max_blocking_threads = ((recommended_workers as f64) * 0.75).ceil() as usize;
    let max_blocking_threads = max_blocking_threads.clamp(4, 16);

    let queue_capacity = ram_budget_gb
        .saturating_mul(8_000)
        .saturating_add((recommended_workers as u64).saturating_mul(1_500))
        .clamp(20_000, 200_000) as usize;

    RecommendedSizing {
        recommended_workers,
        max_blocking_threads,
        queue_capacity,
    }
}

pub fn recommend_embedding_lane_sizing(profile: &RuntimeProfile) -> EmbeddingLaneSizing {
    let query_workers = 1usize;
    let available_background_workers = profile.recommended_workers.saturating_sub(query_workers);

    let (mut vector_workers, mut graph_workers) = if profile.gpu_present {
        let gpu_vector_workers = if available_background_workers >= 8 {
            3usize
        } else if available_background_workers >= 4 {
            2usize
        } else {
            1usize
        };
        (
            gpu_vector_workers,
            available_background_workers
                .saturating_sub(gpu_vector_workers)
                .max(1),
        )
    } else {
        let vw = (available_background_workers / 2).max(1);
        (vw, available_background_workers.saturating_sub(vw).max(1))
    };

    if available_background_workers <= 2 {
        vector_workers = 1;
        graph_workers = 1;
    } else if vector_workers + graph_workers > available_background_workers {
        vector_workers = available_background_workers.max(1);
        graph_workers = 1;
    }

    if !graph_embeddings_enabled() {
        graph_workers = 0;
    }

    let (chunk_batch_size, file_vectorization_batch_size, graph_batch_size) = if profile.gpu_present
    {
        // Keep GPU vector fan-out bounded by default so model residency stays safe,
        // while still allowing enough concurrency to drain real backlogs.
        (96, 24, 8)
    } else {
        // CPU-only hosts should stay conservative by default.
        // Runtime evidence showed that widening the background pool here
        // increases contention and hurts MCP latency more than it helps drain.
        (16, 8, 4)
    };

    EmbeddingLaneSizing {
        query_workers,
        vector_workers: vector_workers.max(1),
        graph_workers,
        chunk_batch_size,
        file_vectorization_batch_size,
        graph_batch_size,
    }
}

pub fn canonical_watcher_first_priority_lanes() -> [PipelinePriorityLane; 3] {
    [
        PipelinePriorityLane {
            lane: "watcher_identification",
            priority: "highest",
            admission_requires: &[],
        },
        PipelinePriorityLane {
            lane: "graphing_after_enqueue",
            priority: "second",
            admission_requires: &["persisted_file"],
        },
        PipelinePriorityLane {
            lane: "vectorization_after_graph_ready",
            priority: "third",
            admission_requires: &["graph_ready"],
        },
    ]
}

pub fn current_runtime_priority_contract_state(
    runtime_mode: &str,
    ingress_buffered_entries: usize,
    structural_graph_backlog_depth: usize,
) -> RuntimePriorityContractState {
    let mode = crate::runtime_mode::AxonRuntimeMode::from_str(runtime_mode);
    let processing_disabled = !mode.ingestion_enabled();
    let graph_backlog_present = structural_graph_backlog_depth > 0;

    RuntimePriorityContractState {
        watcher_identification_backlog_gated: processing_disabled,
        graphing_after_enqueue_backlog_gated: !processing_disabled && ingress_buffered_entries > 0,
        vectorization_after_graph_ready_backlog_gated: !processing_disabled
            && ingress_buffered_entries > 0,
        vectorization_allowed_ahead_of_graph_backlog: !processing_disabled,
        graph_backlog_present,
        enforcement_state: if processing_disabled {
            "runtime_processing_disabled"
        } else {
            "declared_runtime_truth_scheduler_follow_through_pending"
        },
    }
}

pub fn recommend_admission_controller_profile(
    profile: &RuntimeProfile,
) -> AdmissionControllerProfile {
    let target_band = profile
        .recommended_workers
        .saturating_mul(64)
        .clamp(256, 2_048);
    let reorder_point = (target_band.saturating_mul(3) / 4).clamp(192, 1_536);
    let max_wip = target_band
        .saturating_mul(8)
        .clamp(target_band.saturating_add(256), 16_384);
    let forced_bulk_fill_threshold = (reorder_point / 2).max(256);

    AdmissionControllerProfile {
        target_band,
        reorder_point,
        max_wip,
        hold_window_ms: 500,
        forced_bulk_fill_threshold,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn current_admission_controller_state(
    profile: AdmissionControllerProfile,
    buffered_discovery: usize,
    _watcher_buffered: usize,
    scan_buffered: usize,
    persisted_file_pending: usize,
    graph_wip: usize,
    runtime_processing_disabled: bool,
    critical_pressure: bool,
) -> AdmissionControllerState {
    let admission_stock_current = persisted_file_pending.saturating_add(graph_wip);
    let blocking_authority = if runtime_processing_disabled {
        "runtime_processing_disabled"
    } else if critical_pressure {
        "service_pressure_critical"
    } else if buffered_discovery == 0 {
        "no_buffered_discovery"
    } else if persisted_file_pending >= profile.max_wip {
        "persisted_file_pending_wip_cap_reached"
    } else if admission_stock_current >= profile.max_wip {
        "admission_stock_wip_cap_reached"
    } else {
        "none"
    };
    let admission_open = blocking_authority == "none";
    let bulk_fill_preferred = admission_open
        && scan_buffered >= profile.forced_bulk_fill_threshold
        && admission_stock_current < profile.target_band;

    AdmissionControllerState {
        profile,
        blocking_authority,
        admission_open,
        bulk_fill_preferred,
    }
}

pub fn current_graph_production_state(
    persisted_file_pending: usize,
    graph_wip: usize,
    graph_wip_cap: usize,
    runtime_processing_disabled: bool,
    critical_pressure: bool,
) -> GraphProductionState {
    let blocking_authority = if runtime_processing_disabled {
        "runtime_processing_disabled"
    } else if critical_pressure {
        "service_pressure_critical"
    } else if persisted_file_pending == 0 {
        "no_persisted_file_pending"
    } else if graph_wip >= graph_wip_cap.max(1) {
        "graph_wip_cap_reached"
    } else {
        "none"
    };

    GraphProductionState {
        blocking_authority,
        graph_open: blocking_authority == "none",
    }
}

pub fn current_vector_downstream_state(
    graph_ready: usize,
    _structural_graph_backlog_depth: usize,
    semantic_runtime_enabled: bool,
    critical_pressure: bool,
) -> VectorDownstreamState {
    let blocking_authority = if !semantic_runtime_enabled {
        "semantic_runtime_disabled"
    } else if graph_ready == 0 {
        "no_graph_ready_stock"
    } else if critical_pressure {
        "service_pressure_critical"
    } else {
        "none"
    };

    VectorDownstreamState {
        blocking_authority,
        vector_open: blocking_authority == "none",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_watcher_first_priority_lanes, configured_max_worker_cap,
        current_admission_controller_state, current_graph_production_state,
        current_runtime_priority_contract_state, current_vector_downstream_state,
        detect_ingestion_memory_budget_gb, detect_ram_budget_gb,
        recommend_admission_controller_profile, recommend_embedding_lane_sizing, recommend_sizing,
        wsl_cuda_runtime_available, RuntimeProfile,
    };

    #[test]
    fn test_recommend_sizing_scales_down_on_low_memory() {
        let sizing = recommend_sizing(16, 8, false);
        assert!(sizing.recommended_workers <= 6);
        assert!(sizing.queue_capacity >= 20_000);
    }

    #[test]
    fn test_recommend_sizing_supports_larger_hosts() {
        let sizing = recommend_sizing(32, 64, false);
        assert!(sizing.recommended_workers >= 16);
        assert_eq!(sizing.queue_capacity, 200_000);
    }

    #[test]
    fn test_recommend_sizing_varies_between_nearby_memory_sizes() {
        let lower = recommend_sizing(12, 20, false);
        let higher = recommend_sizing(12, 24, false);

        assert!(
            higher.queue_capacity > lower.queue_capacity,
            "queue capacity should scale continuously instead of staying flat across a wide RAM tier"
        );
    }

    #[test]
    fn test_ram_budget_keeps_headroom() {
        assert_eq!(detect_ram_budget_gb(32), 24);
        assert_eq!(detect_ram_budget_gb(16), 12);
        assert_eq!(detect_ram_budget_gb(8), 6);
        assert_eq!(detect_ram_budget_gb(20), 15);
    }

    #[test]
    fn test_ingestion_budget_keeps_only_a_fraction_of_axon_budget() {
        assert_eq!(detect_ingestion_memory_budget_gb(24), 9);
        assert_eq!(detect_ingestion_memory_budget_gb(12), 5);
        assert_eq!(detect_ingestion_memory_budget_gb(6), 3);
    }

    #[test]
    fn test_wsl_cuda_runtime_available_detects_wsl_gpu_shape() {
        assert!(wsl_cuda_runtime_available(
            "6.6.87.2-microsoft-standard-WSL2",
            true
        ));
        assert!(!wsl_cuda_runtime_available("6.6.87.2-linux", true));
        assert!(!wsl_cuda_runtime_available(
            "6.6.87.2-microsoft-standard-WSL2",
            false
        ));
    }

    #[test]
    fn test_max_worker_cap_reads_positive_integer() {
        std::env::set_var("MAX_AXON_WORKERS", "1");
        assert_eq!(configured_max_worker_cap(), Some(1));
        std::env::remove_var("MAX_AXON_WORKERS");
    }

    #[test]
    fn test_embedding_lane_sizing_disables_graph_lane_on_large_cpu_hosts_by_default() {
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
        let profile = RuntimeProfile {
            cpu_cores: 16,
            ram_total_gb: 31,
            ram_budget_gb: 23,
            ingestion_memory_budget_gb: 9,
            gpu_present: false,
            recommended_workers: 14,
            max_blocking_threads: 11,
            queue_capacity: 200_000,
        };
        let sizing = recommend_embedding_lane_sizing(&profile);
        assert_eq!(sizing.query_workers, 1);
        assert_eq!(sizing.vector_workers, 6);
        assert_eq!(sizing.graph_workers, 7);
        assert_eq!(sizing.chunk_batch_size, 16);
        assert_eq!(sizing.file_vectorization_batch_size, 8);
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
    }

    #[test]
    fn test_embedding_lane_sizing_expands_when_gpu_is_available() {
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
        let profile = RuntimeProfile {
            cpu_cores: 16,
            ram_total_gb: 31,
            ram_budget_gb: 23,
            ingestion_memory_budget_gb: 9,
            gpu_present: true,
            recommended_workers: 14,
            max_blocking_threads: 11,
            queue_capacity: 200_000,
        };
        let sizing = recommend_embedding_lane_sizing(&profile);
        assert_eq!(sizing.query_workers, 1);
        assert_eq!(sizing.vector_workers, 3);
        assert_eq!(sizing.graph_workers, 10);
        assert_eq!(sizing.chunk_batch_size, 96);
        assert_eq!(sizing.file_vectorization_batch_size, 24);
        assert_eq!(sizing.graph_batch_size, 8);
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
    }

    #[test]
    fn test_embedding_lane_sizing_prefers_batch_depth_over_gpu_worker_fanout() {
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
        let profile = RuntimeProfile {
            cpu_cores: 24,
            ram_total_gb: 64,
            ram_budget_gb: 48,
            ingestion_memory_budget_gb: 17,
            gpu_present: true,
            recommended_workers: 20,
            max_blocking_threads: 15,
            queue_capacity: 200_000,
        };
        let sizing = recommend_embedding_lane_sizing(&profile);
        assert_eq!(sizing.query_workers, 1);
        assert_eq!(sizing.vector_workers, 3);
        assert_eq!(sizing.graph_workers, 16);
        assert_eq!(sizing.chunk_batch_size, 96);
        assert_eq!(sizing.file_vectorization_batch_size, 24);
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
    }

    #[test]
    fn test_embedding_lane_sizing_stays_small_on_constrained_hosts() {
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
        let profile = RuntimeProfile {
            cpu_cores: 4,
            ram_total_gb: 8,
            ram_budget_gb: 6,
            ingestion_memory_budget_gb: 3,
            gpu_present: false,
            recommended_workers: 2,
            max_blocking_threads: 4,
            queue_capacity: 20_000,
        };
        let sizing = recommend_embedding_lane_sizing(&profile);
        assert_eq!(sizing.query_workers, 1);
        assert_eq!(sizing.vector_workers, 1);
        assert_eq!(sizing.graph_workers, 1);
        assert_eq!(sizing.chunk_batch_size, 16);
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
    }

    #[test]
    fn test_embedding_lane_sizing_disables_graph_lane_when_canonical_flag_is_off() {
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "false");
        let profile = RuntimeProfile {
            cpu_cores: 16,
            ram_total_gb: 31,
            ram_budget_gb: 23,
            ingestion_memory_budget_gb: 9,
            gpu_present: true,
            recommended_workers: 14,
            max_blocking_threads: 11,
            queue_capacity: 200_000,
        };
        let sizing = recommend_embedding_lane_sizing(&profile);
        assert_eq!(sizing.graph_workers, 0);
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
    }

    #[test]
    fn test_canonical_watcher_first_priority_lanes_are_ordered() {
        let lanes = canonical_watcher_first_priority_lanes();
        assert_eq!(lanes[0].lane, "watcher_identification");
        assert_eq!(lanes[0].priority, "highest");
        assert!(lanes[0].admission_requires.is_empty());
        assert_eq!(lanes[1].lane, "graphing_after_enqueue");
        assert_eq!(lanes[1].priority, "second");
        assert_eq!(lanes[1].admission_requires, &["persisted_file"]);
        assert_eq!(lanes[2].lane, "vectorization_after_graph_ready");
        assert_eq!(lanes[2].priority, "third");
        assert_eq!(lanes[2].admission_requires, &["graph_ready"]);
    }

    #[test]
    fn test_current_runtime_priority_contract_state_defaults_to_no_graph_overtake() {
        let state = current_runtime_priority_contract_state("indexer_full", 0, 1);
        assert!(!state.watcher_identification_backlog_gated);
        assert!(!state.graphing_after_enqueue_backlog_gated);
        assert!(!state.vectorization_after_graph_ready_backlog_gated);
        assert!(state.vectorization_allowed_ahead_of_graph_backlog);
        assert!(state.graph_backlog_present);
    }

    #[test]
    fn test_admission_controller_prefers_bulk_fill_when_pending_stock_is_thin() {
        let runtime = RuntimeProfile {
            cpu_cores: 8,
            ram_total_gb: 32,
            ram_budget_gb: 24,
            ingestion_memory_budget_gb: 8,
            gpu_present: true,
            recommended_workers: 6,
            max_blocking_threads: 8,
            queue_capacity: 50_000,
        };
        let profile = recommend_admission_controller_profile(&runtime);
        let state =
            current_admission_controller_state(profile, 1_200, 8, 1_192, 32, 4, false, false);
        assert!(state.admission_open);
        assert!(state.bulk_fill_preferred);
        assert_eq!(state.blocking_authority, "none");
    }

    #[test]
    fn test_admission_controller_reports_pending_wip_cap() {
        let runtime = RuntimeProfile {
            cpu_cores: 8,
            ram_total_gb: 32,
            ram_budget_gb: 24,
            ingestion_memory_budget_gb: 8,
            gpu_present: true,
            recommended_workers: 6,
            max_blocking_threads: 8,
            queue_capacity: 50_000,
        };
        let profile = recommend_admission_controller_profile(&runtime);
        let state = current_admission_controller_state(
            profile,
            300,
            2,
            298,
            profile.max_wip,
            0,
            false,
            false,
        );
        assert!(!state.admission_open);
        assert_eq!(
            state.blocking_authority,
            "persisted_file_pending_wip_cap_reached"
        );
    }

    #[test]
    fn test_admission_controller_allows_graph_wip_to_climb_until_total_stock_cap() {
        let runtime = RuntimeProfile {
            cpu_cores: 8,
            ram_total_gb: 32,
            ram_budget_gb: 24,
            ingestion_memory_budget_gb: 8,
            gpu_present: true,
            recommended_workers: 6,
            max_blocking_threads: 8,
            queue_capacity: 50_000,
        };
        let profile = recommend_admission_controller_profile(&runtime);
        let state = current_admission_controller_state(
            profile,
            1_200,
            0,
            1_192,
            profile.reorder_point,
            profile.max_wip / 2,
            false,
            false,
        );
        assert!(state.admission_open);
        assert_eq!(state.blocking_authority, "none");
    }

    #[test]
    fn test_admission_controller_caps_total_admission_stock_at_budget() {
        let runtime = RuntimeProfile {
            cpu_cores: 8,
            ram_total_gb: 32,
            ram_budget_gb: 24,
            ingestion_memory_budget_gb: 8,
            gpu_present: true,
            recommended_workers: 6,
            max_blocking_threads: 8,
            queue_capacity: 50_000,
        };
        let profile = recommend_admission_controller_profile(&runtime);
        let state = current_admission_controller_state(
            profile,
            1_200,
            0,
            1_192,
            profile.reorder_point,
            profile.max_wip.saturating_sub(profile.reorder_point),
            false,
            false,
        );
        assert!(!state.admission_open);
        assert_eq!(state.blocking_authority, "admission_stock_wip_cap_reached");
    }

    #[test]
    fn test_admission_controller_bulk_fill_is_not_blocked_by_watcher_noise() {
        let runtime = RuntimeProfile {
            cpu_cores: 8,
            ram_total_gb: 32,
            ram_budget_gb: 24,
            ingestion_memory_budget_gb: 8,
            gpu_present: true,
            recommended_workers: 6,
            max_blocking_threads: 8,
            queue_capacity: 50_000,
        };
        let profile = recommend_admission_controller_profile(&runtime);
        let state = current_admission_controller_state(
            profile,
            1_200,
            profile.reorder_point.saturating_mul(4),
            1_192,
            32,
            4,
            false,
            false,
        );
        assert!(state.admission_open);
        assert!(state.bulk_fill_preferred);
        assert_eq!(state.blocking_authority, "none");
    }

    #[test]
    fn test_graph_production_state_reports_pending_and_wip_cap() {
        let pending_empty = current_graph_production_state(0, 0, 32, false, false);
        assert_eq!(
            pending_empty.blocking_authority,
            "no_persisted_file_pending"
        );
        assert!(!pending_empty.graph_open);

        let capped = current_graph_production_state(64, 32, 32, false, false);
        assert_eq!(capped.blocking_authority, "graph_wip_cap_reached");
        assert!(!capped.graph_open);

        let open = current_graph_production_state(64, 8, 32, false, false);
        assert_eq!(open.blocking_authority, "none");
        assert!(open.graph_open);
    }

    #[test]
    fn test_vector_downstream_state_stays_open_without_runtime_pressure() {
        let disabled = current_vector_downstream_state(32, 0, false, false);
        assert_eq!(disabled.blocking_authority, "semantic_runtime_disabled");
        assert!(!disabled.vector_open);

        let reserved = current_vector_downstream_state(32, 12, true, false);
        assert_eq!(reserved.blocking_authority, "none");
        assert!(reserved.vector_open);

        let open = current_vector_downstream_state(32, 0, true, false);
        assert_eq!(open.blocking_authority, "none");
        assert!(open.vector_open);
    }
}
