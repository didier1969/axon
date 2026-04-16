use crate::embedder::EmbeddingLaneConfig;
use crate::service_guard::{self, ServicePressure};
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
const MAX_CHUNKS_PER_FILE: usize = 64;
const MAX_EMBED_BATCH_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_GPU_WARMUP_EMBED_BATCH_CHUNKS: usize = 64;
const DEFAULT_GPU_WARMUP_FILES_PER_CYCLE: usize = 16;
const READY_RESERVE_FLOOR: usize = 16;
const READY_RESERVE_HEAVY_BACKLOG: usize = 24;
const READY_RESERVE_EXTREME_BACKLOG: usize = 32;
const GPU_IDLE_WAIT_ESCALATION_MS: u64 = 100;

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
    pub window_embed_calls: u64,
    pub window_chunks: u64,
    pub window_files_touched: u64,
    pub avg_chunks_per_embed_call: f64,
    pub avg_files_per_embed_call: f64,
    pub embed_ms_per_chunk: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct VectorBatchControllerObservation {
    pub queue_pending: usize,
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
    last_window: VectorBatchControllerWindow,
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticPolicy {
    pub pause: bool,
    pub sleep: Duration,
    pub idle_sleep: Duration,
}

static VECTOR_BATCH_CONTROLLER: OnceLock<Mutex<VectorBatchController>> = OnceLock::new();

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
            observation.queue_pending >= VECTOR_BATCH_CONTROLLER_IDLE_BACKLOG_THRESHOLD;

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
        let target_ready_reserve = vector_ready_reserve_target(
            READY_RESERVE_FLOOR,
            observation.queue_pending,
            self.target_files_per_cycle,
            self.target_embed_batch_chunks,
            observation.metrics.ready_queue_depth_current as usize,
        );
        let ready_queue_starved = observation.metrics.ready_queue_depth_current == 0;
        let ready_queue_under_floor =
            (observation.metrics.ready_queue_depth_current as usize) < target_ready_reserve;
        let persist_congested = observation.metrics.persist_queue_depth_current > 0;
        let gpu_idle_wait_delta = observation
            .metrics
            .gpu_idle_wait_ms_total
            .saturating_sub(self.baseline_metrics.gpu_idle_wait_ms_total);
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
        let embed_regressed = self
            .last_best_embed_ms_per_chunk
            .is_some_and(|best| embed_ms_per_chunk > best * 1.20);

        if embed_regressed {
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
        } else if !observation.interactive_active && backlog_meaningful {
            let underfed_chunks =
                avg_chunks_per_embed_call < (self.target_embed_batch_chunks as f64 * 0.75);
            let underfed_files = underfed_chunks && avg_files_per_embed_call < 1.5;
            if ready_queue_under_floor
                || gpu_idle_wait_delta > GPU_IDLE_WAIT_ESCALATION_MS
                || underfed_chunks
                || underfed_files
            {
                reason = if ready_queue_under_floor
                    || gpu_idle_wait_delta > GPU_IDLE_WAIT_ESCALATION_MS
                {
                    "ready_queue_starved".to_string()
                } else {
                    "idle_underfed".to_string()
                };
                if underfed_chunks || ready_queue_under_floor {
                    desired_embed_chunks = desired_embed_chunks
                        .saturating_add(
                            VECTOR_BATCH_CONTROLLER_EMBED_STEP
                                * if ready_queue_under_floor { 2 } else { 1 },
                        )
                        .min(self.bounds.max_embed_batch_chunks);
                }
                if underfed_files || ready_queue_under_floor || ready_queue_starved {
                    desired_files_per_cycle = desired_files_per_cycle
                        .saturating_add(
                            VECTOR_BATCH_CONTROLLER_FILES_STEP
                                * if ready_queue_under_floor { 2 } else { 1 },
                        )
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
    let max_claim_target = target_files_per_cycle
        .saturating_mul(target_ready_depth.max(2))
        .clamp(min_claim_target, 256);
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
) -> usize {
    let mut reserve = configured_ready_depth.max(READY_RESERVE_FLOOR);
    if queue_pending >= 4_096 {
        reserve = reserve.max(READY_RESERVE_EXTREME_BACKLOG);
    } else if queue_pending >= 512 {
        reserve = reserve.max(READY_RESERVE_HEAVY_BACKLOG);
    }
    reserve = reserve.max(((target_embed_batch_chunks + 15) / 16).saturating_add(4));
    reserve = reserve.max((target_files_per_cycle / 2).clamp(4, READY_RESERVE_EXTREME_BACKLOG));
    if current_ready_depth == 0 && queue_pending >= AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD {
        reserve = reserve.max(READY_RESERVE_HEAVY_BACKLOG);
    }
    reserve.clamp(1, READY_RESERVE_EXTREME_BACKLOG)
}

pub fn current_vector_drain_state(
    file_backlog_depth: usize,
    service_pressure: ServicePressure,
    interactive_active: bool,
    provider_requested: &str,
    provider_effective: &str,
) -> VectorDrainState {
    if file_backlog_depth == 0 {
        return VectorDrainState::QuietCruise;
    }
    if interactive_active {
        return VectorDrainState::InteractiveGuarded;
    }
    if matches!(service_pressure, ServicePressure::Critical) {
        return VectorDrainState::Recovery;
    }
    if provider_requested.eq_ignore_ascii_case("cuda")
        && !provider_effective.eq_ignore_ascii_case("cuda")
    {
        return VectorDrainState::GpuScalingBlocked;
    }
    VectorDrainState::AggressiveDrain
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

pub fn vector_worker_admitted(
    worker_idx: usize,
    service_pressure: ServicePressure,
    gpu_available: bool,
    file_backlog_depth: usize,
) -> bool {
    let interactive_in_flight = service_guard::interactive_requests_in_flight();
    if interactive_in_flight > 0 {
        let allowed_interactive_workers = if gpu_available { 1 } else { 0 };
        if worker_idx >= allowed_interactive_workers {
            service_guard::record_vectorization_suppressed();
            return false;
        }
    } else if service_guard::interactive_priority_active() {
        service_guard::record_vectorization_suppressed();
        return false;
    }
    if !gpu_available {
        return true;
    }
    let allowed_gpu_workers = allowed_gpu_vector_workers(file_backlog_depth, service_pressure);
    if worker_idx >= allowed_gpu_workers {
        service_guard::record_vectorization_suppressed();
        return false;
    }
    true
}

pub fn semantic_policy(queue_len: usize, service_pressure: ServicePressure) -> SemanticPolicy {
    if service_guard::interactive_priority_active() {
        service_guard::record_vectorization_suppressed();
        return SemanticPolicy {
            pause: false,
            sleep: Duration::from_millis(750),
            idle_sleep: Duration::from_millis(1500),
        };
    }
    if queue_len == 0 {
        return SemanticPolicy {
            pause: false,
            sleep: Duration::from_millis(250),
            idle_sleep: Duration::from_secs(20),
        };
    }
    if service_pressure == ServicePressure::Critical {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(2),
            idle_sleep: Duration::from_secs(2),
        };
    }
    if service_guard::interactive_requests_in_flight() > 0 {
        return SemanticPolicy {
            pause: false,
            sleep: Duration::from_millis(500),
            idle_sleep: Duration::from_secs(1),
        };
    }
    if queue_len >= AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD {
        return SemanticPolicy {
            pause: false,
            sleep: Duration::from_millis(100),
            idle_sleep: Duration::from_millis(250),
        };
    }
    if matches!(
        service_pressure,
        ServicePressure::Degraded | ServicePressure::Recovering
    ) {
        return SemanticPolicy {
            pause: false,
            sleep: Duration::from_millis(350),
            idle_sleep: Duration::from_millis(750),
        };
    }
    SemanticPolicy {
        pause: false,
        sleep: Duration::from_millis(250),
        idle_sleep: Duration::from_millis(750),
    }
}

pub fn symbol_embedding_allowed(
    file_backlog_depth: usize,
    service_pressure: ServicePressure,
) -> bool {
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
    vector_backlog_depth: usize,
    gpu_available: bool,
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
    if !gpu_available && vector_backlog_depth >= CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD {
        service_guard::record_projection_suppressed();
        return false;
    }
    if gpu_available && vector_backlog_depth >= GPU_VECTOR_BACKLOG_GRAPH_YIELD_THRESHOLD {
        service_guard::record_projection_suppressed();
        return false;
    }
    true
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

    #[test]
    fn test_allowed_gpu_vector_workers_returns_4_on_high_backlog() {
        let allowed = allowed_gpu_vector_workers(2000, ServicePressure::Healthy);
        assert_eq!(allowed, 4);
    }
}
