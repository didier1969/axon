use crate::embedder::{current_runtime_tuning_state, EmbeddingLaneConfig};
use crate::service_guard::{self, ServicePressure};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const MCP_IDLE_TARGET_EMBED_BATCH_MULTIPLIER: usize = 2;
pub const VECTOR_BATCH_CONTROLLER_MIN_EMBED_CALLS: u64 = 1;
pub const VECTOR_BATCH_CONTROLLER_COOLDOWN_MS: u64 = 1_500;
pub const VECTOR_BATCH_CONTROLLER_EMBED_STEP: usize = 24;
pub const VECTOR_BATCH_CONTROLLER_FILES_STEP: usize = 8;
pub const VECTOR_BATCH_CONTROLLER_IDLE_BACKLOG_THRESHOLD: usize = 64;
pub const QUIET_CRUISE_FILE_BACKLOG_THRESHOLD: usize = 32;
pub const AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD: usize = 128;
pub const SYMBOL_BACKLOG_RESIDUAL_THRESHOLD: usize = 64;
pub const CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD: usize = 512;
pub const GPU_VECTOR_BACKLOG_GRAPH_YIELD_THRESHOLD: usize = 2048;
pub const SEMANTIC_REFILL_BACKLOG_THRESHOLD: usize = 16;
pub const SEMANTIC_REFILL_READY_LOW_WATERMARK: u64 = 1;
pub const SEMANTIC_REFILL_PREPARE_LOW_WATERMARK: u64 = 1;
pub const UTILITY_FIRST_SCHEDULER_HOLD_WINDOW_MS: u64 = 2_000;
pub const GPU_CADENCE_IDLE_GAP_UNDERFEED_MS: u64 = 250;
pub const GPU_CADENCE_READY_STALE_UNDERFEED_MS: u64 = 1_000;
const MAX_CHUNKS_PER_FILE: usize = 64;
const MAX_EMBED_BATCH_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_GPU_WARMUP_EMBED_BATCH_CHUNKS: usize = 64;
const DEFAULT_GPU_WARMUP_FILES_PER_CYCLE: usize = 16;
const GPU_IDLE_WAIT_ESCALATION_MS: u64 = 100;
const DEFAULT_GPU_READY_LOW_WATERMARK_BATCHES: usize = 16;
const DEFAULT_GPU_READY_HIGH_WATERMARK_BATCHES: usize = 32;

fn target_ready_chunk_reserve(target_ready_depth: usize, batch_chunk_capacity: usize) -> usize {
    target_ready_depth.saturating_mul(batch_chunk_capacity.max(1))
}

fn default_stock_chunk_unit() -> usize {
    current_runtime_tuning_state().chunk_batch_size.max(1)
}

pub fn configured_target_ready_chunks() -> usize {
    let tuning = current_runtime_tuning_state();
    let legacy_depth_chunks = target_ready_chunk_reserve(
        tuning.vector_ready_queue_depth.max(1),
        default_stock_chunk_unit(),
    );
    std::env::var("AXON_VECTOR_TARGET_READY_CHUNKS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(legacy_depth_chunks)
        .max(default_stock_chunk_unit())
}

pub fn configured_gpu_ready_low_watermark_chunks() -> usize {
    let default_chunks = target_ready_chunk_reserve(
        DEFAULT_GPU_READY_LOW_WATERMARK_BATCHES,
        default_stock_chunk_unit(),
    );
    let low = std::env::var("AXON_GPU_READY_LOW_WATERMARK_CHUNKS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .or_else(|| {
            std::env::var("AXON_GPU_READY_LOW_WATERMARK")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|value| *value > 0)
                .map(|legacy_batches| {
                    target_ready_chunk_reserve(legacy_batches, default_stock_chunk_unit())
                })
        })
        .unwrap_or(default_chunks);
    low.clamp(
        default_stock_chunk_unit(),
        configured_target_ready_chunks().max(default_stock_chunk_unit()),
    )
}

pub fn configured_gpu_ready_high_watermark_chunks() -> usize {
    let default_chunks = target_ready_chunk_reserve(
        DEFAULT_GPU_READY_HIGH_WATERMARK_BATCHES,
        default_stock_chunk_unit(),
    );
    let low = configured_gpu_ready_low_watermark_chunks();
    std::env::var("AXON_GPU_READY_HIGH_WATERMARK_CHUNKS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .or_else(|| {
            std::env::var("AXON_GPU_READY_HIGH_WATERMARK")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|value| *value > 0)
                .map(|legacy_batches| {
                    target_ready_chunk_reserve(legacy_batches, default_stock_chunk_unit())
                })
        })
        .unwrap_or(default_chunks)
        .max(low.saturating_add(default_stock_chunk_unit()))
        .clamp(
            low.saturating_add(default_stock_chunk_unit()),
            configured_target_ready_chunks().max(low.saturating_add(default_stock_chunk_unit())),
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorBatchControllerState {
    Holding,
    IdleDrain,
    InteractiveGuarded,
    GpuMemoryGuarded,
}

impl VectorBatchControllerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Holding => "holding",
            Self::IdleDrain => "idle_drain",
            Self::InteractiveGuarded => "interactive_guarded",
            Self::GpuMemoryGuarded => "gpu_memory_guarded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorDrainState {
    AggressiveDrain,
    InteractiveGuarded,
    QuietCruise,
    Recovery,
    GpuScalingBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtilityFirstSchedulerState {
    BalancedDrain,
    RecoveryOverride,
}

impl UtilityFirstSchedulerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BalancedDrain => "balanced_drain",
            Self::RecoveryOverride => "recovery_override",
        }
    }
}

impl VectorDrainState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AggressiveDrain => "aggressive_drain",
            Self::InteractiveGuarded => "interactive_guarded",
            Self::QuietCruise => "quiet_cruise",
            Self::Recovery => "recovery",
            Self::GpuScalingBlocked => "gpu_scaling_blocked",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VectorBatchControllerDiagnostics {
    pub state: VectorBatchControllerState,
    pub reason: String,
    pub adjustments_total: u64,
    pub last_adjustment_ms: u64,
    pub target_embed_batch_chunks: usize,
    pub target_files_per_cycle: usize,
    pub gpu_ready_low_watermark_chunks: usize,
    pub gpu_ready_high_watermark_chunks: usize,
    pub window_embed_calls: u64,
    pub window_chunks: u64,
    pub window_files_touched: u64,
    pub avg_chunks_per_embed_call: f64,
    pub avg_files_per_embed_call: f64,
    pub embed_ms_per_chunk: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct UtilityFirstSchedulerDiagnostics {
    pub state: UtilityFirstSchedulerState,
    pub reason: &'static str,
    pub semantic_underfeed: bool,
    pub ready_reserve_target: usize,
    pub target_ready_chunks: usize,
    pub hold_window_ms: u64,
}

fn cadence_underfed(
    metrics: service_guard::VectorRuntimeMetrics,
    target_ready_chunks: usize,
) -> bool {
    let ready_chunks = metrics.ready_queue_chunks_current as usize;
    let front_chunk_supply =
        ready_chunks.saturating_add(metrics.prepare_inflight_chunks_current as usize);
    let has_backlog = metrics.canonical_backlog_depth_current > 0 || ready_chunks > 0;
    let gap_threshold_ms = GPU_CADENCE_IDLE_GAP_UNDERFEED_MS
        .max(metrics.avg_embed_attempt_wall_ms.round() as u64)
        .max(metrics.last_embed_attempt_wall_ms);
    let stalled_gap = metrics.last_embed_gap_ms >= gap_threshold_ms && gap_threshold_ms > 0;
    let stale_ready = ready_chunks > 0
        && metrics.oldest_ready_batch_age_ms_current >= GPU_CADENCE_READY_STALE_UNDERFEED_MS;
    has_backlog
        && (front_chunk_supply < target_ready_chunks || stalled_gap || stale_ready)
        && metrics.persist_queue_depth_current == 0
}

#[derive(Debug, Clone, Copy)]
pub struct VectorBatchControllerObservation {
    pub upstream_file_pressure: usize,
    pub front_chunk_supply: usize,
    pub interactive_active: bool,
    pub gpu_memory_pressure: bool,
    pub metrics: service_guard::VectorRuntimeMetrics,
}

#[derive(Debug, Clone, Copy)]
struct VectorBatchControllerBounds {
    min_embed_batch_chunks: usize,
    default_embed_batch_chunks: usize,
    max_embed_batch_chunks: usize,
    gpu_pressure_embed_batch_chunks: usize,
    min_files_per_cycle: usize,
    default_files_per_cycle: usize,
    max_files_per_cycle: usize,
    gpu_pressure_files_per_cycle: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct VectorBatchControllerWindow {
    embed_calls: u64,
    chunks: u64,
    files_touched: u64,
    embed_ms: u64,
}

#[derive(Debug, Clone)]
pub struct VectorBatchController {
    bounds: VectorBatchControllerBounds,
    state: VectorBatchControllerState,
    reason: String,
    adjustments_total: u64,
    last_adjustment_ms: u64,
    target_embed_batch_chunks: usize,
    target_files_per_cycle: usize,
    baseline_metrics: service_guard::VectorRuntimeMetrics,
    last_window_started_ms: u64,
    last_best_embed_ms_per_chunk: Option<f64>,
    consecutive_embed_regressions: u8,
    consecutive_underfed_windows: u8,
    last_window: VectorBatchControllerWindow,
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticPolicy {
    pub profile: &'static str,
    pub pause: bool,
    pub sleep: Duration,
    pub idle_sleep: Duration,
}

fn semantic_policy_profile(
    profile: &'static str,
    pause: bool,
    sleep_ms: u64,
    idle_sleep_ms: u64,
) -> SemanticPolicy {
    SemanticPolicy {
        profile,
        pause,
        sleep: Duration::from_millis(sleep_ms),
        idle_sleep: Duration::from_millis(idle_sleep_ms),
    }
}

static VECTOR_BATCH_CONTROLLER: OnceLock<Mutex<VectorBatchController>> = OnceLock::new();
static UTILITY_FIRST_SCHEDULER: OnceLock<Mutex<UtilityFirstScheduler>> = OnceLock::new();
static GPU_VECTOR_LEASE: OnceLock<Mutex<Option<GpuVectorLease>>> = OnceLock::new();

struct GpuVectorLease {
    _file: File,
    path: PathBuf,
    owner_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuVectorLeaseDiagnostics {
    pub exclusive_required: bool,
    pub path: String,
    pub owned_by_current_instance: bool,
    pub owner_identity: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct UtilityFirstScheduler {
    state: UtilityFirstSchedulerState,
    entered_at_ms: u64,
}

impl Default for UtilityFirstScheduler {
    fn default() -> Self {
        Self {
            state: UtilityFirstSchedulerState::BalancedDrain,
            entered_at_ms: 0,
        }
    }
}

fn utility_first_scheduler() -> &'static Mutex<UtilityFirstScheduler> {
    UTILITY_FIRST_SCHEDULER.get_or_init(|| Mutex::new(UtilityFirstScheduler::default()))
}

fn gpu_vector_lease_slot() -> &'static Mutex<Option<GpuVectorLease>> {
    GPU_VECTOR_LEASE.get_or_init(|| Mutex::new(None))
}

fn gpu_vector_lease_required() -> bool {
    std::env::var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE")
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

fn gpu_vector_lease_owner_identity() -> String {
    std::env::var("AXON_RUNTIME_IDENTITY")
        .or_else(|_| std::env::var("AXON_INSTANCE_KIND"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn gpu_vector_lease_path() -> PathBuf {
    std::env::var("AXON_GPU_VECTOR_LEASE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/axon-gpu-vectorization.lock"))
}

fn try_claim_gpu_vector_lease() -> bool {
    if !gpu_vector_lease_required() {
        return true;
    }
    let path = gpu_vector_lease_path();
    let mut guard = gpu_vector_lease_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if guard.is_some() {
        return true;
    }
    let Ok(file) = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
    else {
        return false;
    };
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        *guard = Some(GpuVectorLease {
            _file: file,
            path,
            owner_identity: gpu_vector_lease_owner_identity(),
        });
        true
    } else {
        false
    }
}

pub fn current_gpu_vector_lease_diagnostics() -> GpuVectorLeaseDiagnostics {
    let path = gpu_vector_lease_path();
    let guard = gpu_vector_lease_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(lease) = guard.as_ref() {
        GpuVectorLeaseDiagnostics {
            exclusive_required: gpu_vector_lease_required(),
            path: lease.path.to_string_lossy().to_string(),
            owned_by_current_instance: true,
            owner_identity: Some(lease.owner_identity.clone()),
        }
    } else {
        GpuVectorLeaseDiagnostics {
            exclusive_required: gpu_vector_lease_required(),
            path: path.to_string_lossy().to_string(),
            owned_by_current_instance: false,
            owner_identity: None,
        }
    }
}

#[cfg(test)]
pub fn reset_gpu_vector_lease_for_tests() {
    let mut guard = gpu_vector_lease_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *guard = None;
}

impl VectorBatchController {
    pub fn new(lane_config: &EmbeddingLaneConfig) -> Self {
        let default_embed_batch_chunks = lane_config.chunk_batch_size.max(1);
        let min_embed_batch_chunks = (default_embed_batch_chunks / 2).max(8);
        let max_embed_batch_chunks = default_embed_batch_chunks.saturating_mul(4).clamp(
            default_embed_batch_chunks,
            default_embed_batch_chunks.max(192),
        );
        let default_files_per_cycle = lane_config.file_vectorization_batch_size.max(1);
        let min_files_per_cycle = (default_files_per_cycle / 2).max(4);
        let max_files_per_cycle = default_files_per_cycle
            .saturating_mul(4)
            .clamp(default_files_per_cycle, default_files_per_cycle.max(64));
        let gpu_pressure_embed_batch_chunks =
            gpu_pressure_embed_batch_chunks(default_embed_batch_chunks, min_embed_batch_chunks);
        let gpu_pressure_files_per_cycle = gpu_pressure_files_per_cycle(default_files_per_cycle);
        Self {
            bounds: VectorBatchControllerBounds {
                min_embed_batch_chunks,
                default_embed_batch_chunks,
                max_embed_batch_chunks,
                gpu_pressure_embed_batch_chunks,
                min_files_per_cycle,
                default_files_per_cycle,
                max_files_per_cycle,
                gpu_pressure_files_per_cycle,
            },
            state: VectorBatchControllerState::Holding,
            reason: "startup".to_string(),
            adjustments_total: 0,
            last_adjustment_ms: 0,
            target_embed_batch_chunks: default_embed_batch_chunks,
            target_files_per_cycle: default_files_per_cycle,
            baseline_metrics: service_guard::VectorRuntimeMetrics::default(),
            last_window_started_ms: 0,
            last_best_embed_ms_per_chunk: None,
            consecutive_embed_regressions: 0,
            consecutive_underfed_windows: 0,
            last_window: VectorBatchControllerWindow::default(),
        }
    }

    pub fn diagnostics(&self) -> VectorBatchControllerDiagnostics {
        let avg_chunks_per_embed_call = if self.last_window.embed_calls > 0 {
            self.last_window.chunks as f64 / self.last_window.embed_calls as f64
        } else {
            0.0
        };
        let avg_files_per_embed_call = if self.last_window.embed_calls > 0 {
            self.last_window.files_touched as f64 / self.last_window.embed_calls as f64
        } else {
            0.0
        };
        let embed_ms_per_chunk = if self.last_window.chunks > 0 {
            self.last_window.embed_ms as f64 / self.last_window.chunks as f64
        } else {
            0.0
        };

        VectorBatchControllerDiagnostics {
            state: self.state,
            reason: self.reason.clone(),
            adjustments_total: self.adjustments_total,
            last_adjustment_ms: self.last_adjustment_ms,
            target_embed_batch_chunks: self.target_embed_batch_chunks,
            target_files_per_cycle: self.target_files_per_cycle,
            gpu_ready_low_watermark_chunks: configured_gpu_ready_low_watermark_chunks(),
            gpu_ready_high_watermark_chunks: configured_gpu_ready_high_watermark_chunks(),
            window_embed_calls: self.last_window.embed_calls,
            window_chunks: self.last_window.chunks,
            window_files_touched: self.last_window.files_touched,
            avg_chunks_per_embed_call,
            avg_files_per_embed_call,
            embed_ms_per_chunk,
        }
    }

    pub fn observe(
        &mut self,
        now_ms: u64,
        observation: VectorBatchControllerObservation,
    ) -> VectorBatchControllerDiagnostics {
        let window = VectorBatchControllerWindow {
            embed_calls: observation
                .metrics
                .embed_calls_total
                .saturating_sub(self.baseline_metrics.embed_calls_total),
            chunks: observation
                .metrics
                .chunks_embedded_total
                .saturating_sub(self.baseline_metrics.chunks_embedded_total),
            files_touched: observation
                .metrics
                .files_touched_total
                .saturating_sub(self.baseline_metrics.files_touched_total),
            embed_ms: observation
                .metrics
                .embed_ms_total
                .saturating_sub(self.baseline_metrics.embed_ms_total),
        };
        let enough_calls = window.embed_calls >= VECTOR_BATCH_CONTROLLER_MIN_EMBED_CALLS;
        let cooldown_elapsed = self.last_adjustment_ms == 0
            || now_ms.saturating_sub(self.last_adjustment_ms)
                >= VECTOR_BATCH_CONTROLLER_COOLDOWN_MS;
        let backlog_meaningful =
            observation.upstream_file_pressure >= VECTOR_BATCH_CONTROLLER_IDLE_BACKLOG_THRESHOLD;

        if observation.gpu_memory_pressure {
            self.state = VectorBatchControllerState::GpuMemoryGuarded;
            self.reason = "gpu_memory_pressure".to_string();
            let desired_embed_batch_chunks = self.bounds.gpu_pressure_embed_batch_chunks;
            let desired_files_per_cycle = self.bounds.gpu_pressure_files_per_cycle;
            if self.target_embed_batch_chunks != desired_embed_batch_chunks
                || self.target_files_per_cycle != desired_files_per_cycle
            {
                self.target_embed_batch_chunks = desired_embed_batch_chunks;
                self.target_files_per_cycle = desired_files_per_cycle;
                self.adjustments_total += 1;
                self.last_adjustment_ms = now_ms;
            }
            self.baseline_metrics = observation.metrics;
            self.last_window_started_ms = now_ms;
            self.last_window = window;
            return self.diagnostics();
        }

        if observation.interactive_active
            && (self.target_embed_batch_chunks > self.bounds.default_embed_batch_chunks
                || self.target_files_per_cycle > self.bounds.default_files_per_cycle)
        {
            self.state = VectorBatchControllerState::InteractiveGuarded;
            self.reason = "interactive_priority".to_string();
            self.target_embed_batch_chunks = self.bounds.default_embed_batch_chunks;
            self.target_files_per_cycle = self.bounds.default_files_per_cycle;
            self.adjustments_total += 1;
            self.last_adjustment_ms = now_ms;
            self.baseline_metrics = observation.metrics;
            self.last_window_started_ms = now_ms;
            self.last_window = window;
            return self.diagnostics();
        }

        if !enough_calls || !cooldown_elapsed {
            self.state = if observation.interactive_active {
                VectorBatchControllerState::InteractiveGuarded
            } else if observation.gpu_memory_pressure {
                VectorBatchControllerState::GpuMemoryGuarded
            } else if backlog_meaningful {
                VectorBatchControllerState::IdleDrain
            } else {
                VectorBatchControllerState::Holding
            };
            self.reason = if !enough_calls {
                "warming_window".to_string()
            } else {
                "cooldown".to_string()
            };
            if !observation.interactive_active && backlog_meaningful && !enough_calls {
                if embedding_provider_requested_is_gpu() {
                    self.target_embed_batch_chunks =
                        gpu_warmup_embed_batch_chunks(self.bounds.default_embed_batch_chunks)
                            .clamp(
                                self.bounds.min_embed_batch_chunks,
                                self.bounds.max_embed_batch_chunks,
                            );
                    self.target_files_per_cycle =
                        gpu_warmup_files_per_cycle(self.bounds.default_files_per_cycle).clamp(
                            self.bounds.min_files_per_cycle,
                            self.bounds.max_files_per_cycle,
                        );
                } else {
                    self.target_embed_batch_chunks = vector_embed_target_chunks(
                        &EmbeddingLaneConfig {
                            query_workers: 0,
                            vector_workers: 0,
                            graph_workers: 0,
                            chunk_batch_size: self.bounds.default_embed_batch_chunks,
                            file_vectorization_batch_size: self.bounds.default_files_per_cycle,
                            graph_batch_size: 0,
                            max_chunks_per_file: MAX_CHUNKS_PER_FILE,
                            max_embed_batch_bytes: MAX_EMBED_BATCH_BYTES,
                        },
                        false,
                    )
                    .clamp(
                        self.bounds.min_embed_batch_chunks,
                        self.bounds.max_embed_batch_chunks,
                    );
                }
            }
            return self.diagnostics();
        }

        self.last_window = window;
        let avg_chunks_per_embed_call = if window.embed_calls > 0 {
            window.chunks as f64 / window.embed_calls as f64
        } else {
            0.0
        };
        let avg_files_per_embed_call = if window.embed_calls > 0 {
            window.files_touched as f64 / window.embed_calls as f64
        } else {
            0.0
        };
        let embed_ms_per_chunk = if window.chunks > 0 {
            window.embed_ms as f64 / window.chunks as f64
        } else {
            0.0
        };
        let target_ready_chunks = vector_ready_chunk_reserve_target(
            configured_target_ready_chunks(),
            observation.upstream_file_pressure,
            self.target_files_per_cycle,
            self.target_embed_batch_chunks,
            observation.metrics.ready_queue_chunks_current as usize,
            observation.metrics.prepare_inflight_chunks_current as usize,
            avg_chunks_per_embed_call,
            observation.metrics.oldest_ready_batch_age_ms_current,
        );
        let ready_queue_starved = observation.metrics.ready_queue_chunks_current == 0;
        let ready_queue_under_floor = observation.front_chunk_supply < target_ready_chunks;
        let persist_congested = observation.metrics.persist_queue_depth_current > 0;
        let gpu_idle_wait_delta = observation
            .metrics
            .gpu_idle_wait_ms_total
            .saturating_sub(self.baseline_metrics.gpu_idle_wait_ms_total);
        let gpu_single_worker_cruise = gpu_single_worker_cruise_mode(observation);
        let mut desired_embed_chunks = self.target_embed_batch_chunks;
        let mut desired_files_per_cycle = self.target_files_per_cycle;
        let mut reason = "holding_density".to_string();
        let state = if observation.interactive_active {
            VectorBatchControllerState::InteractiveGuarded
        } else if observation.gpu_memory_pressure {
            VectorBatchControllerState::GpuMemoryGuarded
        } else if backlog_meaningful {
            VectorBatchControllerState::IdleDrain
        } else {
            VectorBatchControllerState::Holding
        };
        let underfed_chunks =
            avg_chunks_per_embed_call < (self.target_embed_batch_chunks as f64 * 0.75);
        let underfed_files = underfed_chunks && avg_files_per_embed_call < 1.5;
        let low_density_collapse = underfed_chunks
            && avg_files_per_embed_call >= 1.5
            && avg_chunks_per_embed_call < (self.target_embed_batch_chunks as f64 * 0.60);
        let meaningful_density_window =
            avg_chunks_per_embed_call >= (self.target_embed_batch_chunks as f64 * 0.50);
        let embed_regressed = !ready_queue_under_floor
            && !ready_queue_starved
            && meaningful_density_window
            && self.last_best_embed_ms_per_chunk.is_some_and(|best| {
                embed_ms_per_chunk > best * if gpu_single_worker_cruise { 1.35 } else { 1.20 }
            });

        if embed_regressed {
            self.consecutive_embed_regressions =
                self.consecutive_embed_regressions.saturating_add(1);
        } else {
            self.consecutive_embed_regressions = 0;
        }

        let underfed_window = low_density_collapse
            || ((!observation.interactive_active && backlog_meaningful)
                && (ready_queue_under_floor
                    || gpu_idle_wait_delta > GPU_IDLE_WAIT_ESCALATION_MS
                    || underfed_chunks
                    || underfed_files));
        if underfed_window {
            self.consecutive_underfed_windows = self.consecutive_underfed_windows.saturating_add(1);
        } else {
            self.consecutive_underfed_windows = 0;
        }

        if embed_regressed
            && self.consecutive_embed_regressions >= if gpu_single_worker_cruise { 2 } else { 1 }
        {
            reason = "embed_efficiency_regressed".to_string();
            if desired_embed_chunks > self.bounds.default_embed_batch_chunks {
                desired_embed_chunks = desired_embed_chunks
                    .saturating_sub(VECTOR_BATCH_CONTROLLER_EMBED_STEP)
                    .max(self.bounds.default_embed_batch_chunks);
            } else if desired_files_per_cycle > self.bounds.default_files_per_cycle {
                desired_files_per_cycle = desired_files_per_cycle
                    .saturating_sub(VECTOR_BATCH_CONTROLLER_FILES_STEP)
                    .max(self.bounds.default_files_per_cycle);
            }
            self.consecutive_embed_regressions = 0;
        } else if !observation.interactive_active && backlog_meaningful {
            if low_density_collapse && (ready_queue_under_floor || ready_queue_starved) {
                reason = "ready_queue_starved_low_density".to_string();
                let persistent_underfeed = self.consecutive_underfed_windows >= 2;
                let embed_step = VECTOR_BATCH_CONTROLLER_EMBED_STEP
                    * if gpu_single_worker_cruise {
                        if persistent_underfeed {
                            3
                        } else {
                            2
                        }
                    } else if persistent_underfeed {
                        2
                    } else {
                        1
                    };
                desired_embed_chunks = desired_embed_chunks
                    .saturating_sub(embed_step)
                    .max(self.bounds.default_embed_batch_chunks);
                let files_step = VECTOR_BATCH_CONTROLLER_FILES_STEP
                    * if gpu_single_worker_cruise {
                        if persistent_underfeed {
                            3
                        } else {
                            2
                        }
                    } else if persistent_underfeed {
                        2
                    } else {
                        1
                    };
                desired_files_per_cycle = desired_files_per_cycle
                    .saturating_add(files_step)
                    .min(self.bounds.max_files_per_cycle);
            } else if ready_queue_under_floor
                || gpu_idle_wait_delta > GPU_IDLE_WAIT_ESCALATION_MS
                || underfed_chunks
                || underfed_files
            {
                let persistent_underfeed = self.consecutive_underfed_windows >= 2;
                reason = if ready_queue_under_floor
                    || gpu_idle_wait_delta > GPU_IDLE_WAIT_ESCALATION_MS
                {
                    "ready_queue_starved".to_string()
                } else {
                    "idle_underfed".to_string()
                };
                if underfed_chunks || ready_queue_under_floor {
                    let embed_step = VECTOR_BATCH_CONTROLLER_EMBED_STEP
                        * if gpu_single_worker_cruise {
                            if ready_queue_under_floor {
                                if persistent_underfeed {
                                    4
                                } else {
                                    3
                                }
                            } else {
                                if persistent_underfeed {
                                    3
                                } else {
                                    2
                                }
                            }
                        } else if ready_queue_under_floor {
                            if persistent_underfeed {
                                3
                            } else {
                                2
                            }
                        } else if persistent_underfeed {
                            2
                        } else {
                            1
                        };
                    desired_embed_chunks = desired_embed_chunks
                        .saturating_add(embed_step)
                        .min(self.bounds.max_embed_batch_chunks);
                }
                if underfed_files || ready_queue_under_floor || ready_queue_starved {
                    let files_step = VECTOR_BATCH_CONTROLLER_FILES_STEP
                        * if gpu_single_worker_cruise {
                            if persistent_underfeed {
                                3
                            } else {
                                2
                            }
                        } else if ready_queue_under_floor {
                            if persistent_underfeed {
                                3
                            } else {
                                2
                            }
                        } else if persistent_underfeed {
                            2
                        } else {
                            1
                        };
                    desired_files_per_cycle = desired_files_per_cycle
                        .saturating_add(files_step)
                        .min(self.bounds.max_files_per_cycle);
                }
            } else if persist_congested {
                reason = "persist_congested".to_string();
                desired_files_per_cycle = desired_files_per_cycle
                    .saturating_sub(VECTOR_BATCH_CONTROLLER_FILES_STEP)
                    .max(self.bounds.default_files_per_cycle);
            }
        }

        self.state = state;
        self.reason = reason;
        if desired_embed_chunks != self.target_embed_batch_chunks
            || desired_files_per_cycle != self.target_files_per_cycle
        {
            self.target_embed_batch_chunks = desired_embed_chunks.clamp(
                self.bounds.min_embed_batch_chunks,
                self.bounds.max_embed_batch_chunks,
            );
            self.target_files_per_cycle = desired_files_per_cycle.clamp(
                self.bounds.min_files_per_cycle,
                self.bounds.max_files_per_cycle,
            );
            self.adjustments_total += 1;
            self.last_adjustment_ms = now_ms;
        }
        if window.chunks > 0 {
            self.last_best_embed_ms_per_chunk = Some(
                self.last_best_embed_ms_per_chunk
                    .map(|best| best.min(embed_ms_per_chunk))
                    .unwrap_or(embed_ms_per_chunk),
            );
        }
        self.baseline_metrics = observation.metrics;
        self.last_window_started_ms = now_ms;
        self.diagnostics()
    }
}

fn gpu_single_worker_cruise_mode(observation: VectorBatchControllerObservation) -> bool {
    if observation.interactive_active
        || observation.gpu_memory_pressure
        || !embedding_provider_requested_is_gpu()
    {
        return false;
    }

    let tuning = current_runtime_tuning_state();
    tuning.vector_workers <= 1
        && observation.upstream_file_pressure >= VECTOR_BATCH_CONTROLLER_IDLE_BACKLOG_THRESHOLD * 8
}

fn vector_batch_controller_slot(
    lane_config: &EmbeddingLaneConfig,
) -> &'static Mutex<VectorBatchController> {
    VECTOR_BATCH_CONTROLLER.get_or_init(|| Mutex::new(VectorBatchController::new(lane_config)))
}

pub fn current_vector_batch_controller_diagnostics(
    lane_config: &EmbeddingLaneConfig,
) -> VectorBatchControllerDiagnostics {
    vector_batch_controller_slot(lane_config)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .diagnostics()
}

pub fn observe_vector_batch_controller(
    lane_config: &EmbeddingLaneConfig,
    observation: VectorBatchControllerObservation,
) -> VectorBatchControllerDiagnostics {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    vector_batch_controller_slot(lane_config)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .observe(now_ms, observation)
}

pub fn reset_vector_batch_controller_for_tests(lane_config: &EmbeddingLaneConfig) {
    let slot = vector_batch_controller_slot(lane_config);
    *slot.lock().unwrap_or_else(|poison| poison.into_inner()) =
        VectorBatchController::new(lane_config);
}

pub fn gpu_pressure_embed_batch_chunks(
    default_embed_batch_chunks: usize,
    min_embed_batch_chunks: usize,
) -> usize {
    std::env::var("AXON_GPU_PRESSURE_EMBED_BATCH_CHUNKS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or_else(|| (default_embed_batch_chunks / 2).max(16))
        .clamp(
            min_embed_batch_chunks.max(1),
            default_embed_batch_chunks.max(1),
        )
}

pub fn gpu_pressure_files_per_cycle(default_files_per_cycle: usize) -> usize {
    std::env::var("AXON_GPU_PRESSURE_FILES_PER_CYCLE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or_else(|| (default_files_per_cycle / 2).max(4))
        .clamp(
            (default_files_per_cycle / 2).max(4),
            default_files_per_cycle.max(4),
        )
}

pub fn vector_embed_target_chunks(
    lane_config: &EmbeddingLaneConfig,
    interactive_active: bool,
) -> usize {
    if interactive_active {
        lane_config.chunk_batch_size.max(1)
    } else {
        let idle_multiplier = if lane_config.chunk_batch_size >= 32 {
            1
        } else {
            MCP_IDLE_TARGET_EMBED_BATCH_MULTIPLIER
        };
        lane_config
            .chunk_batch_size
            .saturating_mul(idle_multiplier)
            .clamp(1, 256)
    }
}

pub fn vector_claim_target(
    target_files_per_cycle: usize,
    avg_files_per_embed_call: f64,
    target_embed_batch_chunks: usize,
    avg_chunks_per_embed_call: f64,
    target_ready_depth: usize,
    current_ready_depth: usize,
    queue_pending: usize,
) -> usize {
    if target_files_per_cycle == 0 {
        return 0;
    }
    let min_claim_target = target_files_per_cycle.clamp(4, 24);
    let avg_chunks_per_file = if avg_chunks_per_embed_call.is_finite()
        && avg_chunks_per_embed_call > 0.0
        && avg_files_per_embed_call.is_finite()
        && avg_files_per_embed_call > 0.0
    {
        (avg_chunks_per_embed_call / avg_files_per_embed_call).max(1.0)
    } else {
        std::env::var("AXON_VECTOR_DEFAULT_CHUNKS_PER_FILE")
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .unwrap_or(4.0)
            .max(1.0)
    };
    let desired_files_for_chunks =
        ((target_embed_batch_chunks.max(1) as f64) / avg_chunks_per_file).ceil() as usize;
    let max_claim_target = target_files_per_cycle
        .saturating_mul(target_ready_depth.max(2))
        .max(desired_files_for_chunks.saturating_mul(target_ready_depth.max(2)))
        .clamp(
            min_claim_target,
            queue_pending.max(min_claim_target).min(4_096),
        );
    let reserve_deficit = target_ready_depth
        .saturating_sub(current_ready_depth)
        .max(1);
    let per_batch_files = desired_files_for_chunks
        .max(target_files_per_cycle / 2)
        .max(2);
    let backlog_bonus_batches = if queue_pending >= 4_096 {
        2
    } else if queue_pending >= 512 {
        1
    } else {
        0
    };
    per_batch_files
        .saturating_mul(reserve_deficit.saturating_add(backlog_bonus_batches))
        .saturating_add(2)
        .clamp(min_claim_target, max_claim_target)
}

pub fn vector_ready_reserve_target(
    configured_ready_depth: usize,
    queue_pending: usize,
    target_files_per_cycle: usize,
    target_embed_batch_chunks: usize,
    current_ready_depth: usize,
    prepare_inflight_depth: usize,
    avg_chunks_per_embed_call: f64,
    oldest_ready_batch_age_ms: u64,
) -> usize {
    let configured_ready_chunks = target_ready_chunk_reserve(
        configured_ready_depth.max(1),
        target_embed_batch_chunks.max(1),
    );
    let current_ready_chunks =
        target_ready_chunk_reserve(current_ready_depth, target_embed_batch_chunks.max(1));
    let prepare_inflight_chunks =
        target_ready_chunk_reserve(prepare_inflight_depth, target_embed_batch_chunks.max(1));
    vector_ready_chunk_reserve_target(
        configured_ready_chunks,
        queue_pending,
        target_files_per_cycle,
        target_embed_batch_chunks,
        current_ready_chunks,
        prepare_inflight_chunks,
        avg_chunks_per_embed_call,
        oldest_ready_batch_age_ms,
    )
    .div_ceil(target_embed_batch_chunks.max(1))
}

pub fn current_vector_drain_state(
    file_backlog_depth: usize,
    service_pressure: ServicePressure,
    interactive_active: bool,
    provider_requested: &str,
    provider_effective: &str,
) -> VectorDrainState {
    let provider_effective_is_gpu = provider_effective
        .trim()
        .to_ascii_lowercase()
        .starts_with("cuda")
        || provider_effective
            .trim()
            .to_ascii_lowercase()
            .starts_with("tensorrt");
    if file_backlog_depth == 0 {
        return VectorDrainState::QuietCruise;
    }
    if interactive_active {
        return VectorDrainState::InteractiveGuarded;
    }
    if matches!(service_pressure, ServicePressure::Critical) {
        return VectorDrainState::Recovery;
    }
    if provider_requested.eq_ignore_ascii_case("cuda") && !provider_effective_is_gpu {
        return VectorDrainState::GpuScalingBlocked;
    }
    VectorDrainState::AggressiveDrain
}

pub fn current_utility_first_scheduler_diagnostics(
    graph_queue_depth: usize,
    file_backlog_depth: usize,
    service_pressure: ServicePressure,
) -> UtilityFirstSchedulerDiagnostics {
    let metrics = service_guard::vector_runtime_metrics();
    let tuning = current_runtime_tuning_state();
    let lane_config = EmbeddingLaneConfig {
        query_workers: 1,
        vector_workers: tuning.vector_workers.max(1),
        graph_workers: tuning.graph_workers,
        chunk_batch_size: tuning.chunk_batch_size.max(1),
        file_vectorization_batch_size: tuning.file_vectorization_batch_size.max(1),
        graph_batch_size: 1,
        max_chunks_per_file: MAX_CHUNKS_PER_FILE,
        max_embed_batch_bytes: MAX_EMBED_BATCH_BYTES,
    };
    let controller = current_vector_batch_controller_diagnostics(&lane_config);
    let interactive_active = service_guard::interactive_priority_active()
        || service_guard::interactive_requests_in_flight() > 0;
    let target_ready_chunks = vector_ready_chunk_reserve_target(
        configured_target_ready_chunks(),
        file_backlog_depth,
        controller.target_files_per_cycle,
        controller.target_embed_batch_chunks,
        metrics.ready_queue_chunks_current as usize,
        metrics.prepare_inflight_chunks_current as usize,
        controller.avg_chunks_per_embed_call,
        metrics.oldest_ready_batch_age_ms_current,
    );
    let front_chunk_supply = (metrics.ready_queue_chunks_current as usize)
        .saturating_add(metrics.prepare_inflight_chunks_current as usize);
    let stock_underfeed = file_backlog_depth >= SEMANTIC_REFILL_BACKLOG_THRESHOLD
        && (front_chunk_supply < target_ready_chunks
            || metrics.ready_replenishment_deficit_current > 0);
    let cadence_underfeed = cadence_underfed(metrics, target_ready_chunks);
    let semantic_underfeed = stock_underfeed || cadence_underfeed;
    let persist_congested = metrics.persist_queue_depth_current > 0;
    let desired_state =
        if interactive_active || matches!(service_pressure, ServicePressure::Critical) {
            UtilityFirstSchedulerState::RecoveryOverride
        } else {
            UtilityFirstSchedulerState::BalancedDrain
        };
    let reason = if interactive_active {
        "interactive_priority"
    } else if matches!(service_pressure, ServicePressure::Critical) {
        "service_pressure_critical"
    } else if cadence_underfeed && !persist_congested {
        "gpu_cadence_underfed"
    } else if semantic_underfeed && !persist_congested {
        "semantic_underfed"
    } else if persist_congested {
        "persist_congested"
    } else if graph_queue_depth > 0 {
        "graph_backlog_observed"
    } else {
        "steady_balanced"
    };
    let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let mut guard = utility_first_scheduler()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let hold_active = guard.entered_at_ms > 0
        && now_ms.saturating_sub(guard.entered_at_ms) < UTILITY_FIRST_SCHEDULER_HOLD_WINDOW_MS;
    if desired_state != guard.state
        && (!hold_active || matches!(desired_state, UtilityFirstSchedulerState::RecoveryOverride))
    {
        guard.state = desired_state;
        guard.entered_at_ms = now_ms;
    } else if guard.entered_at_ms == 0 {
        guard.entered_at_ms = now_ms;
    }
    UtilityFirstSchedulerDiagnostics {
        state: guard.state,
        reason,
        semantic_underfeed,
        ready_reserve_target: target_ready_chunks,
        target_ready_chunks,
        hold_window_ms: UTILITY_FIRST_SCHEDULER_HOLD_WINDOW_MS,
    }
}

pub fn vector_ready_chunk_reserve_target(
    configured_ready_chunks: usize,
    upstream_file_pressure: usize,
    target_files_per_cycle: usize,
    target_embed_batch_chunks: usize,
    current_ready_chunks: usize,
    prepare_inflight_chunks: usize,
    avg_chunks_per_embed_call: f64,
    oldest_ready_batch_age_ms: u64,
) -> usize {
    let batch_chunk_capacity = target_embed_batch_chunks.max(1);
    let avg_chunks_per_file = std::env::var("AXON_VECTOR_DEFAULT_CHUNKS_PER_FILE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4);
    let per_cycle_chunks = target_embed_batch_chunks.max(
        target_files_per_cycle
            .max(1)
            .saturating_mul(avg_chunks_per_file),
    );
    let reserve_floor = configured_ready_chunks
        .max(per_cycle_chunks)
        .max(batch_chunk_capacity);
    let backlog_bonus = if upstream_file_pressure >= 4_096 {
        per_cycle_chunks
    } else if upstream_file_pressure >= 512 {
        per_cycle_chunks / 2
    } else {
        0
    };
    let mut reserve = reserve_floor.saturating_add(backlog_bonus);
    let supply_in_flight = current_ready_chunks.saturating_add(prepare_inflight_chunks);
    if current_ready_chunks == 0
        && upstream_file_pressure >= AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD
    {
        reserve = reserve.max(reserve_floor.saturating_add(per_cycle_chunks / 2));
    }
    if upstream_file_pressure > 0 {
        let mut safety_stock = 0usize;
        let reorder_gap = reserve.saturating_sub(supply_in_flight);
        if reorder_gap > 0 {
            safety_stock = safety_stock
                .saturating_add(reorder_gap.min(configured_ready_chunks.max(per_cycle_chunks / 2)));
        }
        if prepare_inflight_chunks == 0 {
            safety_stock =
                safety_stock.saturating_add((per_cycle_chunks / 2).max(batch_chunk_capacity));
        } else if prepare_inflight_chunks < (per_cycle_chunks / 2) {
            safety_stock =
                safety_stock.saturating_add((per_cycle_chunks / 4).max(batch_chunk_capacity / 2));
        }
        let effective_density_ratio = if target_embed_batch_chunks > 0
            && avg_chunks_per_embed_call.is_finite()
            && avg_chunks_per_embed_call > 0.0
        {
            (avg_chunks_per_embed_call / target_embed_batch_chunks as f64).clamp(0.0, 2.0)
        } else {
            1.0
        };
        if effective_density_ratio < 0.60 {
            safety_stock =
                safety_stock.saturating_add((per_cycle_chunks / 2).max(batch_chunk_capacity));
        } else if effective_density_ratio < 0.80 {
            safety_stock =
                safety_stock.saturating_add((per_cycle_chunks / 4).max(batch_chunk_capacity / 2));
        }
        if oldest_ready_batch_age_ms >= 1_500 && supply_in_flight <= configured_ready_chunks / 2 {
            safety_stock =
                safety_stock.saturating_add((per_cycle_chunks / 4).max(batch_chunk_capacity / 2));
        }
        reserve = reserve.saturating_add(safety_stock);
    }
    let reserve_ceiling = configured_ready_chunks
        .saturating_mul(4)
        .max(per_cycle_chunks.saturating_mul(4))
        .max(batch_chunk_capacity);
    reserve.clamp(batch_chunk_capacity, reserve_ceiling)
}

pub fn allowed_gpu_vector_workers(
    file_backlog_depth: usize,
    service_pressure: ServicePressure,
) -> usize {
    match service_pressure {
        ServicePressure::Critical => 1,
        ServicePressure::Degraded => 2,
        ServicePressure::Recovering | ServicePressure::Healthy => {
            if file_backlog_depth >= AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD {
                6
            } else {
                2
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorWorkerAdmissionDecision {
    pub admitted: bool,
    pub reason: &'static str,
    pub allowed_gpu_workers: usize,
}

pub fn vector_worker_admission_decision(
    worker_idx: usize,
    service_pressure: ServicePressure,
    gpu_available: bool,
    file_backlog_depth: usize,
) -> VectorWorkerAdmissionDecision {
    let interactive_in_flight = service_guard::interactive_requests_in_flight();
    if interactive_in_flight > 0 {
        let allowed_interactive_workers = if gpu_available { 1 } else { 0 };
        if worker_idx >= allowed_interactive_workers {
            service_guard::record_vectorization_suppressed();
            return VectorWorkerAdmissionDecision {
                admitted: false,
                reason: "interactive_requests_in_flight",
                allowed_gpu_workers: allowed_interactive_workers,
            };
        }
    } else if service_guard::interactive_priority_active() {
        service_guard::record_vectorization_suppressed();
        return VectorWorkerAdmissionDecision {
            admitted: false,
            reason: "interactive_priority_active",
            allowed_gpu_workers: 0,
        };
    }
    if !gpu_available {
        return VectorWorkerAdmissionDecision {
            admitted: true,
            reason: "gpu_not_required",
            allowed_gpu_workers: 0,
        };
    }
    if !try_claim_gpu_vector_lease() {
        service_guard::record_vectorization_suppressed();
        return VectorWorkerAdmissionDecision {
            admitted: false,
            reason: "gpu_vector_lease_unavailable",
            allowed_gpu_workers: 0,
        };
    }
    let allowed_gpu_workers = allowed_gpu_vector_workers(file_backlog_depth, service_pressure);
    if worker_idx >= allowed_gpu_workers {
        service_guard::record_vectorization_suppressed();
        return VectorWorkerAdmissionDecision {
            admitted: false,
            reason: "allowed_gpu_worker_cap",
            allowed_gpu_workers,
        };
    }
    VectorWorkerAdmissionDecision {
        admitted: true,
        reason: "admitted",
        allowed_gpu_workers,
    }
}

pub fn vector_worker_admitted(
    worker_idx: usize,
    service_pressure: ServicePressure,
    gpu_available: bool,
    file_backlog_depth: usize,
) -> bool {
    vector_worker_admission_decision(
        worker_idx,
        service_pressure,
        gpu_available,
        file_backlog_depth,
    )
    .admitted
}

pub fn semantic_policy(queue_len: usize, service_pressure: ServicePressure) -> SemanticPolicy {
    semantic_policy_with_graph(queue_len, 0, service_pressure)
}

pub fn baseline_semantic_policy(queue_len: usize) -> SemanticPolicy {
    if queue_len == 0 {
        return semantic_policy_profile("quiescent_idle", false, 250, 20_000);
    }
    if queue_len >= AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD {
        return semantic_policy_profile("aggressive_drain", false, 100, 250);
    }
    semantic_policy_profile("balanced_drain", false, 250, 750)
}

pub fn target_semantic_policy_with_graph(
    queue_len: usize,
    graph_queue_depth: usize,
    service_pressure: ServicePressure,
) -> SemanticPolicy {
    if service_guard::interactive_priority_active() {
        service_guard::record_vectorization_suppressed();
        return semantic_policy_profile("interactive_guarded", false, 750, 1_500);
    }
    if queue_len == 0 {
        return semantic_policy_profile("quiescent_idle", false, 50, 2_000);
    }
    if service_pressure == ServicePressure::Critical {
        return semantic_policy_profile("critical_pause", true, 2_000, 2_000);
    }
    let scheduler =
        current_utility_first_scheduler_diagnostics(graph_queue_depth, queue_len, service_pressure);
    if scheduler.reason == "gpu_cadence_underfed" {
        return semantic_policy_profile("gpu_cadence_refill", false, 1, 5);
    }
    if scheduler.semantic_underfeed {
        return semantic_policy_profile("semantic_refill", false, 5, 20);
    }
    if service_guard::interactive_requests_in_flight() > 0 {
        return semantic_policy_profile("interactive_soft_guard", false, 500, 1_000);
    }
    if queue_len >= AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD {
        return semantic_policy_profile("aggressive_drain", false, 10, 40);
    }
    if matches!(
        service_pressure,
        ServicePressure::Degraded | ServicePressure::Recovering
    ) {
        return semantic_policy_profile("recovery_guarded", false, 50, 150);
    }
    semantic_policy_profile("balanced_drain", false, 25, 75)
}

fn scale_duration(duration: Duration, scale_pct: usize, min_ms: u64, max_ms: u64) -> Duration {
    let scaled_ms = (duration.as_millis() as u128)
        .saturating_mul(scale_pct as u128)
        .saturating_div(100)
        .clamp(min_ms as u128, max_ms as u128) as u64;
    Duration::from_millis(scaled_ms)
}

pub fn apply_semantic_policy_runtime_tuning(target: SemanticPolicy) -> SemanticPolicy {
    let tuning = current_runtime_tuning_state();
    SemanticPolicy {
        profile: target.profile,
        pause: target.pause,
        sleep: scale_duration(target.sleep, tuning.semantic_sleep_scale_pct, 1, 10_000),
        idle_sleep: scale_duration(
            target.idle_sleep,
            tuning.semantic_idle_sleep_scale_pct,
            10,
            60_000,
        ),
    }
}

pub fn semantic_policy_with_graph(
    queue_len: usize,
    graph_queue_depth: usize,
    service_pressure: ServicePressure,
) -> SemanticPolicy {
    apply_semantic_policy_runtime_tuning(target_semantic_policy_with_graph(
        queue_len,
        graph_queue_depth,
        service_pressure,
    ))
}

pub fn symbol_embedding_allowed(
    file_backlog_depth: usize,
    service_pressure: ServicePressure,
) -> bool {
    let enabled = std::env::var("AXON_VECTOR_ENABLE_SYMBOL_EMBEDDING")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false);
    if !enabled {
        return false;
    }
    if service_guard::interactive_priority_active() {
        return false;
    }
    if service_guard::interactive_requests_in_flight() > 0 {
        return false;
    }
    if file_backlog_depth >= SYMBOL_BACKLOG_RESIDUAL_THRESHOLD {
        return false;
    }
    matches!(
        service_pressure,
        ServicePressure::Healthy | ServicePressure::Recovering
    )
}

pub fn graph_projection_allowed(
    _queue_len: usize,
    service_pressure: ServicePressure,
    _vector_backlog_depth: usize,
    _gpu_available: bool,
) -> bool {
    if service_guard::interactive_priority_active() {
        service_guard::record_projection_suppressed();
        return false;
    }
    if service_guard::interactive_requests_in_flight() > 0 {
        service_guard::record_projection_suppressed();
        return false;
    }
    if service_pressure != ServicePressure::Healthy {
        service_guard::record_projection_suppressed();
        return false;
    }
    true
}

#[cfg(test)]
pub fn reset_utility_first_scheduler_for_tests() {
    let mut guard = utility_first_scheduler()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *guard = UtilityFirstScheduler::default();
}

fn embedding_provider_requested_is_gpu() -> bool {
    std::env::var("AXON_EMBEDDING_PROVIDER")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("cuda"))
        .unwrap_or(false)
}

fn gpu_warmup_embed_batch_chunks(default_embed_batch_chunks: usize) -> usize {
    std::env::var("AXON_GPU_WARMUP_EMBED_BATCH_CHUNKS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(DEFAULT_GPU_WARMUP_EMBED_BATCH_CHUNKS)
        .clamp(1, default_embed_batch_chunks.max(1))
}

fn gpu_warmup_files_per_cycle(default_files_per_cycle: usize) -> usize {
    std::env::var("AXON_GPU_WARMUP_FILES_PER_CYCLE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(DEFAULT_GPU_WARMUP_FILES_PER_CYCLE)
        .clamp(
            (default_files_per_cycle / 2).max(4),
            default_files_per_cycle.max(4),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_guard::ServicePressure;
    use tempfile::tempdir;

    #[test]
    fn test_allowed_gpu_vector_workers_returns_4_on_high_backlog() {
        let allowed = allowed_gpu_vector_workers(2000, ServicePressure::Healthy);
        assert_eq!(allowed, 6);
    }

    #[test]
    fn test_gpu_vector_lease_is_claimed_by_current_instance() {
        let dir = tempdir().expect("tempdir");
        let lease_path = dir.path().join("gpu-vector.lock");
        unsafe {
            std::env::set_var("AXON_GPU_VECTOR_LEASE_PATH", &lease_path);
            std::env::set_var("AXON_RUNTIME_IDENTITY", "test-dev");
            std::env::set_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE", "true");
        }
        reset_gpu_vector_lease_for_tests();

        assert!(vector_worker_admitted(
            0,
            ServicePressure::Healthy,
            true,
            256
        ));

        let diagnostics = current_gpu_vector_lease_diagnostics();
        assert!(diagnostics.exclusive_required);
        assert!(diagnostics.owned_by_current_instance);
        assert_eq!(diagnostics.owner_identity.as_deref(), Some("test-dev"));
        assert_eq!(diagnostics.path, lease_path.to_string_lossy().to_string());

        unsafe {
            std::env::remove_var("AXON_GPU_VECTOR_LEASE_PATH");
            std::env::remove_var("AXON_RUNTIME_IDENTITY");
            std::env::remove_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE");
        }
        reset_gpu_vector_lease_for_tests();
    }
}
