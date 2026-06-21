use crate::embedder::{
    current_embedding_provider_diagnostics, current_gpu_memory_snapshot,
    current_gpu_utilization_snapshot, current_runtime_tuning_state, embedding_lane_config_from_env,
};
use crate::embedding_contract::CHUNK_MODEL_ID;
use crate::graph::GraphStore;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_observability::{process_memory_snapshot, ProcessMemorySnapshot};
use crate::runtime_capacity_profile::RuntimeProfile;
use crate::service_guard;
use crate::vector_control::current_vector_batch_controller_diagnostics;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RuntimeSignalsWindow {
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub captured_at_ms: i64,
    pub source: String,
    pub cpu_usage_ratio: f64,
    pub ram_available_ratio: f64,
    pub io_wait_ratio: f64,
    pub process_memory: ProcessMemorySnapshot,
    pub vram_used_mb: u64,
    pub vram_free_mb: u64,
    pub gpu_utilization_ratio: f64,
    pub gpu_memory_utilization_ratio: f64,
    pub file_vectorization_queue_depth: usize,
    pub graph_projection_queue_depth: usize,
    pub canonical_vector_backlog_depth: usize,
    pub ready_queue_depth_current: u64,
    pub ready_queue_depth_max: u64,
    pub ready_queue_chunks_current: u64,
    pub ready_queue_chunks_max: u64,
    pub ready_replenishment_deficit_current: u64,
    pub ready_replenishment_deficit_max: u64,
    pub active_claimed_current: u64,
    pub prepare_claimed_current: u64,
    pub ready_claimed_current: u64,
    pub persist_queue_depth_current: u64,
    pub persist_queue_depth_max: u64,
    pub persist_claimed_current: u64,
    pub prepare_inflight_current: u64,
    pub prepare_inflight_max: u64,
    pub prepare_inflight_chunks_current: u64,
    pub prepare_inflight_chunks_max: u64,
    pub gpu_idle_wait_ms_total: u64,
    pub prepare_queue_wait_ms_total: u64,
    pub prepare_reply_wait_ms_total: u64,
    pub persist_queue_wait_ms_total: u64,
    pub oldest_ready_batch_age_ms_current: u64,
    pub oldest_ready_batch_age_ms_max: u64,
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
    pub chunk_embedding_writes_total: u64,
    pub files_completed_total: u64,
    pub canonical_chunk_embeddings_total: u64,
    pub canonical_files_embedded_total: u64,
    pub canonical_chunks_embedded_last_minute: u64,
    pub canonical_files_embedded_last_minute: u64,
    pub target_ready_chunks_current: u64,
    pub gpu_ready_low_watermark_chunks: u64,
    pub gpu_ready_high_watermark_chunks: u64,
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
        gpu_name: if provider
            .provider_effective
            .trim()
            .to_ascii_lowercase()
            .starts_with("cuda")
            || provider
                .provider_effective
                .trim()
                .to_ascii_lowercase()
                .starts_with("tensorrt")
            || gpu.total_mb > 0
        {
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
    let max_vram_used_mb =
        env_u64("AXON_OPT_MAX_VRAM_USED_MB", host.vram_total_mb).min(host.vram_total_mb.max(1));
    let max_vram_used_ratio = if host.vram_total_mb > 0 {
        (max_vram_used_mb as f64 / host.vram_total_mb as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    OperatorPolicySnapshot {
        captured_at_ms,
        max_cpu_ratio: env_f64("AXON_OPT_MAX_CPU_RATIO", 0.90).clamp(0.0, 1.0),
        min_ram_available_ratio: env_f64("AXON_OPT_MIN_RAM_AVAILABLE_RATIO", 0.33).clamp(0.0, 1.0),
        max_mcp_p95_ms: env_u64("AXON_OPT_MAX_MCP_P95_MS", 300),
        max_vram_used_ratio,
        max_vram_used_mb,
        max_io_wait_ratio: env_f64("AXON_OPT_MAX_IO_WAIT_RATIO", 0.20).clamp(0.0, 1.0),
        backlog_priority_weight: env_f64("AXON_OPT_BACKLOG_PRIORITY_WEIGHT", 1.0).max(0.0),
        interactive_priority_weight: env_f64("AXON_OPT_INTERACTIVE_PRIORITY_WEIGHT", 1.0).max(0.0),
        shadow_mode_enabled: env_bool("AXON_OPT_SHADOW_MODE_ENABLED", true),
        allowed_actuators: optimizer_allowed_actuators(),
        evaluation_window_ms: env_u64("AXON_OPT_EVALUATION_WINDOW_MS", 15_000).max(10_000),
    }
}

fn optimizer_allowed_actuators() -> Vec<String> {
    let default = || {
        vec![
            "chunk_batch_size".to_string(),
            "file_vectorization_batch_size".to_string(),
            "vector_ready_queue_depth".to_string(),
            "vector_persist_queue_bound".to_string(),
            "vector_max_inflight_persists".to_string(),
            "embed_micro_batch_max_items".to_string(),
            "embed_micro_batch_max_total_tokens".to_string(),
        ]
    };
    match std::env::var("AXON_OPT_ALLOWED_ACTUATORS") {
        Ok(raw) => raw
            .split(',')
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .filter(|item| *item != "vector_workers")
            .map(|item| item.to_string())
            .collect::<Vec<_>>(),
        Err(_) => default(),
    }
}

pub fn collect_runtime_signals_window(store: &GraphStore) -> RuntimeSignalsWindow {
    let now_ms = now_ms();
    let runtime_mode = AxonRuntimeMode::from_env();
    let vector_runtime_enabled = runtime_mode.semantic_workers_enabled();
    let memory = process_memory_snapshot();
    let gpu = current_gpu_memory_snapshot().unwrap_or(crate::embedder::GpuMemorySnapshot {
        total_mb: 0,
        used_mb: 0,
        free_mb: 0,
    });
    let gpu_utilization =
        current_gpu_utilization_snapshot().unwrap_or(crate::embedder::GpuUtilizationSnapshot {
            gpu_utilization_ratio: 0.0,
            memory_utilization_ratio: 0.0,
        });
    // REQ-AXO-901653 Slice 3b / REQ-AXO-901674 — FVQ/GPQ queue tables dropped post
    // MIL-AXO-017 / REQ-AXO-289 / slice-5d. Canonical pipeline_v2 writes Chunk +
    // ChunkEmbedding directly. The signals stay as struct fields (read by 50+
    // optimizer heuristics) but are populated with constant 0 until the
    // optimizer is rewired against pipeline_v2 ready-queue/inflight counters
    // (separate REQ).
    let (file_vectorization_queue_queued, _file_vectorization_queue_inflight): (usize, usize) =
        (0, 0);
    let (vector_outbox_queued, vector_outbox_inflight): (usize, usize) = (0, 0);
    let (graph_projection_queue_queued, graph_projection_queue_inflight): (usize, usize) = (0, 0);
    let vector_latency = service_guard::vector_runtime_latency_summaries();
    let vector_runtime = service_guard::vector_runtime_metrics();
    let controller = current_vector_batch_controller_diagnostics(&embedding_lane_config_from_env());
    let (cpu_usage_ratio, ram_available_ratio, io_wait_ratio) = read_host_pressure_ratios();
    let canonical_backlog_depth = file_vectorization_queue_queued
        .saturating_add(vector_outbox_queued)
        .saturating_add(vector_outbox_inflight);
    let current_state = current_runtime_tuning_state();
    // Display-only target: batch×depth floor, then the live ready-queue high
    // water. Inlined after the predictive optimizer (ActionProfile) retirement.
    let target_ready_chunks_current = (controller
        .target_embed_batch_chunks
        .saturating_mul(current_state.vector_ready_queue_depth)
        .max(controller.target_embed_batch_chunks) as u64)
        .max(
            vector_runtime
                .ready_queue_chunks_current
                .saturating_add(vector_runtime.ready_replenishment_deficit_current),
        );
    RuntimeSignalsWindow {
        window_start_ms: now_ms.saturating_sub(60_000),
        window_end_ms: now_ms,
        captured_at_ms: now_ms,
        source: "optimizer.runtime.window".to_string(),
        cpu_usage_ratio,
        ram_available_ratio,
        io_wait_ratio,
        process_memory: memory,
        vram_used_mb: gpu.used_mb,
        vram_free_mb: gpu.free_mb,
        gpu_utilization_ratio: gpu_utilization.gpu_utilization_ratio,
        gpu_memory_utilization_ratio: gpu_utilization.memory_utilization_ratio,
        file_vectorization_queue_depth: canonical_backlog_depth,
        graph_projection_queue_depth: graph_projection_queue_queued
            + graph_projection_queue_inflight,
        canonical_vector_backlog_depth: canonical_backlog_depth,
        ready_queue_depth_current: vector_runtime.ready_queue_depth_current,
        ready_queue_depth_max: vector_runtime.ready_queue_depth_max,
        ready_queue_chunks_current: vector_runtime.ready_queue_chunks_current,
        ready_queue_chunks_max: vector_runtime.ready_queue_chunks_max,
        ready_replenishment_deficit_current: vector_runtime.ready_replenishment_deficit_current,
        ready_replenishment_deficit_max: vector_runtime.ready_replenishment_deficit_max,
        active_claimed_current: vector_runtime.active_claimed_current,
        prepare_claimed_current: vector_runtime.prepare_claimed_current,
        ready_claimed_current: vector_runtime.ready_claimed_current,
        persist_queue_depth_current: vector_runtime.persist_queue_depth_current,
        persist_queue_depth_max: vector_runtime.persist_queue_depth_max,
        persist_claimed_current: vector_runtime.persist_claimed_current,
        prepare_inflight_current: vector_runtime.prepare_inflight_current,
        prepare_inflight_max: vector_runtime.prepare_inflight_max,
        prepare_inflight_chunks_current: vector_runtime.prepare_inflight_chunks_current,
        prepare_inflight_chunks_max: vector_runtime.prepare_inflight_chunks_max,
        gpu_idle_wait_ms_total: vector_runtime.gpu_idle_wait_ms_total,
        prepare_queue_wait_ms_total: vector_runtime.prepare_queue_wait_ms_total,
        prepare_reply_wait_ms_total: vector_runtime.prepare_reply_wait_ms_total,
        persist_queue_wait_ms_total: vector_runtime.persist_queue_wait_ms_total,
        oldest_ready_batch_age_ms_current: vector_runtime.oldest_ready_batch_age_ms_current,
        oldest_ready_batch_age_ms_max: vector_runtime.oldest_ready_batch_age_ms_max,
        latency_recent_fetch_p95_ms: vector_latency.fetch.p95_ms,
        latency_recent_embed_p95_ms: vector_latency.embed.p95_ms,
        latency_recent_db_write_p95_ms: vector_latency.db_write.p95_ms,
        latency_recent_mark_done_p95_ms: vector_latency.mark_done.p95_ms,
        mcp_latency_recent_ms: service_guard::recent_mcp_latency_ms(),
        vector_workers_active_current: vector_runtime.vector_workers_active_current,
        vector_worker_heartbeat_at_ms: vector_runtime.vector_worker_heartbeat_at_ms,
        embed_inflight_started_at_ms: vector_runtime.embed_inflight_started_at_ms,
        interactive_requests_in_flight: service_guard::interactive_requests_in_flight(),
        interactive_priority: service_guard::current_interactive_priority()
            .as_str()
            .to_string(),
        chunk_embedding_writes_total: vector_runtime.chunks_embedded_total,
        files_completed_total: vector_runtime.files_completed_total,
        canonical_chunk_embeddings_total: if vector_runtime_enabled {
            canonical_count(
                store,
                &format!(
                    "SELECT COUNT(*) FROM ChunkEmbedding WHERE model_id = '{}'",
                    CHUNK_MODEL_ID.replace('\'', "''")
                ),
            ) as u64
        } else {
            0
        },
        canonical_files_embedded_total: if vector_runtime_enabled {
            canonical_count(
                store,
                &format!(
                    "SELECT COUNT(DISTINCT c.file_path) \
                 FROM ChunkEmbedding ce \
                 JOIN Chunk c ON c.id = ce.chunk_id \
                 WHERE ce.model_id = '{}'",
                    CHUNK_MODEL_ID.replace('\'', "''")
                ),
            ) as u64
        } else {
            0
        },
        canonical_chunks_embedded_last_minute: if vector_runtime_enabled {
            canonical_count(
                store,
                &format!(
                    "SELECT COUNT(*) FROM ChunkEmbedding \
                 WHERE model_id = '{}' \
                   AND COALESCE(embedded_at_ms, 0) >= {}",
                    CHUNK_MODEL_ID.replace('\'', "''"),
                    now_ms.saturating_sub(60_000)
                ),
            ) as u64
        } else {
            0
        },
        canonical_files_embedded_last_minute: if vector_runtime_enabled {
            canonical_count(
                store,
                &format!(
                    "SELECT COUNT(DISTINCT c.file_path) \
                 FROM ChunkEmbedding ce \
                 JOIN Chunk c ON c.id = ce.chunk_id \
                 WHERE ce.model_id = '{}' \
                   AND COALESCE(ce.embedded_at_ms, 0) >= {}",
                    CHUNK_MODEL_ID.replace('\'', "''"),
                    now_ms.saturating_sub(60_000)
                ),
            ) as u64
        } else {
            0
        },
        target_ready_chunks_current,
        gpu_ready_low_watermark_chunks: controller.gpu_ready_low_watermark_chunks as u64,
        gpu_ready_high_watermark_chunks: controller.gpu_ready_high_watermark_chunks as u64,
    }
}

pub fn collect_recent_analytics_window(store: &GraphStore) -> RecentAnalyticsWindow {
    let now_ms = now_ms();
    let bucket_start_ms = (now_ms / 3_600_000) * 3_600_000;
    if !AxonRuntimeMode::from_env().semantic_workers_enabled() {
        return RecentAnalyticsWindow {
            collected_at_ms: now_ms,
            current_hour_bucket_start_ms: bucket_start_ms,
            ..RecentAnalyticsWindow::default()
        };
    }
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
    let raw = store
        .query_json_writer(&query)
        .unwrap_or_else(|_| "[]".to_string());
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

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|raw| {
            matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
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
        .or_else(|| {
            value
                .and_then(|value| value.as_i64())
                .map(|v| v.max(0) as u64)
        })
        .or_else(|| {
            value
                .and_then(|value| value.as_str())
                .and_then(|v| v.parse::<u64>().ok())
        })
        .unwrap_or(0)
}

fn canonical_count(store: &GraphStore, query: &str) -> i64 {
    match store.query_count_writer(query) {
        Ok(count) => count.max(0),
        Err(_) => return 0,
    }
}

fn read_host_pressure_ratios() -> (f64, f64, f64) {
    let (cpu_usage_ratio, io_wait_ratio) = read_cpu_and_io_usage_ratios();
    let ram_available_ratio = read_ram_available_ratio().clamp(0.0, 1.0);
    (cpu_usage_ratio, ram_available_ratio, io_wait_ratio)
}

#[derive(Debug, Clone, Copy)]
struct ProcStatSample {
    total: u64,
    idle: u64,
    iowait: u64,
}

static HOST_PRESSURE_SAMPLER: OnceLock<Mutex<Option<ProcStatSample>>> = OnceLock::new();

fn host_pressure_sampler() -> &'static Mutex<Option<ProcStatSample>> {
    HOST_PRESSURE_SAMPLER.get_or_init(|| Mutex::new(None))
}

fn read_cpu_and_io_usage_ratios() -> (f64, f64) {
    let current = read_proc_stat_sample();
    let Some(current) = current else {
        return (0.0, 0.0);
    };
    let sampler = host_pressure_sampler();
    let previous = {
        let mut guard = sampler.lock().unwrap_or_else(|poison| poison.into_inner());
        let previous = *guard;
        *guard = Some(current);
        previous
    };
    previous
        .map(|previous| compute_cpu_and_io_usage_ratios(previous, current))
        .unwrap_or((0.0, 0.0))
}

fn read_proc_stat_sample() -> Option<ProcStatSample> {
    let content = std::fs::read_to_string("/proc/stat").ok()?;
    let line = content.lines().find(|line| line.starts_with("cpu "))?;
    let mut values = line.split_whitespace().skip(1);
    let user = values.next()?.parse::<u64>().ok()?;
    let nice = values.next()?.parse::<u64>().ok()?;
    let system = values.next()?.parse::<u64>().ok()?;
    let idle = values.next()?.parse::<u64>().ok()?;
    let iowait = values.next()?.parse::<u64>().ok()?;
    let irq = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let softirq = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let steal = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    Some(ProcStatSample {
        total: user + nice + system + idle + iowait + irq + softirq + steal,
        idle,
        iowait,
    })
}

fn compute_cpu_and_io_usage_ratios(
    previous: ProcStatSample,
    current: ProcStatSample,
) -> (f64, f64) {
    let total_delta = current.total.saturating_sub(previous.total);
    if total_delta == 0 {
        return (0.0, 0.0);
    }
    let idle_delta = current.idle.saturating_sub(previous.idle);
    let iowait_delta = current.iowait.saturating_sub(previous.iowait);
    let busy_delta = total_delta.saturating_sub(idle_delta);
    (
        ((busy_delta as f64) / (total_delta as f64)).clamp(0.0, 1.0),
        ((iowait_delta as f64) / (total_delta as f64)).clamp(0.0, 1.0),
    )
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

