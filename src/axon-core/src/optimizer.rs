use crate::embedder::{
    current_embedding_provider_diagnostics, current_gpu_memory_snapshot,
    embedding_lane_config_from_env,
};
use crate::embedding_contract::CHUNK_MODEL_ID;
use crate::graph::GraphStore;
use crate::runtime_observability::{
    duckdb_memory_snapshot, process_memory_snapshot, DuckDbMemorySnapshot, ProcessMemorySnapshot,
};
use crate::runtime_profile::RuntimeProfile;
use crate::service_guard;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostSnapshot {
    pub captured_at_ms: i64,
    pub source: String,
    pub platform: String,
    pub is_wsl: bool,
    pub cpu_cores: usize,
    pub ram_total_bytes: u64,
    pub gpu_present: bool,
    pub gpu_name: Option<String>,
    pub vram_total_mb: u64,
    pub io_characteristics: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeSignalsWindow {
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub captured_at_ms: i64,
    pub source: String,
    pub cpu_usage_ratio: f64,
    pub ram_available_ratio: f64,
    pub io_wait_ratio: f64,
    pub process_memory: ProcessMemorySnapshot,
    pub duckdb_memory: DuckDbMemorySnapshot,
    pub vram_used_mb: u64,
    pub vram_free_mb: u64,
    pub gpu_utilization_ratio: f64,
    pub gpu_memory_utilization_ratio: f64,
    pub file_vectorization_queue_depth: usize,
    pub graph_projection_queue_depth: usize,
    pub latency_recent_fetch_p95_ms: u64,
    pub latency_recent_embed_p95_ms: u64,
    pub latency_recent_db_write_p95_ms: u64,
    pub latency_recent_mark_done_p95_ms: u64,
    pub mcp_latency_recent_ms: u64,
    pub vector_workers_active_current: u64,
    pub vector_worker_heartbeat_at_ms: u64,
    pub embed_inflight_started_at_ms: u64,
    pub interactive_requests_in_flight: u64,
    pub interactive_priority: String,
    pub chunks_embedded_total: u64,
    pub files_completed_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatorPolicySnapshot {
    pub captured_at_ms: i64,
    pub max_cpu_ratio: f64,
    pub min_ram_available_ratio: f64,
    pub max_mcp_p95_ms: u64,
    pub max_vram_used_ratio: f64,
    pub max_vram_used_mb: u64,
    pub max_io_wait_ratio: f64,
    pub backlog_priority_weight: f64,
    pub interactive_priority_weight: f64,
    pub shadow_mode_enabled: bool,
    pub allowed_actuators: Vec<String>,
    pub evaluation_window_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RecentAnalyticsWindow {
    pub collected_at_ms: i64,
    pub current_hour_bucket_start_ms: i64,
    pub chunks_embedded_current_hour: u64,
    pub files_vector_ready_current_hour: u64,
    pub batches_current_hour: u64,
    pub embed_ms_total_current_hour: u64,
    pub db_write_ms_total_current_hour: u64,
    pub mark_done_ms_total_current_hour: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionProfile {
    pub id: String,
    pub label: String,
    pub target_vector_workers: usize,
    pub target_chunk_batch_size: usize,
    pub target_file_vectorization_batch_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptimizerDecision {
    pub decision_id: String,
    pub proposed_at_ms: i64,
    pub action_profile_id: String,
    pub decision_reason: String,
    pub score: f64,
    pub confidence: f64,
    pub evaluation_window_start_ms: i64,
    pub evaluation_window_end_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RewardObservation {
    pub decision_id: String,
    pub observed_at_ms: i64,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub throughput_chunks_per_hour: f64,
    pub throughput_files_per_hour: f64,
    pub reward: f64,
    pub penalty_cpu: f64,
    pub penalty_ram: f64,
    pub penalty_vram: f64,
    pub penalty_mcp: f64,
    pub penalty_io: f64,
    pub penalty_liveness: f64,
    pub penalty_churn: f64,
}

pub trait PolicyEngine {
    fn choose(
        &self,
        host: &HostSnapshot,
        signals: &RuntimeSignalsWindow,
        policy: &OperatorPolicySnapshot,
        analytics: &RecentAnalyticsWindow,
        action_profiles: &[ActionProfile],
    ) -> Option<OptimizerDecision>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicPolicyEngine;

impl PolicyEngine for HeuristicPolicyEngine {
    fn choose(
        &self,
        host: &HostSnapshot,
        signals: &RuntimeSignalsWindow,
        policy: &OperatorPolicySnapshot,
        analytics: &RecentAnalyticsWindow,
        action_profiles: &[ActionProfile],
    ) -> Option<OptimizerDecision> {
        let now_ms = now_ms();
        let eval_window_start_ms = now_ms;
        let eval_window_end_ms =
            now_ms.saturating_add(i64::try_from(policy.evaluation_window_ms).unwrap_or(i64::MAX));
        let mut best: Option<(ActionProfile, f64, String)> = None;

        for profile in action_profiles {
            let mut score = analytics.chunks_embedded_current_hour as f64
                * policy.backlog_priority_weight.max(0.1);
            let mut reasons = Vec::new();

            if profile.target_chunk_batch_size > signals.file_vectorization_queue_depth.max(1) {
                score -= 1.0;
                reasons.push("batch_gt_backlog");
            }
            if profile.target_vector_workers > 1 && !host.gpu_present {
                score -= 5.0;
                reasons.push("cpu_parallelism_risk");
            }
            if signals.cpu_usage_ratio > policy.max_cpu_ratio {
                score -= 10.0;
                reasons.push("cpu_guard_active");
            } else if profile.target_vector_workers > 1 {
                score += 0.5;
                reasons.push("cpu_headroom");
            }
            if signals.ram_available_ratio < policy.min_ram_available_ratio {
                score -= 10.0;
                reasons.push("ram_guard_active");
            }
            if signals.vram_used_mb > policy.max_vram_used_mb {
                score -= 12.0;
                reasons.push("vram_guard_active");
            } else if host.gpu_present && profile.target_chunk_batch_size > 0 {
                score += (profile.target_chunk_batch_size as f64) / 64.0;
                reasons.push("gpu_batch_depth");
            }
            if signals.io_wait_ratio > policy.max_io_wait_ratio {
                score -= 4.0;
                reasons.push("io_wait_guard_active");
            }
            if signals.mcp_latency_recent_ms > policy.max_mcp_p95_ms {
                score -= 8.0 * policy.interactive_priority_weight.max(0.1);
                reasons.push("mcp_guard_active");
            }
            if signals.interactive_requests_in_flight > 0
                || signals.interactive_priority != "background_normal"
            {
                score -= 2.0 * policy.interactive_priority_weight.max(0.1);
                reasons.push("interactive_pressure");
            }
            if signals.embed_inflight_started_at_ms > 0
                && signals.vector_workers_active_current > 0
                && signals.file_vectorization_queue_depth > 0
            {
                score -= 1.5;
                reasons.push("embed_inflight");
            }
            if profile.target_chunk_batch_size < 8 {
                score -= 1.0;
                reasons.push("overly_small_batch");
            }

            let reason = reasons.join(",");
            if best
                .as_ref()
                .map(|(_, best_score, _)| score > *best_score)
                .unwrap_or(true)
            {
                best = Some((profile.clone(), score, reason));
            }
        }

        let (profile, score, reason) = best?;
        Some(OptimizerDecision {
            decision_id: format!("opt-{}", now_ms),
            proposed_at_ms: now_ms,
            action_profile_id: profile.id,
            decision_reason: reason,
            score,
            confidence: if action_profiles.len() <= 1 { 1.0 } else { 0.6 },
            evaluation_window_start_ms: eval_window_start_ms,
            evaluation_window_end_ms: eval_window_end_ms,
        })
    }
}

pub fn collect_host_snapshot() -> HostSnapshot {
    let profile = RuntimeProfile::detect();
    let gpu = current_gpu_memory_snapshot().unwrap_or(crate::embedder::GpuMemorySnapshot {
        total_mb: 0,
        used_mb: 0,
        free_mb: 0,
    });
    let provider = current_embedding_provider_diagnostics();
    HostSnapshot {
        captured_at_ms: now_ms(),
        source: "optimizer.host.detect".to_string(),
        platform: std::env::consts::OS.to_string(),
        is_wsl: detect_is_wsl(),
        cpu_cores: profile.cpu_cores,
        ram_total_bytes: profile.ram_total_gb.saturating_mul(1024 * 1024 * 1024),
        gpu_present: profile.gpu_present,
        gpu_name: if provider.provider_effective.eq_ignore_ascii_case("cuda") || gpu.total_mb > 0 {
            Some("nvidia".to_string())
        } else {
            None
        },
        vram_total_mb: gpu.total_mb,
        io_characteristics: "linux_procfs_sample".to_string(),
    }
}

pub fn collect_operator_policy_snapshot(host: &HostSnapshot) -> OperatorPolicySnapshot {
    let captured_at_ms = now_ms();
    let max_vram_used_mb = env_u64("AXON_OPT_MAX_VRAM_USED_MB", host.vram_total_mb)
        .min(host.vram_total_mb.max(1));
    let max_vram_used_ratio = if host.vram_total_mb > 0 {
        (max_vram_used_mb as f64 / host.vram_total_mb as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    OperatorPolicySnapshot {
        captured_at_ms,
        max_cpu_ratio: env_f64("AXON_OPT_MAX_CPU_RATIO", 0.50).clamp(0.0, 1.0),
        min_ram_available_ratio: env_f64("AXON_OPT_MIN_RAM_AVAILABLE_RATIO", 0.33)
            .clamp(0.0, 1.0),
        max_mcp_p95_ms: env_u64("AXON_OPT_MAX_MCP_P95_MS", 300),
        max_vram_used_ratio,
        max_vram_used_mb,
        max_io_wait_ratio: env_f64("AXON_OPT_MAX_IO_WAIT_RATIO", 0.20).clamp(0.0, 1.0),
        backlog_priority_weight: env_f64("AXON_OPT_BACKLOG_PRIORITY_WEIGHT", 1.0).max(0.0),
        interactive_priority_weight: env_f64("AXON_OPT_INTERACTIVE_PRIORITY_WEIGHT", 1.0)
            .max(0.0),
        shadow_mode_enabled: env_bool("AXON_OPT_SHADOW_MODE_ENABLED", true),
        allowed_actuators: vec![
            "vector_workers".to_string(),
            "chunk_batch_size".to_string(),
            "file_vectorization_batch_size".to_string(),
        ],
        evaluation_window_ms: env_u64("AXON_OPT_EVALUATION_WINDOW_MS", 60_000).max(10_000),
    }
}

pub fn collect_runtime_signals_window(store: &GraphStore) -> RuntimeSignalsWindow {
    let now_ms = now_ms();
    let memory = process_memory_snapshot();
    let duckdb_memory = duckdb_memory_snapshot(store);
    let gpu = current_gpu_memory_snapshot().unwrap_or(crate::embedder::GpuMemorySnapshot {
        total_mb: 0,
        used_mb: 0,
        free_mb: 0,
    });
    let vector_latency = service_guard::vector_runtime_latency_summaries();
    let vector_runtime = service_guard::vector_runtime_metrics();
    let (file_vectorization_queue_queued, file_vectorization_queue_inflight) = store
        .fetch_file_vectorization_queue_counts()
        .unwrap_or((0, 0));
    let (graph_projection_queue_queued, graph_projection_queue_inflight) = store
        .fetch_graph_projection_queue_counts()
        .unwrap_or((0, 0));
    let (cpu_usage_ratio, ram_available_ratio, io_wait_ratio) = read_host_pressure_ratios();
    RuntimeSignalsWindow {
        window_start_ms: now_ms.saturating_sub(60_000),
        window_end_ms: now_ms,
        captured_at_ms: now_ms,
        source: "optimizer.runtime.window".to_string(),
        cpu_usage_ratio,
        ram_available_ratio,
        io_wait_ratio,
        process_memory: memory,
        duckdb_memory,
        vram_used_mb: gpu.used_mb,
        vram_free_mb: gpu.free_mb,
        gpu_utilization_ratio: 0.0,
        gpu_memory_utilization_ratio: 0.0,
        file_vectorization_queue_depth: file_vectorization_queue_queued + file_vectorization_queue_inflight,
        graph_projection_queue_depth: graph_projection_queue_queued + graph_projection_queue_inflight,
        latency_recent_fetch_p95_ms: vector_latency.fetch.p95_ms,
        latency_recent_embed_p95_ms: vector_latency.embed.p95_ms,
        latency_recent_db_write_p95_ms: vector_latency.db_write.p95_ms,
        latency_recent_mark_done_p95_ms: vector_latency.mark_done.p95_ms,
        mcp_latency_recent_ms: service_guard::recent_peak_latency_ms(),
        vector_workers_active_current: vector_runtime.vector_workers_active_current,
        vector_worker_heartbeat_at_ms: vector_runtime.vector_worker_heartbeat_at_ms,
        embed_inflight_started_at_ms: vector_runtime.embed_inflight_started_at_ms,
        interactive_requests_in_flight: service_guard::interactive_requests_in_flight(),
        interactive_priority: service_guard::current_interactive_priority().as_str().to_string(),
        chunks_embedded_total: vector_runtime.chunks_embedded_total,
        files_completed_total: vector_runtime.files_completed_total,
    }
}

pub fn collect_recent_analytics_window(store: &GraphStore) -> RecentAnalyticsWindow {
    let now_ms = now_ms();
    let bucket_start_ms = (now_ms / 3_600_000) * 3_600_000;
    let _ = store.refresh_hourly_vectorization_rollup(bucket_start_ms, CHUNK_MODEL_ID);
    let query = format!(
        "SELECT COALESCE(sum(chunks_embedded), 0), \
                COALESCE(sum(files_vector_ready), 0), \
                COALESCE(sum(batches), 0), \
                COALESCE(sum(embed_ms_total), 0), \
                COALESCE(sum(db_write_ms_total), 0), \
                COALESCE(sum(mark_done_ms_total), 0) \
         FROM HourlyVectorizationRollup \
         WHERE bucket_start_ms = {} \
           AND model_id = '{}'",
        bucket_start_ms,
        CHUNK_MODEL_ID.replace('\'', "''")
    );
    let raw = store.query_json_writer(&query).unwrap_or_else(|_| "[]".to_string());
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let row = rows.first().cloned().unwrap_or_default();
    RecentAnalyticsWindow {
        collected_at_ms: now_ms,
        current_hour_bucket_start_ms: bucket_start_ms,
        chunks_embedded_current_hour: value_u64(row.first()),
        files_vector_ready_current_hour: value_u64(row.get(1)),
        batches_current_hour: value_u64(row.get(2)),
        embed_ms_total_current_hour: value_u64(row.get(3)),
        db_write_ms_total_current_hour: value_u64(row.get(4)),
        mark_done_ms_total_current_hour: value_u64(row.get(5)),
    }
}

pub fn build_admissible_action_profiles(
    host: &HostSnapshot,
    signals: &RuntimeSignalsWindow,
    policy: &OperatorPolicySnapshot,
) -> Vec<ActionProfile> {
    let current = embedding_lane_config_from_env();
    let mut profiles = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut push = |label: &str, vector_workers: usize, chunk_batch_size: usize, file_batch: usize| {
        let tuple = (
            vector_workers.max(1),
            chunk_batch_size.max(1),
            file_batch.max(1),
        );
        if !seen.insert(tuple) {
            return;
        }
        profiles.push(ActionProfile {
            id: format!("vw{}-cb{}-fb{}", tuple.0, tuple.1, tuple.2),
            label: label.to_string(),
            target_vector_workers: tuple.0,
            target_chunk_batch_size: tuple.1,
            target_file_vectorization_batch_size: tuple.2,
        });
    };

    push(
        "hold",
        current.vector_workers,
        current.chunk_batch_size,
        current.file_vectorization_batch_size,
    );

    if signals.cpu_usage_ratio <= policy.max_cpu_ratio
        && signals.ram_available_ratio >= policy.min_ram_available_ratio
    {
        push(
            "deepen_batch",
            current.vector_workers,
            current.chunk_batch_size.saturating_add(8),
            current.file_vectorization_batch_size.saturating_add(2),
        );
    }
    if current.chunk_batch_size > 8 {
        push(
            "shrink_batch",
            current.vector_workers,
            current.chunk_batch_size.saturating_sub(8).max(8),
            current.file_vectorization_batch_size.saturating_sub(2).max(1),
        );
    }
    if host.gpu_present
        && current.vector_workers < 2
        && signals.vram_used_mb < policy.max_vram_used_mb
        && signals.cpu_usage_ratio <= policy.max_cpu_ratio * 0.9
    {
        push(
            "raise_workers",
            current.vector_workers + 1,
            current.chunk_batch_size,
            current.file_vectorization_batch_size,
        );
    }
    if current.vector_workers > 1 {
        push(
            "lower_workers",
            current.vector_workers - 1,
            current.chunk_batch_size,
            current.file_vectorization_batch_size,
        );
    }

    profiles
}

pub fn observe_reward(
    decision_id: &str,
    previous: &RuntimeSignalsWindow,
    current: &RuntimeSignalsWindow,
    policy: &OperatorPolicySnapshot,
    churn_penalty: f64,
) -> RewardObservation {
    let elapsed_hours = ((current.window_end_ms - previous.window_end_ms).max(1) as f64)
        / 3_600_000.0;
    let chunk_delta = current
        .chunks_embedded_total
        .saturating_sub(previous.chunks_embedded_total) as f64;
    let file_delta = current
        .files_completed_total
        .saturating_sub(previous.files_completed_total) as f64;
    let throughput_chunks_per_hour = chunk_delta / elapsed_hours;
    let throughput_files_per_hour = file_delta / elapsed_hours;

    let penalty_cpu = if current.cpu_usage_ratio > policy.max_cpu_ratio {
        (current.cpu_usage_ratio - policy.max_cpu_ratio) * 100.0
    } else {
        0.0
    };
    let penalty_ram = if current.ram_available_ratio < policy.min_ram_available_ratio {
        (policy.min_ram_available_ratio - current.ram_available_ratio) * 100.0
    } else {
        0.0
    };
    let penalty_vram = if current.vram_used_mb > policy.max_vram_used_mb {
        (current.vram_used_mb - policy.max_vram_used_mb) as f64 / 32.0
    } else {
        0.0
    };
    let penalty_mcp = if current.mcp_latency_recent_ms > policy.max_mcp_p95_ms {
        (current.mcp_latency_recent_ms - policy.max_mcp_p95_ms) as f64 / 10.0
    } else {
        0.0
    };
    let penalty_io = if current.io_wait_ratio > policy.max_io_wait_ratio {
        (current.io_wait_ratio - policy.max_io_wait_ratio) * 100.0
    } else {
        0.0
    };
    let penalty_liveness = if current.vector_workers_active_current == 0 {
        25.0
    } else {
        0.0
    };
    let reward = throughput_chunks_per_hour
        - penalty_cpu
        - penalty_ram
        - penalty_vram
        - penalty_mcp
        - penalty_io
        - penalty_liveness
        - churn_penalty;

    RewardObservation {
        decision_id: decision_id.to_string(),
        observed_at_ms: now_ms(),
        window_start_ms: previous.window_end_ms,
        window_end_ms: current.window_end_ms,
        throughput_chunks_per_hour,
        throughput_files_per_hour,
        reward,
        penalty_cpu,
        penalty_ram,
        penalty_vram,
        penalty_mcp,
        penalty_io,
        penalty_liveness,
        penalty_churn: churn_penalty,
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|raw| matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn detect_is_wsl() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|raw| raw.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

fn value_u64(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(|value| value.as_u64())
        .or_else(|| value.and_then(|value| value.as_i64()).map(|v| v.max(0) as u64))
        .or_else(|| value.and_then(|value| value.as_str()).and_then(|v| v.parse::<u64>().ok()))
        .unwrap_or(0)
}

fn read_host_pressure_ratios() -> (f64, f64, f64) {
    let cpu_usage_ratio = read_cpu_usage_ratio().clamp(0.0, 1.0);
    let ram_available_ratio = read_ram_available_ratio().clamp(0.0, 1.0);
    let io_wait_ratio = read_io_wait_ratio().clamp(0.0, 1.0);
    (cpu_usage_ratio, ram_available_ratio, io_wait_ratio)
}

fn read_cpu_usage_ratio() -> f64 {
    let process = std::fs::read_to_string("/proc/self/stat").ok();
    let uptime = std::fs::read_to_string("/proc/uptime").ok();
    let Some(process) = process else {
        return 0.0;
    };
    let Some(uptime) = uptime else {
        return 0.0;
    };
    let fields = process.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 22 {
        return 0.0;
    }
    let utime = fields.get(13).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let stime = fields.get(14).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let starttime = fields.get(21).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let uptime_secs = uptime
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    let hertz = 100.0;
    let total_time = (utime + stime) as f64 / hertz;
    let seconds = (uptime_secs - (starttime as f64 / hertz)).max(1.0);
    (total_time / seconds).clamp(0.0, 1.0)
}

fn read_ram_available_ratio() -> f64 {
    let content = match std::fs::read_to_string("/proc/meminfo") {
        Ok(content) => content,
        Err(_) => return 0.0,
    };
    let mut total_kb = 0.0;
    let mut available_kb = 0.0;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        match parts.next().unwrap_or_default() {
            "MemTotal:" => {
                total_kb = parts
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            }
            "MemAvailable:" => {
                available_kb = parts
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            }
            _ => {}
        }
    }
    if total_kb <= 0.0 {
        0.0
    } else {
        available_kb / total_kb
    }
}

fn read_io_wait_ratio() -> f64 {
    let content = match std::fs::read_to_string("/proc/stat") {
        Ok(content) => content,
        Err(_) => return 0.0,
    };
    let Some(line) = content.lines().find(|line| line.starts_with("cpu ")) else {
        return 0.0;
    };
    let values = line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if values.len() < 5 {
        return 0.0;
    }
    let total = values.iter().sum::<u64>() as f64;
    if total <= 0.0 {
        0.0
    } else {
        values[4] as f64 / total
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_admissible_action_profiles, collect_operator_policy_snapshot, observe_reward,
        ActionProfile, HeuristicPolicyEngine, HostSnapshot, OperatorPolicySnapshot, PolicyEngine,
        RecentAnalyticsWindow, RuntimeSignalsWindow,
    };
    use crate::service_guard::InteractivePriority;

    fn host() -> HostSnapshot {
        HostSnapshot {
            captured_at_ms: 1,
            source: "test".to_string(),
            platform: "linux".to_string(),
            is_wsl: true,
            cpu_cores: 8,
            ram_total_bytes: 32 * 1024 * 1024 * 1024,
            gpu_present: true,
            gpu_name: Some("rtx".to_string()),
            vram_total_mb: 8192,
            io_characteristics: "test".to_string(),
        }
    }

    fn signals() -> RuntimeSignalsWindow {
        RuntimeSignalsWindow {
            window_start_ms: 0,
            window_end_ms: 60_000,
            captured_at_ms: 60_000,
            source: "test".to_string(),
            cpu_usage_ratio: 0.2,
            ram_available_ratio: 0.5,
            io_wait_ratio: 0.01,
            process_memory: Default::default(),
            duckdb_memory: Default::default(),
            vram_used_mb: 1024,
            vram_free_mb: 7168,
            gpu_utilization_ratio: 0.2,
            gpu_memory_utilization_ratio: 0.1,
            file_vectorization_queue_depth: 128,
            graph_projection_queue_depth: 4,
            latency_recent_fetch_p95_ms: 10,
            latency_recent_embed_p95_ms: 25,
            latency_recent_db_write_p95_ms: 5,
            latency_recent_mark_done_p95_ms: 5,
            mcp_latency_recent_ms: 40,
            vector_workers_active_current: 1,
            vector_worker_heartbeat_at_ms: 59_000,
            embed_inflight_started_at_ms: 0,
            interactive_requests_in_flight: 0,
            interactive_priority: InteractivePriority::BackgroundNormal.as_str().to_string(),
            chunks_embedded_total: 100,
            files_completed_total: 10,
        }
    }

    fn policy() -> OperatorPolicySnapshot {
        OperatorPolicySnapshot {
            captured_at_ms: 1,
            max_cpu_ratio: 0.5,
            min_ram_available_ratio: 0.33,
            max_mcp_p95_ms: 300,
            max_vram_used_ratio: 0.75,
            max_vram_used_mb: 3072,
            max_io_wait_ratio: 0.2,
            backlog_priority_weight: 1.0,
            interactive_priority_weight: 1.0,
            shadow_mode_enabled: true,
            allowed_actuators: vec![
                "vector_workers".to_string(),
                "chunk_batch_size".to_string(),
                "file_vectorization_batch_size".to_string(),
            ],
            evaluation_window_ms: 60_000,
        }
    }

    #[test]
    fn collect_operator_policy_caps_vram_to_host_limit() {
        let mut host = host();
        host.vram_total_mb = 2048;
        std::env::set_var("AXON_OPT_MAX_VRAM_USED_MB", "9999");
        let policy = collect_operator_policy_snapshot(&host);
        assert_eq!(policy.max_vram_used_mb, 2048);
        std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
    }

    #[test]
    fn heuristic_policy_returns_a_decision_from_admissible_profiles() {
        let action_profiles = vec![
            ActionProfile {
                id: "hold".to_string(),
                label: "hold".to_string(),
                target_vector_workers: 1,
                target_chunk_batch_size: 32,
                target_file_vectorization_batch_size: 8,
            },
            ActionProfile {
                id: "deepen".to_string(),
                label: "deepen".to_string(),
                target_vector_workers: 1,
                target_chunk_batch_size: 48,
                target_file_vectorization_batch_size: 12,
            },
        ];
        let decision = HeuristicPolicyEngine
            .choose(
                &host(),
                &signals(),
                &policy(),
                &RecentAnalyticsWindow {
                    chunks_embedded_current_hour: 1000,
                    ..Default::default()
                },
                &action_profiles,
            )
            .unwrap();
        assert!(action_profiles.iter().any(|profile| profile.id == decision.action_profile_id));
    }

    #[test]
    fn reward_observation_penalizes_constraint_violations() {
        let previous = signals();
        let mut current = signals();
        current.window_end_ms = 120_000;
        current.chunks_embedded_total = 200;
        current.cpu_usage_ratio = 0.9;
        let obs = observe_reward("d1", &previous, &current, &policy(), 0.0);
        assert!(obs.penalty_cpu > 0.0);
        assert!(obs.reward < obs.throughput_chunks_per_hour);
    }

    #[test]
    fn admissible_action_profiles_always_include_hold() {
        let profiles = build_admissible_action_profiles(&host(), &signals(), &policy());
        assert!(profiles.iter().any(|profile| profile.label == "hold"));
    }
}
