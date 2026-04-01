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

        Self {
            cpu_cores,
            ram_total_gb,
            ram_budget_gb,
            ingestion_memory_budget_gb,
            gpu_present,
            recommended_workers: sizing.recommended_workers,
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

fn detect_gpu_presence() -> bool {
    Path::new("/dev/dri/renderD128").exists()
        || Path::new("/dev/nvidia0").exists()
        || Path::new("/proc/driver/nvidia/version").exists()
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

#[cfg(test)]
mod tests {
    use super::{detect_ingestion_memory_budget_gb, detect_ram_budget_gb, recommend_sizing};

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
}
