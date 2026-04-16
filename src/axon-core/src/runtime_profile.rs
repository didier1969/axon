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
        (
            1usize,
            available_background_workers.saturating_sub(1).max(1),
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
        // Keep a single GPU vector worker to avoid duplicate model residency,
        // but feed that worker more aggressively now that query embeddings live on CPU
        // and the CUDA controller can clamp under live memory pressure.
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

#[cfg(test)]
mod tests {
    use super::{
        configured_max_worker_cap, detect_ingestion_memory_budget_gb, detect_ram_budget_gb,
        recommend_embedding_lane_sizing, recommend_sizing, wsl_cuda_runtime_available,
        RuntimeProfile,
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
        assert_eq!(sizing.vector_workers, 1);
        assert_eq!(sizing.graph_workers, 6);
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
        assert_eq!(sizing.vector_workers, 1);
        assert_eq!(sizing.graph_workers, 12);
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
        assert_eq!(sizing.vector_workers, 1);
        assert_eq!(sizing.graph_workers, 18);
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
}
