use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::bridge::RuntimeTruthFeed;

static LAST_SQL_LATENCY_MS: AtomicU64 = AtomicU64::new(0);
static LAST_MCP_LATENCY_MS: AtomicU64 = AtomicU64::new(0);
static LAST_MCP_SAMPLE_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_SAMPLE_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_DEGRADED_AT_MS: AtomicU64 = AtomicU64::new(0);
static INTERACTIVE_REQUESTS_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static LAST_INTERACTIVE_AT_MS: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_LAUNCHES_SUPPRESSED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTORIZATION_SUPPRESSED_TOTAL: AtomicU64 = AtomicU64::new(0);
static PROJECTION_SUPPRESSED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTORIZATION_INTERRUPTED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTORIZATION_REQUEUED_FOR_INTERACTIVE_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTORIZATION_RESUMED_AFTER_INTERACTIVE_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_FETCH_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_DB_WRITE_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_COMPLETION_CHECK_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_MARK_DONE_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_BATCHES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_CHUNKS_EMBEDDED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_FILES_COMPLETED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_CALLS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_CLAIMED_WORK_ITEMS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PARTIAL_FILE_CYCLES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_MARK_DONE_CALLS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_FILES_TOUCHED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_DISPATCH_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_PREFETCH_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_FALLBACK_INLINE_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARED_WORK_ITEMS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_EMPTY_BATCHES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_IMMEDIATE_COMPLETED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_FAILED_FETCHES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_FINALIZE_ENQUEUED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_FINALIZE_FALLBACK_INLINE_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_REPLY_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_SEND_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_FINALIZE_SEND_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_QUEUE_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_FINALIZE_QUEUE_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_QUEUE_DEPTH_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_QUEUE_DEPTH_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_INFLIGHT_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_INFLIGHT_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_INFLIGHT_CHUNKS_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_INFLIGHT_CHUNKS_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_QUEUE_DEPTH_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_QUEUE_DEPTH_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_QUEUE_CHUNKS_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_QUEUE_CHUNKS_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_QUEUE_CHUNKS_SMALL: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_QUEUE_CHUNKS_MEDIUM: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_QUEUE_CHUNKS_LARGE: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_BATCHES_SMALL: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_BATCHES_MEDIUM: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_BATCHES_LARGE: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_BATCHES_MIXED: AtomicU64 = AtomicU64::new(0);
static VECTOR_HOMOGENEOUS_BATCHES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_MIXED_FALLBACK_BATCHES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_LAST_CONSUMED_BATCH_LANE_CODE: AtomicU64 = AtomicU64::new(0);
static VECTOR_ACTIVE_SMALL_MAX_TOKENS: AtomicU64 = AtomicU64::new(0);
static VECTOR_ACTIVE_MEDIUM_MAX_TOKENS: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_REPLENISHMENT_DEFICIT_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_REPLENISHMENT_DEFICIT_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_ACTIVE_CLAIMED_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_PREPARE_CLAIMED_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_CLAIMED_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_FINALIZE_QUEUE_DEPTH_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_FINALIZE_QUEUE_DEPTH_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_PERSIST_QUEUE_DEPTH_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_PERSIST_QUEUE_DEPTH_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_PERSIST_CLAIMED_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_PERSIST_SEND_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_PERSIST_QUEUE_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_GPU_IDLE_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_CANONICAL_BACKLOG_DEPTH_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_CANONICAL_BACKLOG_DEPTH_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_OLDEST_READY_BATCH_AGE_MS_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_OLDEST_READY_BATCH_AGE_MS_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_INPUT_TEXTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_INPUT_TEXT_BYTES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_CLONE_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_TRANSFORM_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_EXPORT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_ATTEMPTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_INFLIGHT_STARTED_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_INFLIGHT_TEXTS_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_INFLIGHT_TEXT_BYTES_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_LAST_EMBED_ATTEMPT_WALL_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_ATTEMPT_WALL_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_ATTEMPT_WALL_MS_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_LAST_EMBED_FINISHED_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_LAST_EMBED_GAP_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_GAP_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_GAP_SAMPLES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_EMBED_GAP_MS_MAX: AtomicU64 = AtomicU64::new(0);
static VECTOR_WORKERS_STARTED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_WORKERS_STOPPED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_WORKERS_ACTIVE_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_WORKER_HEARTBEAT_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_WORKER_RESTARTS_TOTAL: AtomicU64 = AtomicU64::new(0);
// REQ-AXO-270 AC1.5 — per-stage heartbeats for the 3-stage vector pipeline.
// Phase 1: only writers (skeleton stage stubs + Phase 2 producer/embedder/persister loops).
// Readers and snapshot exposure land with Phase 2 telemetry.
static VECTOR_PIPELINE_PRODUCER_HEARTBEAT_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_PIPELINE_EMBEDDER_HEARTBEAT_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_PIPELINE_PERSISTER_HEARTBEAT_AT_MS: AtomicU64 = AtomicU64::new(0);
static GRAPH_WORKERS_STARTED_TOTAL: AtomicU64 = AtomicU64::new(0);
static GRAPH_WORKERS_STOPPED_TOTAL: AtomicU64 = AtomicU64::new(0);
static GRAPH_WORKERS_ACTIVE_CURRENT: AtomicU64 = AtomicU64::new(0);
static GRAPH_WORKER_HEARTBEAT_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_LANE_STATE_CODE: AtomicU64 = AtomicU64::new(0);
static VECTOR_LANE_LAST_TRANSITION_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_LANE_LAST_SUCCESS_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_LANE_LAST_FAULT_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_STAGE_LATENCY_WINDOWS: OnceLock<Mutex<VectorStageLatencyWindows>> = OnceLock::new();
static VECTOR_EMBED_THROUGHPUT_SAMPLES: OnceLock<Mutex<VecDeque<(u64, u64)>>> = OnceLock::new();
static VECTOR_BACKLOG_SIGNAL: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();
static RUNTIME_WORK_SIGNAL: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();
static LAST_RUNTIME_WAKEUP_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_QUIESCENT_ENTERED_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_QUIESCENT_EXITED_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_QUIESCENT_EXIT_REASON_CODE: AtomicU64 = AtomicU64::new(0);
static QUIESCENT_EXIT_ACTIVE_BACKLOG_TOTAL: AtomicU64 = AtomicU64::new(0);
static QUIESCENT_EXIT_DRAINING_RESIDUAL_TOTAL: AtomicU64 = AtomicU64::new(0);
static QUIESCENT_EXIT_INTERACTIVE_GUARDED_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAST_RUNTIME_WAKE_SOURCE_CODE: AtomicU64 = AtomicU64::new(0);
static WAKE_SOURCE_BACKGROUND_TOTAL: AtomicU64 = AtomicU64::new(0);
static WAKE_SOURCE_SEMANTIC_VECTOR_TOTAL: AtomicU64 = AtomicU64::new(0);
static WAKE_SOURCE_GRAPH_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAST_BACKGROUND_WAKE_DETAIL_CODE: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_WAKE_MEMORY_RECLAIMER_TOTAL: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_WAKE_SHADOW_OPTIMIZER_TOTAL: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_WAKE_RUNTIME_TRACE_TOTAL: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_WAKE_READER_REFRESH_TOTAL: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_WAKE_AUTONOMOUS_INGESTOR_TOTAL: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_WAKE_INGRESS_PROMOTER_TOTAL: AtomicU64 = AtomicU64::new(0);
static BACKGROUND_WAKE_FEDERATION_ORCHESTRATOR_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAST_USEFUL_RESUME_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_USEFUL_RESUME_FOR_EXIT_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_OBSERVED_QUIESCENT_STATE_CODE: AtomicU64 = AtomicU64::new(0);
static RUNTIME_TRUTH_LAST_HEARTBEAT_AT_MS: AtomicU64 = AtomicU64::new(0);
static RUNTIME_TRUTH_LAST_GOOD_PAYLOAD_AT_MS: AtomicU64 = AtomicU64::new(0);
static RUNTIME_TRUTH_STALE_AFTER_MS: AtomicU64 =
    AtomicU64::new(RuntimeTruthFeed::DEFAULT_STALE_AFTER_MS);
static RUNTIME_TRUTH_DEGRADED_REASON: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static VECTOR_LAST_WORKER_ADMISSION_REASON: OnceLock<Mutex<String>> = OnceLock::new();
static VECTOR_LAST_ALLOWED_GPU_WORKERS: AtomicU64 = AtomicU64::new(0);
static RUNTIME_WAKE_TIMESTAMPS: OnceLock<Mutex<VecDeque<u64>>> = OnceLock::new();
static QUIESCENT_RESUME_LATENCIES_MS: OnceLock<Mutex<VecDeque<u64>>> = OnceLock::new();
static QUIESCENT_USEFUL_RESUME_LATENCIES_MS: OnceLock<Mutex<VecDeque<u64>>> = OnceLock::new();

const SERVICE_SAMPLE_TTL_MS: u64 = 5_000;
const SERVICE_RECOVERY_WINDOW_MS: u64 = 15_000;
#[allow(dead_code)]
const INTERACTIVE_PRIORITY_IDLE_MS: u64 = 2_500;
const VECTOR_STAGE_WINDOW_CAPACITY: usize = 256;
const VECTOR_EMBED_THROUGHPUT_WINDOW_MS: u64 = 5_000;
const VECTOR_EMBED_THROUGHPUT_HISTORY_MS: u64 = 60_000;
const VECTOR_EMBED_THROUGHPUT_CAPACITY: usize = 256;

#[derive(Clone, Copy)]
pub enum ServiceKind {
    Sql,
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServicePressure {
    Healthy,
    Recovering,
    Degraded,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractivePriority {
    BackgroundNormal,
    InteractivePriority,
    InteractiveCritical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeQuiescentState {
    ActiveBacklog,
    DrainingResidualWork,
    InteractiveGuarded,
    QuiescentCandidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeWakeSummary {
    pub wakeups_last_60s: u64,
    pub last_wakeup_at_ms: u64,
    pub quiescent_entered_at_ms: u64,
    pub last_quiescent_exited_at_ms: u64,
    pub quiescent_dwell_ms_current: u64,
    pub resume_latency_samples: u64,
    pub resume_latency_p50_ms: u64,
    pub resume_latency_p95_ms: u64,
    pub resume_latency_max_ms: u64,
    pub useful_resume_latency_samples: u64,
    pub useful_resume_latency_p50_ms: u64,
    pub useful_resume_latency_p95_ms: u64,
    pub useful_resume_latency_max_ms: u64,
    pub last_useful_resume_at_ms: u64,
    pub last_quiescent_exit_reason: &'static str,
    pub exit_due_to_active_backlog_total: u64,
    pub exit_due_to_draining_residual_total: u64,
    pub exit_due_to_interactive_guarded_total: u64,
    pub last_wake_source: &'static str,
    pub wake_source_background_total: u64,
    pub wake_source_semantic_vector_total: u64,
    pub wake_source_graph_total: u64,
    pub last_background_wake_detail: &'static str,
    pub dominant_background_wake_detail: &'static str,
    pub background_wake_memory_reclaimer_total: u64,
    pub background_wake_shadow_optimizer_total: u64,
    pub background_wake_runtime_trace_total: u64,
    pub background_wake_reader_refresh_total: u64,
    pub background_wake_autonomous_ingestor_total: u64,
    pub background_wake_ingress_promoter_total: u64,
    pub background_wake_federation_orchestrator_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeWakeSource {
    Background,
    SemanticVector,
    Graph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundWakeDetail {
    Unknown,
    MemoryReclaimer,
    ShadowOptimizer,
    RuntimeTrace,
    ReaderRefresh,
    AutonomousIngestor,
    IngressPromoter,
    FederationOrchestrator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorStageKind {
    Fetch,
    Embed,
    DbWrite,
    CompletionCheck,
    MarkDone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VectorLaneState {
    #[default]
    Starting,
    Healthy,
    Hold,
    Degraded,
    Unhealthy,
}

impl VectorLaneState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Healthy => "healthy",
            Self::Hold => "hold",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
        }
    }

    fn code(self) -> u64 {
        match self {
            Self::Starting => 0,
            Self::Healthy => 1,
            Self::Hold => 2,
            Self::Degraded => 3,
            Self::Unhealthy => 4,
        }
    }

    fn from_code(code: u64) -> Self {
        match code {
            1 => Self::Healthy,
            2 => Self::Hold,
            3 => Self::Degraded,
            4 => Self::Unhealthy,
            _ => Self::Starting,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VectorBatchLaneKind {
    #[default]
    Unknown,
    Small,
    Medium,
    Large,
    Mixed,
}

impl VectorBatchLaneKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
            Self::Mixed => "mixed",
        }
    }

    fn code(self) -> u64 {
        match self {
            Self::Unknown => 0,
            Self::Small => 1,
            Self::Medium => 2,
            Self::Large => 3,
            Self::Mixed => 4,
        }
    }

    fn from_code(code: u64) -> Self {
        match code {
            1 => Self::Small,
            2 => Self::Medium,
            3 => Self::Large,
            4 => Self::Mixed,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VectorRuntimeMetrics {
    pub fetch_ms_total: u64,
    pub embed_ms_total: u64,
    pub db_write_ms_total: u64,
    pub completion_check_ms_total: u64,
    pub mark_done_ms_total: u64,
    pub batches_total: u64,
    pub chunks_embedded_total: u64,
    pub files_completed_total: u64,
    pub embed_calls_total: u64,
    pub claimed_work_items_total: u64,
    pub partial_file_cycles_total: u64,
    pub mark_done_calls_total: u64,
    pub files_touched_total: u64,
    pub prepare_dispatch_total: u64,
    pub prepare_prefetch_total: u64,
    pub prepare_fallback_inline_total: u64,
    pub prepared_work_items_total: u64,
    pub prepare_empty_batches_total: u64,
    pub prepare_immediate_completed_total: u64,
    pub prepare_failed_fetches_total: u64,
    pub finalize_enqueued_total: u64,
    pub finalize_fallback_inline_total: u64,
    pub prepare_reply_wait_ms_total: u64,
    pub prepare_send_wait_ms_total: u64,
    pub finalize_send_wait_ms_total: u64,
    pub prepare_queue_wait_ms_total: u64,
    pub finalize_queue_wait_ms_total: u64,
    pub prepare_queue_depth_current: u64,
    pub prepare_queue_depth_max: u64,
    pub prepare_inflight_current: u64,
    pub prepare_inflight_max: u64,
    pub prepare_inflight_chunks_current: u64,
    pub prepare_inflight_chunks_max: u64,
    pub ready_queue_depth_current: u64,
    pub ready_queue_depth_max: u64,
    pub ready_queue_chunks_current: u64,
    pub ready_queue_chunks_max: u64,
    pub ready_queue_chunks_small: u64,
    pub ready_queue_chunks_medium: u64,
    pub ready_queue_chunks_large: u64,
    pub ready_batches_small: u64,
    pub ready_batches_medium: u64,
    pub ready_batches_large: u64,
    pub ready_batches_mixed: u64,
    pub homogeneous_batches_total: u64,
    pub mixed_fallback_batches_total: u64,
    pub last_consumed_batch_lane: VectorBatchLaneKind,
    pub active_small_max_tokens: u64,
    pub active_medium_max_tokens: u64,
    pub ready_replenishment_deficit_current: u64,
    pub ready_replenishment_deficit_max: u64,
    pub active_claimed_current: u64,
    pub prepare_claimed_current: u64,
    pub ready_claimed_current: u64,
    pub finalize_queue_depth_current: u64,
    pub finalize_queue_depth_max: u64,
    pub persist_queue_depth_current: u64,
    pub persist_queue_depth_max: u64,
    pub persist_claimed_current: u64,
    pub persist_send_wait_ms_total: u64,
    pub persist_queue_wait_ms_total: u64,
    pub gpu_idle_wait_ms_total: u64,
    pub canonical_backlog_depth_current: u64,
    pub canonical_backlog_depth_max: u64,
    pub oldest_ready_batch_age_ms_current: u64,
    pub oldest_ready_batch_age_ms_max: u64,
    pub embed_input_texts_total: u64,
    pub embed_input_text_bytes_total: u64,
    pub embed_clone_ms_total: u64,
    pub embed_transform_ms_total: u64,
    pub embed_export_ms_total: u64,
    pub embed_attempts_total: u64,
    pub embed_inflight_started_at_ms: u64,
    pub embed_inflight_texts_current: u64,
    pub embed_inflight_text_bytes_current: u64,
    pub last_embed_attempt_wall_ms: u64,
    pub avg_embed_attempt_wall_ms: f64,
    pub max_embed_attempt_wall_ms: u64,
    pub last_embed_gap_ms: u64,
    pub avg_embed_gap_ms: f64,
    pub max_embed_gap_ms: u64,
    pub vector_workers_started_total: u64,
    pub vector_workers_stopped_total: u64,
    pub vector_workers_active_current: u64,
    pub vector_worker_heartbeat_at_ms: u64,
    pub vector_worker_restarts_total: u64,
    pub graph_workers_started_total: u64,
    pub graph_workers_stopped_total: u64,
    pub graph_workers_active_current: u64,
    pub graph_worker_heartbeat_at_ms: u64,
    pub vector_lane_state: VectorLaneState,
    pub vector_lane_last_transition_at_ms: u64,
    pub vector_lane_last_success_at_ms: u64,
    pub vector_lane_last_fault_at_ms: u64,
    pub chunk_embeddings_per_second: f64,
    pub chunk_embeddings_rate_window_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorStageLatencySummary {
    pub samples: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorRuntimeLatencySummaries {
    pub fetch: VectorStageLatencySummary,
    pub embed: VectorStageLatencySummary,
    pub db_write: VectorStageLatencySummary,
    pub completion_check: VectorStageLatencySummary,
    pub mark_done: VectorStageLatencySummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VectorPipelineStageTelemetry {
    pub prepare_ms_total: u64,
    pub ready_wait_ms_total: u64,
    pub inference_ms_total: u64,
    pub output_extract_ms_total: u64,
    pub persist_ms_total: u64,
    pub finalize_ms_total: u64,
}

#[derive(Debug, Default)]
struct VectorStageLatencyWindows {
    fetch: VecDeque<u64>,
    embed: VecDeque<u64>,
    db_write: VecDeque<u64>,
    completion_check: VecDeque<u64>,
    mark_done: VecDeque<u64>,
}

impl InteractivePriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BackgroundNormal => "background_normal",
            Self::InteractivePriority => "interactive_priority",
            Self::InteractiveCritical => "interactive_critical",
        }
    }
}

impl RuntimeQuiescentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActiveBacklog => "active_backlog",
            Self::DrainingResidualWork => "draining_residual_work",
            Self::InteractiveGuarded => "interactive_guarded",
            Self::QuiescentCandidate => "quiescent_candidate",
        }
    }

    fn code(self) -> u64 {
        match self {
            Self::ActiveBacklog => 0,
            Self::DrainingResidualWork => 1,
            Self::InteractiveGuarded => 2,
            Self::QuiescentCandidate => 3,
        }
    }

    fn from_code(code: u64) -> Self {
        match code {
            1 => Self::DrainingResidualWork,
            2 => Self::InteractiveGuarded,
            3 => Self::QuiescentCandidate,
            _ => Self::ActiveBacklog,
        }
    }
}

impl RuntimeWakeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::SemanticVector => "semantic_vector",
            Self::Graph => "graph",
        }
    }

    fn code(self) -> u64 {
        match self {
            Self::Background => 0,
            Self::SemanticVector => 1,
            Self::Graph => 2,
        }
    }

    fn from_code(code: u64) -> Self {
        match code {
            1 => Self::SemanticVector,
            2 => Self::Graph,
            _ => Self::Background,
        }
    }
}

impl BackgroundWakeDetail {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::MemoryReclaimer => "memory_reclaimer",
            Self::ShadowOptimizer => "shadow_optimizer",
            Self::RuntimeTrace => "runtime_trace",
            Self::ReaderRefresh => "reader_refresh",
            Self::AutonomousIngestor => "autonomous_ingestor",
            Self::IngressPromoter => "ingress_promoter",
            Self::FederationOrchestrator => "federation_orchestrator",
        }
    }

    fn code(self) -> u64 {
        match self {
            Self::Unknown => 0,
            Self::MemoryReclaimer => 1,
            Self::ShadowOptimizer => 2,
            Self::RuntimeTrace => 3,
            Self::ReaderRefresh => 4,
            Self::AutonomousIngestor => 5,
            Self::IngressPromoter => 6,
            Self::FederationOrchestrator => 7,
        }
    }

    fn from_code(code: u64) -> Self {
        match code {
            1 => Self::MemoryReclaimer,
            2 => Self::ShadowOptimizer,
            3 => Self::RuntimeTrace,
            4 => Self::ReaderRefresh,
            5 => Self::AutonomousIngestor,
            6 => Self::IngressPromoter,
            7 => Self::FederationOrchestrator,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRequestClass {
    Control,
    Observer,
}

pub fn record_latency(kind: ServiceKind, latency_ms: u64) {
    let now = now_ms();
    match kind {
        ServiceKind::Sql => LAST_SQL_LATENCY_MS.store(latency_ms, Ordering::Relaxed),
        ServiceKind::Mcp => {
            LAST_MCP_LATENCY_MS.store(latency_ms, Ordering::Relaxed);
            LAST_MCP_SAMPLE_AT_MS.store(now, Ordering::Relaxed);
        }
    }
    if latency_ms >= 500 {
        LAST_DEGRADED_AT_MS.store(now, Ordering::Relaxed);
    }
    LAST_SAMPLE_AT_MS.store(now, Ordering::Relaxed);
}

pub fn recent_peak_latency_ms() -> u64 {
    recent_peak_latency_ms_at(now_ms())
}

pub fn recent_mcp_latency_ms() -> u64 {
    recent_mcp_latency_ms_at(now_ms())
}

pub fn current_pressure() -> ServicePressure {
    current_pressure_at(now_ms())
}

pub fn mcp_request_started_with_class(class: McpRequestClass) {
    let now = now_ms();
    if matches!(class, McpRequestClass::Control) {
        INTERACTIVE_REQUESTS_IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    }
    LAST_INTERACTIVE_AT_MS.store(now, Ordering::Relaxed);
}

pub fn mcp_request_started() {
    mcp_request_started_with_class(McpRequestClass::Control);
}

pub fn mcp_request_finished_with_class(class: McpRequestClass) {
    let now = now_ms();
    if matches!(class, McpRequestClass::Control) {
        let _ = INTERACTIVE_REQUESTS_IN_FLIGHT.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(current.saturating_sub(1)),
        );
    }
    LAST_INTERACTIVE_AT_MS.store(now, Ordering::Relaxed);
}

pub fn mcp_request_finished() {
    mcp_request_finished_with_class(McpRequestClass::Control);
}

pub fn interactive_requests_in_flight() -> u64 {
    INTERACTIVE_REQUESTS_IN_FLIGHT.load(Ordering::Relaxed)
}

pub fn interactive_priority_active() -> bool {
    current_interactive_priority() != InteractivePriority::BackgroundNormal
}

pub fn current_interactive_priority() -> InteractivePriority {
    current_interactive_priority_at(now_ms())
}

pub fn current_runtime_quiescent_state(
    graph_backlog_depth: u64,
    semantic_backlog_depth: u64,
) -> RuntimeQuiescentState {
    if interactive_requests_in_flight() > 0 {
        return RuntimeQuiescentState::InteractiveGuarded;
    }

    let metrics = vector_runtime_metrics();
    if graph_backlog_depth > 0 || semantic_backlog_depth > 0 {
        return RuntimeQuiescentState::ActiveBacklog;
    }

    if metrics.ready_queue_chunks_current > 0
        || metrics.prepare_inflight_chunks_current > 0
        || metrics.ready_replenishment_deficit_current > 0
        || metrics.prepare_queue_depth_current > 0
        || metrics.persist_queue_depth_current > 0
        || metrics.active_claimed_current > 0
        || metrics.persist_claimed_current > 0
    {
        return RuntimeQuiescentState::DrainingResidualWork;
    }

    RuntimeQuiescentState::QuiescentCandidate
}

pub fn record_runtime_wakeup(
    source: RuntimeWakeSource,
    graph_backlog_depth: u64,
    semantic_backlog_depth: u64,
) {
    let now = now_ms();
    LAST_RUNTIME_WAKEUP_AT_MS.store(now, Ordering::Relaxed);
    LAST_RUNTIME_WAKE_SOURCE_CODE.store(source.code(), Ordering::Relaxed);
    match source {
        RuntimeWakeSource::Background => {
            WAKE_SOURCE_BACKGROUND_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        RuntimeWakeSource::SemanticVector => {
            WAKE_SOURCE_SEMANTIC_VECTOR_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        RuntimeWakeSource::Graph => {
            WAKE_SOURCE_GRAPH_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
    }
    {
        let mut wakes = runtime_wake_timestamps()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        wakes.push_back(now);
        let cutoff = now.saturating_sub(60_000);
        while wakes.front().copied().is_some_and(|ts| ts < cutoff) {
            wakes.pop_front();
        }
    }
    observe_quiescent_transition(current_runtime_quiescent_state(
        graph_backlog_depth,
        semantic_backlog_depth,
    ));
}

pub fn record_background_runtime_wakeup(
    detail: BackgroundWakeDetail,
    graph_backlog_depth: u64,
    semantic_backlog_depth: u64,
) {
    LAST_BACKGROUND_WAKE_DETAIL_CODE.store(detail.code(), Ordering::Relaxed);
    match detail {
        BackgroundWakeDetail::Unknown => {}
        BackgroundWakeDetail::MemoryReclaimer => {
            BACKGROUND_WAKE_MEMORY_RECLAIMER_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        BackgroundWakeDetail::ShadowOptimizer => {
            BACKGROUND_WAKE_SHADOW_OPTIMIZER_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        BackgroundWakeDetail::RuntimeTrace => {
            BACKGROUND_WAKE_RUNTIME_TRACE_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        BackgroundWakeDetail::ReaderRefresh => {
            BACKGROUND_WAKE_READER_REFRESH_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        BackgroundWakeDetail::AutonomousIngestor => {
            BACKGROUND_WAKE_AUTONOMOUS_INGESTOR_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        BackgroundWakeDetail::IngressPromoter => {
            BACKGROUND_WAKE_INGRESS_PROMOTER_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        BackgroundWakeDetail::FederationOrchestrator => {
            BACKGROUND_WAKE_FEDERATION_ORCHESTRATOR_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
    }
    record_runtime_wakeup(
        RuntimeWakeSource::Background,
        graph_backlog_depth,
        semantic_backlog_depth,
    );
}

pub fn runtime_wake_summary(
    graph_backlog_depth: u64,
    semantic_backlog_depth: u64,
) -> RuntimeWakeSummary {
    let now = now_ms();
    let state = current_runtime_quiescent_state(graph_backlog_depth, semantic_backlog_depth);
    let resume_latency = summarize_runtime_window(&quiescent_resume_latencies_ms());
    let useful_resume_latency = summarize_runtime_window(&quiescent_useful_resume_latencies_ms());
    let wakeups_last_60s = {
        let mut wakes = runtime_wake_timestamps()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let cutoff = now.saturating_sub(60_000);
        while wakes.front().copied().is_some_and(|ts| ts < cutoff) {
            wakes.pop_front();
        }
        wakes.len() as u64
    };
    let quiescent_entered_at_ms = LAST_QUIESCENT_ENTERED_AT_MS.load(Ordering::Relaxed);
    let background_totals = [
        (
            BackgroundWakeDetail::MemoryReclaimer,
            BACKGROUND_WAKE_MEMORY_RECLAIMER_TOTAL.load(Ordering::Relaxed),
        ),
        (
            BackgroundWakeDetail::ShadowOptimizer,
            BACKGROUND_WAKE_SHADOW_OPTIMIZER_TOTAL.load(Ordering::Relaxed),
        ),
        (
            BackgroundWakeDetail::RuntimeTrace,
            BACKGROUND_WAKE_RUNTIME_TRACE_TOTAL.load(Ordering::Relaxed),
        ),
        (
            BackgroundWakeDetail::ReaderRefresh,
            BACKGROUND_WAKE_READER_REFRESH_TOTAL.load(Ordering::Relaxed),
        ),
        (
            BackgroundWakeDetail::AutonomousIngestor,
            BACKGROUND_WAKE_AUTONOMOUS_INGESTOR_TOTAL.load(Ordering::Relaxed),
        ),
        (
            BackgroundWakeDetail::IngressPromoter,
            BACKGROUND_WAKE_INGRESS_PROMOTER_TOTAL.load(Ordering::Relaxed),
        ),
        (
            BackgroundWakeDetail::FederationOrchestrator,
            BACKGROUND_WAKE_FEDERATION_ORCHESTRATOR_TOTAL.load(Ordering::Relaxed),
        ),
    ];
    let dominant_background_wake_detail = background_totals
        .into_iter()
        .max_by_key(|(_, total)| *total)
        .filter(|(_, total)| *total > 0)
        .map(|(detail, _)| detail.as_str())
        .unwrap_or("unknown");
    RuntimeWakeSummary {
        wakeups_last_60s,
        last_wakeup_at_ms: LAST_RUNTIME_WAKEUP_AT_MS.load(Ordering::Relaxed),
        quiescent_entered_at_ms,
        last_quiescent_exited_at_ms: LAST_QUIESCENT_EXITED_AT_MS.load(Ordering::Relaxed),
        quiescent_dwell_ms_current: if state == RuntimeQuiescentState::QuiescentCandidate
            && quiescent_entered_at_ms > 0
        {
            now.saturating_sub(quiescent_entered_at_ms)
        } else {
            0
        },
        resume_latency_samples: resume_latency.samples,
        resume_latency_p50_ms: resume_latency.p50_ms,
        resume_latency_p95_ms: resume_latency.p95_ms,
        resume_latency_max_ms: resume_latency.max_ms,
        useful_resume_latency_samples: useful_resume_latency.samples,
        useful_resume_latency_p50_ms: useful_resume_latency.p50_ms,
        useful_resume_latency_p95_ms: useful_resume_latency.p95_ms,
        useful_resume_latency_max_ms: useful_resume_latency.max_ms,
        last_useful_resume_at_ms: LAST_USEFUL_RESUME_AT_MS.load(Ordering::Relaxed),
        last_quiescent_exit_reason: RuntimeQuiescentState::from_code(
            LAST_QUIESCENT_EXIT_REASON_CODE.load(Ordering::Relaxed),
        )
        .as_str(),
        exit_due_to_active_backlog_total: QUIESCENT_EXIT_ACTIVE_BACKLOG_TOTAL
            .load(Ordering::Relaxed),
        exit_due_to_draining_residual_total: QUIESCENT_EXIT_DRAINING_RESIDUAL_TOTAL
            .load(Ordering::Relaxed),
        exit_due_to_interactive_guarded_total: QUIESCENT_EXIT_INTERACTIVE_GUARDED_TOTAL
            .load(Ordering::Relaxed),
        last_wake_source: RuntimeWakeSource::from_code(
            LAST_RUNTIME_WAKE_SOURCE_CODE.load(Ordering::Relaxed),
        )
        .as_str(),
        wake_source_background_total: WAKE_SOURCE_BACKGROUND_TOTAL.load(Ordering::Relaxed),
        wake_source_semantic_vector_total: WAKE_SOURCE_SEMANTIC_VECTOR_TOTAL
            .load(Ordering::Relaxed),
        wake_source_graph_total: WAKE_SOURCE_GRAPH_TOTAL.load(Ordering::Relaxed),
        last_background_wake_detail: BackgroundWakeDetail::from_code(
            LAST_BACKGROUND_WAKE_DETAIL_CODE.load(Ordering::Relaxed),
        )
        .as_str(),
        dominant_background_wake_detail,
        background_wake_memory_reclaimer_total: BACKGROUND_WAKE_MEMORY_RECLAIMER_TOTAL
            .load(Ordering::Relaxed),
        background_wake_shadow_optimizer_total: BACKGROUND_WAKE_SHADOW_OPTIMIZER_TOTAL
            .load(Ordering::Relaxed),
        background_wake_runtime_trace_total: BACKGROUND_WAKE_RUNTIME_TRACE_TOTAL
            .load(Ordering::Relaxed),
        background_wake_reader_refresh_total: BACKGROUND_WAKE_READER_REFRESH_TOTAL
            .load(Ordering::Relaxed),
        background_wake_autonomous_ingestor_total: BACKGROUND_WAKE_AUTONOMOUS_INGESTOR_TOTAL
            .load(Ordering::Relaxed),
        background_wake_ingress_promoter_total: BACKGROUND_WAKE_INGRESS_PROMOTER_TOTAL
            .load(Ordering::Relaxed),
        background_wake_federation_orchestrator_total:
            BACKGROUND_WAKE_FEDERATION_ORCHESTRATOR_TOTAL.load(Ordering::Relaxed),
    }
}

pub fn scale_interval_for_quiescent(
    base_ms: u64,
    state: RuntimeQuiescentState,
    scale_pct: usize,
    min_ms: u64,
    max_ms: u64,
) -> u64 {
    if state != RuntimeQuiescentState::QuiescentCandidate {
        return base_ms.clamp(min_ms, max_ms);
    }

    ((base_ms as u128)
        .saturating_mul(scale_pct as u128)
        .saturating_div(100) as u64)
        .clamp(min_ms, max_ms)
}

pub fn record_background_launch_suppressed() {
    BACKGROUND_LAUNCHES_SUPPRESSED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_vectorization_suppressed() {
    VECTORIZATION_SUPPRESSED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_projection_suppressed() {
    PROJECTION_SUPPRESSED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_vectorization_interrupted(count: u64) {
    VECTORIZATION_INTERRUPTED_TOTAL.fetch_add(count, Ordering::Relaxed);
}

pub fn record_vectorization_requeued_for_interactive(count: u64) {
    VECTORIZATION_REQUEUED_FOR_INTERACTIVE_TOTAL.fetch_add(count, Ordering::Relaxed);
}

pub fn record_vectorization_resumed_after_interactive(count: u64) {
    VECTORIZATION_RESUMED_AFTER_INTERACTIVE_TOTAL.fetch_add(count, Ordering::Relaxed);
}

pub fn record_vector_stage_ms(stage: VectorStageKind, latency_ms: u64) {
    let counter = match stage {
        VectorStageKind::Fetch => &VECTOR_FETCH_MS_TOTAL,
        VectorStageKind::Embed => &VECTOR_EMBED_MS_TOTAL,
        VectorStageKind::DbWrite => &VECTOR_DB_WRITE_MS_TOTAL,
        VectorStageKind::CompletionCheck => &VECTOR_COMPLETION_CHECK_MS_TOTAL,
        VectorStageKind::MarkDone => &VECTOR_MARK_DONE_MS_TOTAL,
    };
    counter.fetch_add(latency_ms, Ordering::Relaxed);
    let mut windows = vector_stage_latency_windows()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let window = match stage {
        VectorStageKind::Fetch => &mut windows.fetch,
        VectorStageKind::Embed => &mut windows.embed,
        VectorStageKind::DbWrite => &mut windows.db_write,
        VectorStageKind::CompletionCheck => &mut windows.completion_check,
        VectorStageKind::MarkDone => &mut windows.mark_done,
    };
    if window.len() >= VECTOR_STAGE_WINDOW_CAPACITY {
        window.pop_front();
    }
    window.push_back(latency_ms);
}

pub fn record_vector_embed_call(chunks: u64, files_touched: u64) {
    VECTOR_EMBED_CALLS_TOTAL.fetch_add(1, Ordering::Relaxed);
    VECTOR_BATCHES_TOTAL.fetch_add(1, Ordering::Relaxed);
    VECTOR_CHUNKS_EMBEDDED_TOTAL.fetch_add(chunks, Ordering::Relaxed);
    VECTOR_FILES_TOUCHED_TOTAL.fetch_add(files_touched, Ordering::Relaxed);
    if chunks > 0 {
        record_vector_embed_throughput_sample(now_ms(), chunks);
    }
}

pub fn record_vector_embed_inputs(texts: u64, text_bytes: u64, clone_ms: u64) {
    VECTOR_EMBED_INPUT_TEXTS_TOTAL.fetch_add(texts, Ordering::Relaxed);
    VECTOR_EMBED_INPUT_TEXT_BYTES_TOTAL.fetch_add(text_bytes, Ordering::Relaxed);
    VECTOR_EMBED_CLONE_MS_TOTAL.fetch_add(clone_ms, Ordering::Relaxed);
}

pub fn record_vector_embed_breakdown(transform_ms: u64, export_ms: u64) {
    VECTOR_EMBED_TRANSFORM_MS_TOTAL.fetch_add(transform_ms, Ordering::Relaxed);
    VECTOR_EMBED_EXPORT_MS_TOTAL.fetch_add(export_ms, Ordering::Relaxed);
}

pub fn record_vector_embed_attempt(texts: u64, text_bytes: u64) {
    let now = now_ms();
    VECTOR_EMBED_ATTEMPTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let last_finished_at = VECTOR_LAST_EMBED_FINISHED_AT_MS.load(Ordering::Relaxed);
    if last_finished_at > 0 && now >= last_finished_at {
        let gap_ms = now.saturating_sub(last_finished_at);
        VECTOR_LAST_EMBED_GAP_MS.store(gap_ms, Ordering::Relaxed);
        VECTOR_EMBED_GAP_MS_TOTAL.fetch_add(gap_ms, Ordering::Relaxed);
        VECTOR_EMBED_GAP_SAMPLES_TOTAL.fetch_add(1, Ordering::Relaxed);
        update_atomic_max(&VECTOR_EMBED_GAP_MS_MAX, gap_ms);
    }
    VECTOR_EMBED_INFLIGHT_STARTED_AT_MS.store(now, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXTS_CURRENT.store(texts, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXT_BYTES_CURRENT.store(text_bytes, Ordering::Relaxed);
}

pub fn record_vector_embed_attempt_finished() {
    let finished_at = now_ms();
    let started_at = VECTOR_EMBED_INFLIGHT_STARTED_AT_MS.load(Ordering::Relaxed);
    if started_at > 0 && finished_at >= started_at {
        let wall_ms = finished_at.saturating_sub(started_at);
        VECTOR_LAST_EMBED_ATTEMPT_WALL_MS.store(wall_ms, Ordering::Relaxed);
        VECTOR_EMBED_ATTEMPT_WALL_MS_TOTAL.fetch_add(wall_ms, Ordering::Relaxed);
        update_atomic_max(&VECTOR_EMBED_ATTEMPT_WALL_MS_MAX, wall_ms);
    }
    VECTOR_LAST_EMBED_FINISHED_AT_MS.store(finished_at, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_STARTED_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXTS_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXT_BYTES_CURRENT.store(0, Ordering::Relaxed);
}

pub fn current_last_embed_finished_at_ms() -> u64 {
    VECTOR_LAST_EMBED_FINISHED_AT_MS.load(Ordering::Relaxed)
}

pub fn record_vector_worker_started() {
    VECTOR_WORKERS_STARTED_TOTAL.fetch_add(1, Ordering::Relaxed);
    VECTOR_WORKERS_ACTIVE_CURRENT.fetch_add(1, Ordering::Relaxed);
    VECTOR_WORKER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
    record_vector_lane_state(VectorLaneState::Starting);
}

pub fn record_vector_worker_stopped() {
    VECTOR_WORKERS_STOPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
    let _ = VECTOR_WORKERS_ACTIVE_CURRENT.fetch_update(
        Ordering::Relaxed,
        Ordering::Relaxed,
        |current| Some(current.saturating_sub(1)),
    );
    VECTOR_WORKER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
}

pub fn record_vector_worker_heartbeat() {
    VECTOR_WORKER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
}

pub fn record_vector_worker_restart() {
    VECTOR_WORKER_RESTARTS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

// REQ-AXO-270 AC1.5 — per-stage heartbeat writers for the 3-stage pipeline.
pub fn record_vector_pipeline_producer_heartbeat() {
    VECTOR_PIPELINE_PRODUCER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
}

pub fn record_vector_pipeline_embedder_heartbeat() {
    VECTOR_PIPELINE_EMBEDDER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
}

pub fn record_vector_pipeline_persister_heartbeat() {
    VECTOR_PIPELINE_PERSISTER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
}

// Read accessors — Phase 1 only used by tests; Phase 2 wires them into
// the runtime snapshot surface for axon_debug / status views.
#[allow(dead_code)]
pub fn vector_pipeline_producer_heartbeat_at_ms() -> u64 {
    VECTOR_PIPELINE_PRODUCER_HEARTBEAT_AT_MS.load(Ordering::Relaxed)
}

#[allow(dead_code)]
pub fn vector_pipeline_embedder_heartbeat_at_ms() -> u64 {
    VECTOR_PIPELINE_EMBEDDER_HEARTBEAT_AT_MS.load(Ordering::Relaxed)
}

#[allow(dead_code)]
pub fn vector_pipeline_persister_heartbeat_at_ms() -> u64 {
    VECTOR_PIPELINE_PERSISTER_HEARTBEAT_AT_MS.load(Ordering::Relaxed)
}

pub fn record_graph_worker_started() {
    GRAPH_WORKERS_STARTED_TOTAL.fetch_add(1, Ordering::Relaxed);
    GRAPH_WORKERS_ACTIVE_CURRENT.fetch_add(1, Ordering::Relaxed);
    GRAPH_WORKER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
}

pub fn record_graph_worker_stopped() {
    GRAPH_WORKERS_STOPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
    let _ = GRAPH_WORKERS_ACTIVE_CURRENT.fetch_update(
        Ordering::Relaxed,
        Ordering::Relaxed,
        |current| Some(current.saturating_sub(1)),
    );
    GRAPH_WORKER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
}

pub fn record_graph_worker_heartbeat() {
    GRAPH_WORKER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
}

pub fn record_vector_lane_state(state: VectorLaneState) {
    VECTOR_LANE_STATE_CODE.store(state.code(), Ordering::Relaxed);
    VECTOR_LANE_LAST_TRANSITION_AT_MS.store(now_ms(), Ordering::Relaxed);
}

pub fn record_vector_lane_success() {
    let now = now_ms();
    VECTOR_LANE_LAST_SUCCESS_AT_MS.store(now, Ordering::Relaxed);
    VECTOR_LANE_STATE_CODE.store(VectorLaneState::Healthy.code(), Ordering::Relaxed);
    VECTOR_LANE_LAST_TRANSITION_AT_MS.store(now, Ordering::Relaxed);
}

pub fn record_vector_lane_fault() {
    let now = now_ms();
    VECTOR_LANE_LAST_FAULT_AT_MS.store(now, Ordering::Relaxed);
    VECTOR_LANE_STATE_CODE.store(VectorLaneState::Degraded.code(), Ordering::Relaxed);
    VECTOR_LANE_LAST_TRANSITION_AT_MS.store(now, Ordering::Relaxed);
}

pub fn notify_vector_backlog_activity() {
    let (lock, cvar) = VECTOR_BACKLOG_SIGNAL.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let mut generation = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    *generation = generation.saturating_add(1);
    cvar.notify_all();
}

pub fn wait_for_vector_backlog_signal(timeout: Duration) -> bool {
    let (lock, cvar) = VECTOR_BACKLOG_SIGNAL.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let generation = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    let observed = *generation;
    let result = cvar.wait_timeout_while(generation, timeout, |current| *current == observed);
    let (guard, _) = result.unwrap_or_else(|poison| poison.into_inner());
    *guard != observed
}

pub fn notify_runtime_work_activity() {
    let (lock, cvar) = RUNTIME_WORK_SIGNAL.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let mut generation = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    *generation = generation.saturating_add(1);
    cvar.notify_all();
}

pub fn wait_for_runtime_work_activity_or_timeout(timeout: Duration) -> bool {
    let (lock, cvar) = RUNTIME_WORK_SIGNAL.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let generation = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    let observed = *generation;
    let result = cvar.wait_timeout_while(generation, timeout, |current| *current == observed);
    let (guard, _) = result.unwrap_or_else(|poison| poison.into_inner());
    *guard != observed
}

pub fn record_vector_files_completed(count: u64) {
    VECTOR_FILES_COMPLETED_TOTAL.fetch_add(count, Ordering::Relaxed);
    if count > 0 {
        observe_useful_resume_after_quiescent();
    }
}

pub fn record_vector_claimed_work_items(count: u64) {
    VECTOR_CLAIMED_WORK_ITEMS_TOTAL.fetch_add(count, Ordering::Relaxed);
}

pub fn record_vector_partial_file_cycles(count: u64) {
    VECTOR_PARTIAL_FILE_CYCLES_TOTAL.fetch_add(count, Ordering::Relaxed);
}

pub fn record_vector_mark_done_call() {
    VECTOR_MARK_DONE_CALLS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_vector_prepare_dispatch() {
    VECTOR_PREPARE_DISPATCH_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_vector_prepare_prefetch() {
    VECTOR_PREPARE_PREFETCH_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_vector_prepare_fallback_inline() {
    VECTOR_PREPARE_FALLBACK_INLINE_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_vector_prepare_outcome(
    work_items: u64,
    immediate_completed: u64,
    failed_fetches: u64,
) {
    VECTOR_PREPARED_WORK_ITEMS_TOTAL.fetch_add(work_items, Ordering::Relaxed);
    VECTOR_PREPARE_IMMEDIATE_COMPLETED_TOTAL.fetch_add(immediate_completed, Ordering::Relaxed);
    VECTOR_PREPARE_FAILED_FETCHES_TOTAL.fetch_add(failed_fetches, Ordering::Relaxed);
    if work_items == 0 {
        VECTOR_PREPARE_EMPTY_BATCHES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn record_vector_finalize_enqueued() {
    VECTOR_FINALIZE_ENQUEUED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_vector_finalize_fallback_inline() {
    VECTOR_FINALIZE_FALLBACK_INLINE_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_vector_prepare_reply_wait_ms(latency_ms: u64) {
    VECTOR_PREPARE_REPLY_WAIT_MS_TOTAL.fetch_add(latency_ms, Ordering::Relaxed);
}

pub fn record_vector_prepare_send_wait_ms(latency_ms: u64) {
    VECTOR_PREPARE_SEND_WAIT_MS_TOTAL.fetch_add(latency_ms, Ordering::Relaxed);
}

pub fn record_vector_finalize_send_wait_ms(latency_ms: u64) {
    VECTOR_FINALIZE_SEND_WAIT_MS_TOTAL.fetch_add(latency_ms, Ordering::Relaxed);
}

pub fn record_vector_prepare_queue_wait_ms(latency_ms: u64) {
    VECTOR_PREPARE_QUEUE_WAIT_MS_TOTAL.fetch_add(latency_ms, Ordering::Relaxed);
}

pub fn record_vector_finalize_queue_wait_ms(latency_ms: u64) {
    VECTOR_FINALIZE_QUEUE_WAIT_MS_TOTAL.fetch_add(latency_ms, Ordering::Relaxed);
}

pub fn record_vector_prepare_queue_depth(depth: u64) {
    VECTOR_PREPARE_QUEUE_DEPTH_CURRENT.store(depth, Ordering::Relaxed);
    update_atomic_max(&VECTOR_PREPARE_QUEUE_DEPTH_MAX, depth);
}

pub fn record_vector_prepare_inflight_depth(depth: u64) {
    VECTOR_PREPARE_INFLIGHT_CURRENT.store(depth, Ordering::Relaxed);
    update_atomic_max(&VECTOR_PREPARE_INFLIGHT_MAX, depth);
}

pub fn record_vector_prepare_inflight_chunks(chunks: u64) {
    VECTOR_PREPARE_INFLIGHT_CHUNKS_CURRENT.store(chunks, Ordering::Relaxed);
    update_atomic_max(&VECTOR_PREPARE_INFLIGHT_CHUNKS_MAX, chunks);
}

pub fn record_vector_finalize_queue_depth(depth: u64) {
    VECTOR_FINALIZE_QUEUE_DEPTH_CURRENT.store(depth, Ordering::Relaxed);
    update_atomic_max(&VECTOR_FINALIZE_QUEUE_DEPTH_MAX, depth);
}

pub fn record_vector_ready_queue_depth(depth: u64) {
    VECTOR_READY_QUEUE_DEPTH_CURRENT.store(depth, Ordering::Relaxed);
    update_atomic_max(&VECTOR_READY_QUEUE_DEPTH_MAX, depth);
}

pub fn record_vector_ready_queue_chunks(chunks: u64) {
    VECTOR_READY_QUEUE_CHUNKS_CURRENT.store(chunks, Ordering::Relaxed);
    update_atomic_max(&VECTOR_READY_QUEUE_CHUNKS_MAX, chunks);
}

pub fn record_vector_ready_queue_lane_chunks(small: u64, medium: u64, large: u64) {
    VECTOR_READY_QUEUE_CHUNKS_SMALL.store(small, Ordering::Relaxed);
    VECTOR_READY_QUEUE_CHUNKS_MEDIUM.store(medium, Ordering::Relaxed);
    VECTOR_READY_QUEUE_CHUNKS_LARGE.store(large, Ordering::Relaxed);
}

pub fn record_vector_ready_queue_lane_batches(small: u64, medium: u64, large: u64, mixed: u64) {
    VECTOR_READY_BATCHES_SMALL.store(small, Ordering::Relaxed);
    VECTOR_READY_BATCHES_MEDIUM.store(medium, Ordering::Relaxed);
    VECTOR_READY_BATCHES_LARGE.store(large, Ordering::Relaxed);
    VECTOR_READY_BATCHES_MIXED.store(mixed, Ordering::Relaxed);
}

pub fn record_vector_batch_shape(homogeneous: bool) {
    if homogeneous {
        VECTOR_HOMOGENEOUS_BATCHES_TOTAL.fetch_add(1, Ordering::Relaxed);
    } else {
        VECTOR_MIXED_FALLBACK_BATCHES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn record_vector_last_consumed_batch_lane(lane: VectorBatchLaneKind) {
    VECTOR_LAST_CONSUMED_BATCH_LANE_CODE.store(lane.code(), Ordering::Relaxed);
}

pub fn record_vector_active_lane_thresholds(small_max_tokens: u64, medium_max_tokens: u64) {
    VECTOR_ACTIVE_SMALL_MAX_TOKENS.store(small_max_tokens, Ordering::Relaxed);
    VECTOR_ACTIVE_MEDIUM_MAX_TOKENS.store(medium_max_tokens, Ordering::Relaxed);
}

pub fn record_vector_ready_replenishment_requested(count: u64) {
    if count == 0 {
        return;
    }
    let next = VECTOR_READY_REPLENISHMENT_DEFICIT_CURRENT
        .fetch_add(count, Ordering::Relaxed)
        .saturating_add(count);
    update_atomic_max(&VECTOR_READY_REPLENISHMENT_DEFICIT_MAX, next);
}

pub fn record_vector_ready_replenishment_fulfilled(count: u64) {
    if count == 0 {
        return;
    }
    let _ = VECTOR_READY_REPLENISHMENT_DEFICIT_CURRENT.fetch_update(
        Ordering::Relaxed,
        Ordering::Relaxed,
        |current| Some(current.saturating_sub(count)),
    );
}

pub fn set_vector_ready_replenishment_deficit(current: u64) {
    VECTOR_READY_REPLENISHMENT_DEFICIT_CURRENT.store(current, Ordering::Relaxed);
    update_atomic_max(&VECTOR_READY_REPLENISHMENT_DEFICIT_MAX, current);
}

pub fn record_vector_active_claimed(depth: u64) {
    VECTOR_ACTIVE_CLAIMED_CURRENT.store(depth, Ordering::Relaxed);
}

pub fn record_vector_prepare_claimed(depth: u64) {
    VECTOR_PREPARE_CLAIMED_CURRENT.store(depth, Ordering::Relaxed);
}

pub fn record_vector_ready_claimed(depth: u64) {
    VECTOR_READY_CLAIMED_CURRENT.store(depth, Ordering::Relaxed);
}

pub fn record_vector_persist_queue_depth(depth: u64) {
    VECTOR_PERSIST_QUEUE_DEPTH_CURRENT.store(depth, Ordering::Relaxed);
    update_atomic_max(&VECTOR_PERSIST_QUEUE_DEPTH_MAX, depth);
}

pub fn record_vector_persist_claimed(depth: u64) {
    VECTOR_PERSIST_CLAIMED_CURRENT.store(depth, Ordering::Relaxed);
}

pub fn record_vector_persist_send_wait_ms(latency_ms: u64) {
    VECTOR_PERSIST_SEND_WAIT_MS_TOTAL.fetch_add(latency_ms, Ordering::Relaxed);
}

pub fn record_vector_persist_queue_wait_ms(latency_ms: u64) {
    VECTOR_PERSIST_QUEUE_WAIT_MS_TOTAL.fetch_add(latency_ms, Ordering::Relaxed);
}

pub fn record_vector_gpu_idle_wait_ms(latency_ms: u64) {
    VECTOR_GPU_IDLE_WAIT_MS_TOTAL.fetch_add(latency_ms, Ordering::Relaxed);
}

pub fn record_vector_canonical_backlog_depth(depth: u64) {
    VECTOR_CANONICAL_BACKLOG_DEPTH_CURRENT.store(depth, Ordering::Relaxed);
    update_atomic_max(&VECTOR_CANONICAL_BACKLOG_DEPTH_MAX, depth);
}

pub fn record_vector_oldest_ready_batch_age_ms(latency_ms: u64) {
    VECTOR_OLDEST_READY_BATCH_AGE_MS_CURRENT.store(latency_ms, Ordering::Relaxed);
    update_atomic_max(&VECTOR_OLDEST_READY_BATCH_AGE_MS_MAX, latency_ms);
}

pub fn background_launches_suppressed_total() -> u64 {
    BACKGROUND_LAUNCHES_SUPPRESSED_TOTAL.load(Ordering::Relaxed)
}

pub fn vectorization_suppressed_total() -> u64 {
    VECTORIZATION_SUPPRESSED_TOTAL.load(Ordering::Relaxed)
}

pub fn projection_suppressed_total() -> u64 {
    PROJECTION_SUPPRESSED_TOTAL.load(Ordering::Relaxed)
}

pub fn vectorization_interrupted_total() -> u64 {
    VECTORIZATION_INTERRUPTED_TOTAL.load(Ordering::Relaxed)
}

pub fn vectorization_requeued_for_interactive_total() -> u64 {
    VECTORIZATION_REQUEUED_FOR_INTERACTIVE_TOTAL.load(Ordering::Relaxed)
}

pub fn vectorization_resumed_after_interactive_total() -> u64 {
    VECTORIZATION_RESUMED_AFTER_INTERACTIVE_TOTAL.load(Ordering::Relaxed)
}

pub fn graph_workers_started_total() -> u64 {
    GRAPH_WORKERS_STARTED_TOTAL.load(Ordering::Relaxed)
}

pub fn graph_workers_active_current() -> u64 {
    GRAPH_WORKERS_ACTIVE_CURRENT.load(Ordering::Relaxed)
}

pub fn graph_worker_heartbeat_at_ms() -> u64 {
    GRAPH_WORKER_HEARTBEAT_AT_MS.load(Ordering::Relaxed)
}

pub fn vector_chunks_embedded_cumulative() -> u64 {
    VECTOR_CHUNKS_EMBEDDED_TOTAL.load(Ordering::Relaxed)
}

pub fn vector_chunk_embeddings_per_second() -> f64 {
    vector_chunk_embeddings_per_second_at(now_ms(), VECTOR_EMBED_THROUGHPUT_WINDOW_MS)
}

pub fn vector_chunk_embeddings_rate_window_ms() -> u64 {
    VECTOR_EMBED_THROUGHPUT_WINDOW_MS
}

pub fn vector_runtime_metrics() -> VectorRuntimeMetrics {
    let mut metrics = VectorRuntimeMetrics {
        fetch_ms_total: VECTOR_FETCH_MS_TOTAL.load(Ordering::Relaxed),
        embed_ms_total: VECTOR_EMBED_MS_TOTAL.load(Ordering::Relaxed),
        db_write_ms_total: VECTOR_DB_WRITE_MS_TOTAL.load(Ordering::Relaxed),
        completion_check_ms_total: VECTOR_COMPLETION_CHECK_MS_TOTAL.load(Ordering::Relaxed),
        mark_done_ms_total: VECTOR_MARK_DONE_MS_TOTAL.load(Ordering::Relaxed),
        batches_total: VECTOR_BATCHES_TOTAL.load(Ordering::Relaxed),
        chunks_embedded_total: VECTOR_CHUNKS_EMBEDDED_TOTAL.load(Ordering::Relaxed),
        files_completed_total: VECTOR_FILES_COMPLETED_TOTAL.load(Ordering::Relaxed),
        embed_calls_total: VECTOR_EMBED_CALLS_TOTAL.load(Ordering::Relaxed),
        claimed_work_items_total: VECTOR_CLAIMED_WORK_ITEMS_TOTAL.load(Ordering::Relaxed),
        partial_file_cycles_total: VECTOR_PARTIAL_FILE_CYCLES_TOTAL.load(Ordering::Relaxed),
        mark_done_calls_total: VECTOR_MARK_DONE_CALLS_TOTAL.load(Ordering::Relaxed),
        files_touched_total: VECTOR_FILES_TOUCHED_TOTAL.load(Ordering::Relaxed),
        prepare_dispatch_total: VECTOR_PREPARE_DISPATCH_TOTAL.load(Ordering::Relaxed),
        prepare_prefetch_total: VECTOR_PREPARE_PREFETCH_TOTAL.load(Ordering::Relaxed),
        prepare_fallback_inline_total: VECTOR_PREPARE_FALLBACK_INLINE_TOTAL.load(Ordering::Relaxed),
        prepared_work_items_total: VECTOR_PREPARED_WORK_ITEMS_TOTAL.load(Ordering::Relaxed),
        prepare_empty_batches_total: VECTOR_PREPARE_EMPTY_BATCHES_TOTAL.load(Ordering::Relaxed),
        prepare_immediate_completed_total: VECTOR_PREPARE_IMMEDIATE_COMPLETED_TOTAL
            .load(Ordering::Relaxed),
        prepare_failed_fetches_total: VECTOR_PREPARE_FAILED_FETCHES_TOTAL.load(Ordering::Relaxed),
        finalize_enqueued_total: VECTOR_FINALIZE_ENQUEUED_TOTAL.load(Ordering::Relaxed),
        finalize_fallback_inline_total: VECTOR_FINALIZE_FALLBACK_INLINE_TOTAL
            .load(Ordering::Relaxed),
        prepare_reply_wait_ms_total: VECTOR_PREPARE_REPLY_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        prepare_send_wait_ms_total: VECTOR_PREPARE_SEND_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        finalize_send_wait_ms_total: VECTOR_FINALIZE_SEND_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        prepare_queue_wait_ms_total: VECTOR_PREPARE_QUEUE_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        finalize_queue_wait_ms_total: VECTOR_FINALIZE_QUEUE_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        prepare_queue_depth_current: VECTOR_PREPARE_QUEUE_DEPTH_CURRENT.load(Ordering::Relaxed),
        prepare_queue_depth_max: VECTOR_PREPARE_QUEUE_DEPTH_MAX.load(Ordering::Relaxed),
        prepare_inflight_current: VECTOR_PREPARE_INFLIGHT_CURRENT.load(Ordering::Relaxed),
        prepare_inflight_max: VECTOR_PREPARE_INFLIGHT_MAX.load(Ordering::Relaxed),
        prepare_inflight_chunks_current: VECTOR_PREPARE_INFLIGHT_CHUNKS_CURRENT
            .load(Ordering::Relaxed),
        prepare_inflight_chunks_max: VECTOR_PREPARE_INFLIGHT_CHUNKS_MAX.load(Ordering::Relaxed),
        ready_queue_depth_current: VECTOR_READY_QUEUE_DEPTH_CURRENT.load(Ordering::Relaxed),
        ready_queue_depth_max: VECTOR_READY_QUEUE_DEPTH_MAX.load(Ordering::Relaxed),
        ready_queue_chunks_current: VECTOR_READY_QUEUE_CHUNKS_CURRENT.load(Ordering::Relaxed),
        ready_queue_chunks_max: VECTOR_READY_QUEUE_CHUNKS_MAX.load(Ordering::Relaxed),
        ready_queue_chunks_small: VECTOR_READY_QUEUE_CHUNKS_SMALL.load(Ordering::Relaxed),
        ready_queue_chunks_medium: VECTOR_READY_QUEUE_CHUNKS_MEDIUM.load(Ordering::Relaxed),
        ready_queue_chunks_large: VECTOR_READY_QUEUE_CHUNKS_LARGE.load(Ordering::Relaxed),
        ready_batches_small: VECTOR_READY_BATCHES_SMALL.load(Ordering::Relaxed),
        ready_batches_medium: VECTOR_READY_BATCHES_MEDIUM.load(Ordering::Relaxed),
        ready_batches_large: VECTOR_READY_BATCHES_LARGE.load(Ordering::Relaxed),
        ready_batches_mixed: VECTOR_READY_BATCHES_MIXED.load(Ordering::Relaxed),
        homogeneous_batches_total: VECTOR_HOMOGENEOUS_BATCHES_TOTAL.load(Ordering::Relaxed),
        mixed_fallback_batches_total: VECTOR_MIXED_FALLBACK_BATCHES_TOTAL.load(Ordering::Relaxed),
        last_consumed_batch_lane: VectorBatchLaneKind::from_code(
            VECTOR_LAST_CONSUMED_BATCH_LANE_CODE.load(Ordering::Relaxed),
        ),
        active_small_max_tokens: VECTOR_ACTIVE_SMALL_MAX_TOKENS.load(Ordering::Relaxed),
        active_medium_max_tokens: VECTOR_ACTIVE_MEDIUM_MAX_TOKENS.load(Ordering::Relaxed),
        ready_replenishment_deficit_current: VECTOR_READY_REPLENISHMENT_DEFICIT_CURRENT
            .load(Ordering::Relaxed),
        ready_replenishment_deficit_max: VECTOR_READY_REPLENISHMENT_DEFICIT_MAX
            .load(Ordering::Relaxed),
        active_claimed_current: VECTOR_ACTIVE_CLAIMED_CURRENT.load(Ordering::Relaxed),
        prepare_claimed_current: VECTOR_PREPARE_CLAIMED_CURRENT.load(Ordering::Relaxed),
        ready_claimed_current: VECTOR_READY_CLAIMED_CURRENT.load(Ordering::Relaxed),
        finalize_queue_depth_current: VECTOR_FINALIZE_QUEUE_DEPTH_CURRENT.load(Ordering::Relaxed),
        finalize_queue_depth_max: VECTOR_FINALIZE_QUEUE_DEPTH_MAX.load(Ordering::Relaxed),
        persist_queue_depth_current: VECTOR_PERSIST_QUEUE_DEPTH_CURRENT.load(Ordering::Relaxed),
        persist_queue_depth_max: VECTOR_PERSIST_QUEUE_DEPTH_MAX.load(Ordering::Relaxed),
        persist_claimed_current: VECTOR_PERSIST_CLAIMED_CURRENT.load(Ordering::Relaxed),
        persist_send_wait_ms_total: VECTOR_PERSIST_SEND_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        persist_queue_wait_ms_total: VECTOR_PERSIST_QUEUE_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        gpu_idle_wait_ms_total: VECTOR_GPU_IDLE_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        canonical_backlog_depth_current: VECTOR_CANONICAL_BACKLOG_DEPTH_CURRENT
            .load(Ordering::Relaxed),
        canonical_backlog_depth_max: VECTOR_CANONICAL_BACKLOG_DEPTH_MAX.load(Ordering::Relaxed),
        oldest_ready_batch_age_ms_current: VECTOR_OLDEST_READY_BATCH_AGE_MS_CURRENT
            .load(Ordering::Relaxed),
        oldest_ready_batch_age_ms_max: VECTOR_OLDEST_READY_BATCH_AGE_MS_MAX.load(Ordering::Relaxed),
        embed_input_texts_total: VECTOR_EMBED_INPUT_TEXTS_TOTAL.load(Ordering::Relaxed),
        embed_input_text_bytes_total: VECTOR_EMBED_INPUT_TEXT_BYTES_TOTAL.load(Ordering::Relaxed),
        embed_clone_ms_total: VECTOR_EMBED_CLONE_MS_TOTAL.load(Ordering::Relaxed),
        embed_transform_ms_total: VECTOR_EMBED_TRANSFORM_MS_TOTAL.load(Ordering::Relaxed),
        embed_export_ms_total: VECTOR_EMBED_EXPORT_MS_TOTAL.load(Ordering::Relaxed),
        embed_attempts_total: VECTOR_EMBED_ATTEMPTS_TOTAL.load(Ordering::Relaxed),
        embed_inflight_started_at_ms: VECTOR_EMBED_INFLIGHT_STARTED_AT_MS.load(Ordering::Relaxed),
        embed_inflight_texts_current: VECTOR_EMBED_INFLIGHT_TEXTS_CURRENT.load(Ordering::Relaxed),
        embed_inflight_text_bytes_current: VECTOR_EMBED_INFLIGHT_TEXT_BYTES_CURRENT
            .load(Ordering::Relaxed),
        last_embed_attempt_wall_ms: VECTOR_LAST_EMBED_ATTEMPT_WALL_MS.load(Ordering::Relaxed),
        avg_embed_attempt_wall_ms: 0.0,
        max_embed_attempt_wall_ms: VECTOR_EMBED_ATTEMPT_WALL_MS_MAX.load(Ordering::Relaxed),
        last_embed_gap_ms: VECTOR_LAST_EMBED_GAP_MS.load(Ordering::Relaxed),
        avg_embed_gap_ms: 0.0,
        max_embed_gap_ms: VECTOR_EMBED_GAP_MS_MAX.load(Ordering::Relaxed),
        vector_workers_started_total: VECTOR_WORKERS_STARTED_TOTAL.load(Ordering::Relaxed),
        vector_workers_stopped_total: VECTOR_WORKERS_STOPPED_TOTAL.load(Ordering::Relaxed),
        vector_workers_active_current: VECTOR_WORKERS_ACTIVE_CURRENT.load(Ordering::Relaxed),
        vector_worker_heartbeat_at_ms: VECTOR_WORKER_HEARTBEAT_AT_MS.load(Ordering::Relaxed),
        vector_worker_restarts_total: VECTOR_WORKER_RESTARTS_TOTAL.load(Ordering::Relaxed),
        graph_workers_started_total: GRAPH_WORKERS_STARTED_TOTAL.load(Ordering::Relaxed),
        graph_workers_stopped_total: GRAPH_WORKERS_STOPPED_TOTAL.load(Ordering::Relaxed),
        graph_workers_active_current: GRAPH_WORKERS_ACTIVE_CURRENT.load(Ordering::Relaxed),
        graph_worker_heartbeat_at_ms: GRAPH_WORKER_HEARTBEAT_AT_MS.load(Ordering::Relaxed),
        vector_lane_state: VectorLaneState::from_code(
            VECTOR_LANE_STATE_CODE.load(Ordering::Relaxed),
        ),
        vector_lane_last_transition_at_ms: VECTOR_LANE_LAST_TRANSITION_AT_MS
            .load(Ordering::Relaxed),
        vector_lane_last_success_at_ms: VECTOR_LANE_LAST_SUCCESS_AT_MS.load(Ordering::Relaxed),
        vector_lane_last_fault_at_ms: VECTOR_LANE_LAST_FAULT_AT_MS.load(Ordering::Relaxed),
        chunk_embeddings_per_second: vector_chunk_embeddings_per_second(),
        chunk_embeddings_rate_window_ms: VECTOR_EMBED_THROUGHPUT_WINDOW_MS,
    };
    if metrics.embed_attempts_total > 0 {
        metrics.avg_embed_attempt_wall_ms = VECTOR_EMBED_ATTEMPT_WALL_MS_TOTAL
            .load(Ordering::Relaxed) as f64
            / metrics.embed_attempts_total as f64;
    }
    let gap_samples = VECTOR_EMBED_GAP_SAMPLES_TOTAL.load(Ordering::Relaxed);
    if gap_samples > 0 {
        metrics.avg_embed_gap_ms =
            VECTOR_EMBED_GAP_MS_TOTAL.load(Ordering::Relaxed) as f64 / gap_samples as f64;
    }
    metrics
}

pub fn vector_runtime_latency_summaries() -> VectorRuntimeLatencySummaries {
    let windows = vector_stage_latency_windows()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    VectorRuntimeLatencySummaries {
        fetch: summarize_window(&windows.fetch),
        embed: summarize_window(&windows.embed),
        db_write: summarize_window(&windows.db_write),
        completion_check: summarize_window(&windows.completion_check),
        mark_done: summarize_window(&windows.mark_done),
    }
}

pub fn vector_pipeline_stage_telemetry() -> VectorPipelineStageTelemetry {
    VectorPipelineStageTelemetry {
        prepare_ms_total: VECTOR_PREPARE_REPLY_WAIT_MS_TOTAL
            .load(Ordering::Relaxed)
            .saturating_add(VECTOR_PREPARE_SEND_WAIT_MS_TOTAL.load(Ordering::Relaxed))
            .saturating_add(VECTOR_PREPARE_QUEUE_WAIT_MS_TOTAL.load(Ordering::Relaxed)),
        ready_wait_ms_total: VECTOR_GPU_IDLE_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        inference_ms_total: VECTOR_EMBED_TRANSFORM_MS_TOTAL.load(Ordering::Relaxed),
        output_extract_ms_total: VECTOR_EMBED_EXPORT_MS_TOTAL.load(Ordering::Relaxed),
        persist_ms_total: VECTOR_DB_WRITE_MS_TOTAL
            .load(Ordering::Relaxed)
            .saturating_add(VECTOR_PERSIST_SEND_WAIT_MS_TOTAL.load(Ordering::Relaxed))
            .saturating_add(VECTOR_PERSIST_QUEUE_WAIT_MS_TOTAL.load(Ordering::Relaxed)),
        finalize_ms_total: VECTOR_COMPLETION_CHECK_MS_TOTAL
            .load(Ordering::Relaxed)
            .saturating_add(VECTOR_MARK_DONE_MS_TOTAL.load(Ordering::Relaxed))
            .saturating_add(VECTOR_FINALIZE_SEND_WAIT_MS_TOTAL.load(Ordering::Relaxed))
            .saturating_add(VECTOR_FINALIZE_QUEUE_WAIT_MS_TOTAL.load(Ordering::Relaxed)),
    }
}

/// REQ-AXO-291 — crate-level test serialization mutex. Any test
/// (anywhere in the workspace) that calls `record_*` / `reset_for_tests`
/// or reads the runtime-pressure global state must hold this guard
/// for its critical section, otherwise parallel `cargo test`
/// invocations across modules race against each other on the shared
/// atomics. Returns a guard ; drop releases.
pub fn lock_for_tests() -> parking_lot::MutexGuard<'static, ()> {
    static TEST_SERIAL_GUARD: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    TEST_SERIAL_GUARD.lock()
}

pub fn reset_for_tests() {
    LAST_SQL_LATENCY_MS.store(0, Ordering::Relaxed);
    LAST_MCP_LATENCY_MS.store(0, Ordering::Relaxed);
    LAST_SAMPLE_AT_MS.store(0, Ordering::Relaxed);
    LAST_DEGRADED_AT_MS.store(0, Ordering::Relaxed);
    INTERACTIVE_REQUESTS_IN_FLIGHT.store(0, Ordering::Relaxed);
    LAST_INTERACTIVE_AT_MS.store(0, Ordering::Relaxed);
    BACKGROUND_LAUNCHES_SUPPRESSED_TOTAL.store(0, Ordering::Relaxed);
    VECTORIZATION_SUPPRESSED_TOTAL.store(0, Ordering::Relaxed);
    PROJECTION_SUPPRESSED_TOTAL.store(0, Ordering::Relaxed);
    VECTORIZATION_INTERRUPTED_TOTAL.store(0, Ordering::Relaxed);
    VECTORIZATION_REQUEUED_FOR_INTERACTIVE_TOTAL.store(0, Ordering::Relaxed);
    VECTORIZATION_RESUMED_AFTER_INTERACTIVE_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_FETCH_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_DB_WRITE_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_COMPLETION_CHECK_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_MARK_DONE_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_BATCHES_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_CHUNKS_EMBEDDED_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_FILES_COMPLETED_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_CALLS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_CLAIMED_WORK_ITEMS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PARTIAL_FILE_CYCLES_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_MARK_DONE_CALLS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_FILES_TOUCHED_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_DISPATCH_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_PREFETCH_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_FALLBACK_INLINE_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARED_WORK_ITEMS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_EMPTY_BATCHES_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_IMMEDIATE_COMPLETED_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_FAILED_FETCHES_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_FINALIZE_ENQUEUED_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_FINALIZE_FALLBACK_INLINE_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_REPLY_WAIT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_SEND_WAIT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_FINALIZE_SEND_WAIT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_QUEUE_WAIT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_FINALIZE_QUEUE_WAIT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_QUEUE_DEPTH_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_QUEUE_DEPTH_MAX.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_INFLIGHT_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_INFLIGHT_MAX.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_INFLIGHT_CHUNKS_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_INFLIGHT_CHUNKS_MAX.store(0, Ordering::Relaxed);
    VECTOR_READY_QUEUE_DEPTH_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_READY_QUEUE_DEPTH_MAX.store(0, Ordering::Relaxed);
    VECTOR_READY_QUEUE_CHUNKS_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_READY_QUEUE_CHUNKS_MAX.store(0, Ordering::Relaxed);
    VECTOR_READY_QUEUE_CHUNKS_SMALL.store(0, Ordering::Relaxed);
    VECTOR_READY_QUEUE_CHUNKS_MEDIUM.store(0, Ordering::Relaxed);
    VECTOR_READY_QUEUE_CHUNKS_LARGE.store(0, Ordering::Relaxed);
    VECTOR_READY_BATCHES_SMALL.store(0, Ordering::Relaxed);
    VECTOR_READY_BATCHES_MEDIUM.store(0, Ordering::Relaxed);
    VECTOR_READY_BATCHES_LARGE.store(0, Ordering::Relaxed);
    VECTOR_READY_BATCHES_MIXED.store(0, Ordering::Relaxed);
    VECTOR_HOMOGENEOUS_BATCHES_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_MIXED_FALLBACK_BATCHES_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_LAST_CONSUMED_BATCH_LANE_CODE.store(0, Ordering::Relaxed);
    VECTOR_ACTIVE_SMALL_MAX_TOKENS.store(0, Ordering::Relaxed);
    VECTOR_ACTIVE_MEDIUM_MAX_TOKENS.store(0, Ordering::Relaxed);
    VECTOR_READY_REPLENISHMENT_DEFICIT_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_READY_REPLENISHMENT_DEFICIT_MAX.store(0, Ordering::Relaxed);
    VECTOR_ACTIVE_CLAIMED_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_PREPARE_CLAIMED_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_READY_CLAIMED_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_FINALIZE_QUEUE_DEPTH_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_FINALIZE_QUEUE_DEPTH_MAX.store(0, Ordering::Relaxed);
    VECTOR_PERSIST_QUEUE_DEPTH_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_PERSIST_QUEUE_DEPTH_MAX.store(0, Ordering::Relaxed);
    VECTOR_PERSIST_CLAIMED_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_PERSIST_SEND_WAIT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PERSIST_QUEUE_WAIT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_GPU_IDLE_WAIT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_CANONICAL_BACKLOG_DEPTH_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_CANONICAL_BACKLOG_DEPTH_MAX.store(0, Ordering::Relaxed);
    VECTOR_OLDEST_READY_BATCH_AGE_MS_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_OLDEST_READY_BATCH_AGE_MS_MAX.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INPUT_TEXTS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INPUT_TEXT_BYTES_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_CLONE_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_TRANSFORM_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_EXPORT_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_ATTEMPTS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_STARTED_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXTS_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXT_BYTES_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_LAST_EMBED_ATTEMPT_WALL_MS.store(0, Ordering::Relaxed);
    VECTOR_EMBED_ATTEMPT_WALL_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_ATTEMPT_WALL_MS_MAX.store(0, Ordering::Relaxed);
    VECTOR_LAST_EMBED_FINISHED_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_LAST_EMBED_GAP_MS.store(0, Ordering::Relaxed);
    VECTOR_EMBED_GAP_MS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_GAP_SAMPLES_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_EMBED_GAP_MS_MAX.store(0, Ordering::Relaxed);
    VECTOR_WORKERS_STARTED_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_WORKERS_STOPPED_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_WORKERS_ACTIVE_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_WORKER_HEARTBEAT_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_WORKER_RESTARTS_TOTAL.store(0, Ordering::Relaxed);
    VECTOR_PIPELINE_PRODUCER_HEARTBEAT_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_PIPELINE_EMBEDDER_HEARTBEAT_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_PIPELINE_PERSISTER_HEARTBEAT_AT_MS.store(0, Ordering::Relaxed);
    GRAPH_WORKERS_STARTED_TOTAL.store(0, Ordering::Relaxed);
    GRAPH_WORKERS_STOPPED_TOTAL.store(0, Ordering::Relaxed);
    GRAPH_WORKERS_ACTIVE_CURRENT.store(0, Ordering::Relaxed);
    GRAPH_WORKER_HEARTBEAT_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_LANE_STATE_CODE.store(VectorLaneState::Starting.code(), Ordering::Relaxed);
    VECTOR_LANE_LAST_TRANSITION_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_LANE_LAST_SUCCESS_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_LANE_LAST_FAULT_AT_MS.store(0, Ordering::Relaxed);
    LAST_RUNTIME_WAKEUP_AT_MS.store(0, Ordering::Relaxed);
    LAST_QUIESCENT_ENTERED_AT_MS.store(0, Ordering::Relaxed);
    LAST_QUIESCENT_EXITED_AT_MS.store(0, Ordering::Relaxed);
    LAST_QUIESCENT_EXIT_REASON_CODE.store(0, Ordering::Relaxed);
    QUIESCENT_EXIT_ACTIVE_BACKLOG_TOTAL.store(0, Ordering::Relaxed);
    QUIESCENT_EXIT_DRAINING_RESIDUAL_TOTAL.store(0, Ordering::Relaxed);
    QUIESCENT_EXIT_INTERACTIVE_GUARDED_TOTAL.store(0, Ordering::Relaxed);
    LAST_RUNTIME_WAKE_SOURCE_CODE.store(0, Ordering::Relaxed);
    WAKE_SOURCE_BACKGROUND_TOTAL.store(0, Ordering::Relaxed);
    WAKE_SOURCE_SEMANTIC_VECTOR_TOTAL.store(0, Ordering::Relaxed);
    WAKE_SOURCE_GRAPH_TOTAL.store(0, Ordering::Relaxed);
    LAST_USEFUL_RESUME_AT_MS.store(0, Ordering::Relaxed);
    LAST_USEFUL_RESUME_FOR_EXIT_AT_MS.store(0, Ordering::Relaxed);
    LAST_OBSERVED_QUIESCENT_STATE_CODE.store(0, Ordering::Relaxed);
    RUNTIME_TRUTH_LAST_HEARTBEAT_AT_MS.store(0, Ordering::Relaxed);
    RUNTIME_TRUTH_LAST_GOOD_PAYLOAD_AT_MS.store(0, Ordering::Relaxed);
    RUNTIME_TRUTH_STALE_AFTER_MS.store(RuntimeTruthFeed::DEFAULT_STALE_AFTER_MS, Ordering::Relaxed);
    *runtime_truth_degraded_reason_cell()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = None;
    *vector_stage_latency_windows()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = VectorStageLatencyWindows::default();
    runtime_wake_timestamps()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clear();
    quiescent_resume_latencies_ms()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clear();
    quiescent_useful_resume_latencies_ms()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clear();
    vector_embed_throughput_samples()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clear();
}

fn vector_stage_latency_windows() -> &'static Mutex<VectorStageLatencyWindows> {
    VECTOR_STAGE_LATENCY_WINDOWS.get_or_init(|| Mutex::new(VectorStageLatencyWindows::default()))
}

fn vector_embed_throughput_samples() -> &'static Mutex<VecDeque<(u64, u64)>> {
    VECTOR_EMBED_THROUGHPUT_SAMPLES.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn record_vector_embed_throughput_sample(now_ms: u64, chunks: u64) {
    let mut samples = vector_embed_throughput_samples()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    while samples.len() >= VECTOR_EMBED_THROUGHPUT_CAPACITY {
        samples.pop_front();
    }
    samples.push_back((now_ms, chunks));
    let cutoff = now_ms.saturating_sub(VECTOR_EMBED_THROUGHPUT_HISTORY_MS);
    while matches!(samples.front(), Some((at_ms, _)) if *at_ms < cutoff) {
        samples.pop_front();
    }
}

fn vector_chunk_embeddings_per_second_at(now_ms: u64, window_ms: u64) -> f64 {
    let mut samples = vector_embed_throughput_samples()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let history_cutoff = now_ms.saturating_sub(VECTOR_EMBED_THROUGHPUT_HISTORY_MS);
    while matches!(samples.front(), Some((at_ms, _)) if *at_ms < history_cutoff) {
        samples.pop_front();
    }
    let cutoff = now_ms.saturating_sub(window_ms);
    let mut chunks_total = 0u64;
    let mut oldest_in_window = None;
    for (at_ms, chunks) in samples.iter().copied() {
        if at_ms >= cutoff {
            chunks_total = chunks_total.saturating_add(chunks);
            oldest_in_window = oldest_in_window.or(Some(at_ms));
        }
    }
    let Some(oldest_at_ms) = oldest_in_window else {
        return 0.0;
    };
    let elapsed_ms = now_ms
        .saturating_sub(oldest_at_ms)
        .clamp(1_000, window_ms.max(1_000));
    chunks_total as f64 / (elapsed_ms as f64 / 1_000.0)
}

fn runtime_wake_timestamps() -> &'static Mutex<VecDeque<u64>> {
    RUNTIME_WAKE_TIMESTAMPS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn quiescent_resume_latencies_ms() -> &'static Mutex<VecDeque<u64>> {
    QUIESCENT_RESUME_LATENCIES_MS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn quiescent_useful_resume_latencies_ms() -> &'static Mutex<VecDeque<u64>> {
    QUIESCENT_USEFUL_RESUME_LATENCIES_MS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn observe_quiescent_transition(state: RuntimeQuiescentState) {
    let now = now_ms();
    let code = state.code();
    let previous = LAST_OBSERVED_QUIESCENT_STATE_CODE.swap(code, Ordering::Relaxed);
    if previous == code {
        return;
    }
    if code == RuntimeQuiescentState::QuiescentCandidate.code() {
        LAST_QUIESCENT_ENTERED_AT_MS.store(now, Ordering::Relaxed);
    } else if previous == RuntimeQuiescentState::QuiescentCandidate.code() {
        let entered_at = LAST_QUIESCENT_ENTERED_AT_MS.load(Ordering::Relaxed);
        if entered_at > 0 {
            let latency_ms = now.saturating_sub(entered_at);
            let mut window = quiescent_resume_latencies_ms()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if window.len() >= VECTOR_STAGE_WINDOW_CAPACITY {
                window.pop_front();
            }
            window.push_back(latency_ms);
        }
        LAST_QUIESCENT_EXIT_REASON_CODE.store(code, Ordering::Relaxed);
        match state {
            RuntimeQuiescentState::ActiveBacklog => {
                QUIESCENT_EXIT_ACTIVE_BACKLOG_TOTAL.fetch_add(1, Ordering::Relaxed);
            }
            RuntimeQuiescentState::DrainingResidualWork => {
                QUIESCENT_EXIT_DRAINING_RESIDUAL_TOTAL.fetch_add(1, Ordering::Relaxed);
            }
            RuntimeQuiescentState::InteractiveGuarded => {
                QUIESCENT_EXIT_INTERACTIVE_GUARDED_TOTAL.fetch_add(1, Ordering::Relaxed);
            }
            RuntimeQuiescentState::QuiescentCandidate => {}
        }
        LAST_QUIESCENT_EXITED_AT_MS.store(now, Ordering::Relaxed);
    }
}

fn observe_useful_resume_after_quiescent() {
    let now = now_ms();
    let exited_at = LAST_QUIESCENT_EXITED_AT_MS.load(Ordering::Relaxed);
    if exited_at == 0 {
        return;
    }
    let already_recorded_for_exit = LAST_USEFUL_RESUME_FOR_EXIT_AT_MS.load(Ordering::Relaxed);
    if already_recorded_for_exit >= exited_at {
        return;
    }
    let latency_ms = now.saturating_sub(exited_at);
    let mut window = quiescent_useful_resume_latencies_ms()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if window.len() >= VECTOR_STAGE_WINDOW_CAPACITY {
        window.pop_front();
    }
    window.push_back(latency_ms);
    LAST_USEFUL_RESUME_AT_MS.store(now, Ordering::Relaxed);
    LAST_USEFUL_RESUME_FOR_EXIT_AT_MS.store(exited_at, Ordering::Relaxed);
}

fn summarize_runtime_window(window: &Mutex<VecDeque<u64>>) -> VectorStageLatencySummary {
    let guard = window.lock().unwrap_or_else(|poison| poison.into_inner());
    summarize_window(&guard)
}

fn summarize_window(window: &VecDeque<u64>) -> VectorStageLatencySummary {
    if window.is_empty() {
        return VectorStageLatencySummary {
            samples: 0,
            p50_ms: 0,
            p95_ms: 0,
            max_ms: 0,
        };
    }

    let mut values: Vec<u64> = window.iter().copied().collect();
    values.sort_unstable();
    let len = values.len();
    let idx = |percentile: f64| -> usize {
        (((len.saturating_sub(1)) as f64) * percentile).round() as usize
    };
    VectorStageLatencySummary {
        samples: len as u64,
        p50_ms: values[idx(0.50)],
        p95_ms: values[idx(0.95)],
        max_ms: *values.last().unwrap_or(&0),
    }
}

fn update_atomic_max(target: &AtomicU64, candidate: u64) {
    let _ = target.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        (candidate > current).then_some(candidate)
    });
}

fn runtime_truth_degraded_reason_cell() -> &'static Mutex<Option<String>> {
    RUNTIME_TRUTH_DEGRADED_REASON.get_or_init(|| Mutex::new(None))
}

fn vector_last_worker_admission_reason_cell() -> &'static Mutex<String> {
    VECTOR_LAST_WORKER_ADMISSION_REASON.get_or_init(|| Mutex::new("unknown".to_string()))
}

pub fn record_vector_worker_admission_reason(reason: &str, allowed_gpu_workers: usize) {
    let mut guard = vector_last_worker_admission_reason_cell()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    guard.clear();
    guard.push_str(reason);
    VECTOR_LAST_ALLOWED_GPU_WORKERS.store(allowed_gpu_workers as u64, Ordering::Relaxed);
}

pub fn current_vector_worker_admission_reason() -> String {
    vector_last_worker_admission_reason_cell()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

pub fn current_allowed_gpu_workers() -> u64 {
    VECTOR_LAST_ALLOWED_GPU_WORKERS.load(Ordering::Relaxed)
}

fn runtime_truth_feed_at(now_ms: u64) -> RuntimeTruthFeed {
    let stale_after_ms = RUNTIME_TRUTH_STALE_AFTER_MS.load(Ordering::Relaxed).max(1);
    let last_heartbeat_at_ms = match RUNTIME_TRUTH_LAST_HEARTBEAT_AT_MS.load(Ordering::Relaxed) {
        0 => None,
        value => Some(value),
    };
    let last_good_payload_at_ms =
        match RUNTIME_TRUTH_LAST_GOOD_PAYLOAD_AT_MS.load(Ordering::Relaxed) {
            0 => None,
            value => Some(value),
        };
    let degraded_reason = runtime_truth_degraded_reason_cell()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();

    RuntimeTruthFeed::from_observed_times(
        now_ms,
        last_heartbeat_at_ms,
        last_good_payload_at_ms,
        stale_after_ms,
        degraded_reason,
    )
}

pub fn current_runtime_truth_feed() -> RuntimeTruthFeed {
    runtime_truth_feed_at(now_ms())
}

pub fn current_runtime_truth_snapshot() -> RuntimeTruthFeed {
    let stale_after_ms = RUNTIME_TRUTH_STALE_AFTER_MS.load(Ordering::Relaxed).max(1);
    let last_good_payload_at_ms =
        match RUNTIME_TRUTH_LAST_GOOD_PAYLOAD_AT_MS.load(Ordering::Relaxed) {
            0 => None,
            value => Some(value),
        };
    let degraded_reason = runtime_truth_degraded_reason_cell()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let degraded_reason = if last_good_payload_at_ms.is_some() {
        degraded_reason
    } else {
        degraded_reason.or_else(|| Some("ist_snapshot_not_recently_refreshed".to_string()))
    };

    RuntimeTruthFeed::from_observed_times(
        now_ms(),
        last_good_payload_at_ms,
        last_good_payload_at_ms,
        stale_after_ms,
        degraded_reason,
    )
}

pub fn record_runtime_truth_bridge_dispatch(degraded_reason: Option<&str>) -> RuntimeTruthFeed {
    let now = now_ms();
    RUNTIME_TRUTH_LAST_HEARTBEAT_AT_MS.store(now, Ordering::Relaxed);
    if degraded_reason.is_none() {
        RUNTIME_TRUTH_LAST_GOOD_PAYLOAD_AT_MS.store(now, Ordering::Relaxed);
    }
    *runtime_truth_degraded_reason_cell()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = degraded_reason.map(str::to_string);
    runtime_truth_feed_at(now)
}

pub fn set_runtime_truth_feed_for_tests(
    last_heartbeat_at_ms: Option<u64>,
    last_good_payload_at_ms: Option<u64>,
    stale_after_ms: u64,
    degraded_reason: Option<&str>,
) {
    RUNTIME_TRUTH_LAST_HEARTBEAT_AT_MS.store(last_heartbeat_at_ms.unwrap_or(0), Ordering::Relaxed);
    RUNTIME_TRUTH_LAST_GOOD_PAYLOAD_AT_MS
        .store(last_good_payload_at_ms.unwrap_or(0), Ordering::Relaxed);
    RUNTIME_TRUTH_STALE_AFTER_MS.store(stale_after_ms.max(1), Ordering::Relaxed);
    *runtime_truth_degraded_reason_cell()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = degraded_reason.map(str::to_string);
}

fn recent_peak_latency_ms_at(now_ms: u64) -> u64 {
    let last_seen = LAST_SAMPLE_AT_MS.load(Ordering::Relaxed);
    if last_seen == 0 || now_ms.saturating_sub(last_seen) > SERVICE_SAMPLE_TTL_MS {
        return 0;
    }

    LAST_SQL_LATENCY_MS
        .load(Ordering::Relaxed)
        .max(LAST_MCP_LATENCY_MS.load(Ordering::Relaxed))
}

fn current_pressure_at(now_ms: u64) -> ServicePressure {
    let last_seen = LAST_SAMPLE_AT_MS.load(Ordering::Relaxed);
    let last_degraded = LAST_DEGRADED_AT_MS.load(Ordering::Relaxed);
    if last_seen == 0 {
        return ServicePressure::Healthy;
    }

    let age_ms = now_ms.saturating_sub(last_seen);
    let peak = LAST_SQL_LATENCY_MS
        .load(Ordering::Relaxed)
        .max(LAST_MCP_LATENCY_MS.load(Ordering::Relaxed));

    if age_ms <= SERVICE_SAMPLE_TTL_MS {
        if peak >= 1_500 {
            ServicePressure::Critical
        } else if peak >= 500 {
            ServicePressure::Degraded
        } else if last_degraded != 0
            && now_ms.saturating_sub(last_degraded) <= SERVICE_RECOVERY_WINDOW_MS
        {
            ServicePressure::Recovering
        } else {
            ServicePressure::Healthy
        }
    } else if last_degraded != 0
        && now_ms.saturating_sub(last_degraded) <= SERVICE_RECOVERY_WINDOW_MS
    {
        if peak >= 500 || age_ms <= SERVICE_SAMPLE_TTL_MS * 3 {
            ServicePressure::Recovering
        } else {
            ServicePressure::Healthy
        }
    } else {
        ServicePressure::Healthy
    }
}

fn current_interactive_priority_at(now_ms: u64) -> InteractivePriority {
    let in_flight = INTERACTIVE_REQUESTS_IN_FLIGHT.load(Ordering::Relaxed);
    if in_flight == 0 {
        return InteractivePriority::BackgroundNormal;
    }

    match current_pressure_at(now_ms) {
        ServicePressure::Degraded | ServicePressure::Critical => {
            InteractivePriority::InteractiveCritical
        }
        ServicePressure::Healthy | ServicePressure::Recovering => {
            InteractivePriority::InteractivePriority
        }
    }
}

fn recent_mcp_latency_ms_at(now_ms: u64) -> u64 {
    let last_seen = LAST_MCP_SAMPLE_AT_MS.load(Ordering::Relaxed);
    if last_seen == 0 || now_ms.saturating_sub(last_seen) > SERVICE_SAMPLE_TTL_MS {
        return 0;
    }

    LAST_MCP_LATENCY_MS.load(Ordering::Relaxed)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn test_recent_peak_latency_expires() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(900, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(200, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(10_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(10_000, Ordering::Relaxed);

        assert_eq!(recent_peak_latency_ms_at(12_000), 900);
        assert_eq!(recent_peak_latency_ms_at(16_000), 0);
    }

    #[test]
    fn test_recent_peak_latency_uses_max_surface() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(250, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(700, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(20_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(20_000, Ordering::Relaxed);

        assert_eq!(recent_peak_latency_ms_at(21_000), 700);
    }

    #[test]
    fn test_current_pressure_reports_critical_when_sample_is_fresh() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(1_700, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(200, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(30_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(30_000, Ordering::Relaxed);

        assert_eq!(current_pressure_at(31_000), ServicePressure::Critical);
    }

    #[test]
    fn test_current_pressure_enters_recovering_after_ttl() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(1_700, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(200, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(40_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(40_000, Ordering::Relaxed);

        assert_eq!(current_pressure_at(46_000), ServicePressure::Recovering);
    }

    #[test]
    fn test_current_pressure_returns_healthy_after_recovery_window() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(1_700, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(200, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(50_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(50_000, Ordering::Relaxed);

        assert_eq!(current_pressure_at(66_000), ServicePressure::Healthy);
    }

    #[test]
    fn test_current_pressure_stays_recovering_after_low_latency_sample() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(120, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(140, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(70_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(68_000, Ordering::Relaxed);

        assert_eq!(current_pressure_at(71_000), ServicePressure::Recovering);
        assert_eq!(current_pressure_at(84_500), ServicePressure::Healthy);
    }

    #[test]
    fn test_interactive_priority_enters_and_exits_cleanly() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();
        let before = interactive_requests_in_flight();
        mcp_request_started();
        assert!(interactive_priority_active());
        assert!(interactive_requests_in_flight() >= before.saturating_add(1));
        mcp_request_finished();
        assert!(interactive_requests_in_flight() <= before);
    }

    #[test]
    fn test_interactive_priority_does_not_linger_without_inflight_requests() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();
        LAST_INTERACTIVE_AT_MS.store(50_000, Ordering::Relaxed);
        INTERACTIVE_REQUESTS_IN_FLIGHT.store(0, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(50_000, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(50, Ordering::Relaxed);

        assert_eq!(
            current_interactive_priority_at(50_500),
            InteractivePriority::BackgroundNormal
        );
    }

    #[test]
    fn test_vector_runtime_metrics_reports_real_ready_depth_without_graph_floor() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();
        record_vector_ready_queue_depth(0);

        assert_eq!(vector_runtime_metrics().ready_queue_depth_current, 0);
    }

    #[test]
    fn test_vector_ready_replenishment_deficit_tracks_request_and_fulfillment() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_vector_ready_replenishment_requested(3);
        record_vector_ready_replenishment_fulfilled(1);
        let metrics = vector_runtime_metrics();
        assert_eq!(metrics.ready_replenishment_deficit_current, 2);
        assert_eq!(metrics.ready_replenishment_deficit_max, 3);

        record_vector_ready_replenishment_fulfilled(10);
        assert_eq!(
            vector_runtime_metrics().ready_replenishment_deficit_current,
            0
        );
    }

    #[test]
    fn test_quiescent_state_detects_idle_candidate_without_backlog_or_claims() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();
        assert_eq!(
            current_runtime_quiescent_state(0, 0),
            RuntimeQuiescentState::QuiescentCandidate
        );
    }

    #[test]
    fn test_quiescent_scale_only_applies_in_idle_candidate_state() {
        let _guard = TEST_GUARD.lock().unwrap();
        assert_eq!(
            scale_interval_for_quiescent(
                1_000,
                RuntimeQuiescentState::QuiescentCandidate,
                400,
                250,
                60_000
            ),
            4_000
        );
        assert_eq!(
            scale_interval_for_quiescent(
                1_000,
                RuntimeQuiescentState::ActiveBacklog,
                400,
                250,
                60_000
            ),
            1_000
        );
    }

    #[test]
    fn test_runtime_wake_summary_tracks_quiescent_entry_and_exit() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_runtime_wakeup(RuntimeWakeSource::Background, 0, 0);
        let idle = runtime_wake_summary(0, 0);
        assert!(idle.wakeups_last_60s >= 1);
        assert!(idle.last_wakeup_at_ms > 0);
        assert!(idle.quiescent_entered_at_ms > 0);

        record_runtime_wakeup(RuntimeWakeSource::Background, 1, 0);
        let active = runtime_wake_summary(1, 0);
        assert!(active.wakeups_last_60s >= 2);
        assert!(active.last_quiescent_exited_at_ms > 0);
        assert!(active.resume_latency_samples >= 1);
        assert!(active.resume_latency_max_ms <= active.last_quiescent_exited_at_ms);
    }

    #[test]
    fn test_runtime_wake_summary_tracks_useful_resume_after_completion() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_runtime_wakeup(RuntimeWakeSource::Background, 0, 0);
        record_runtime_wakeup(RuntimeWakeSource::Background, 1, 0);
        record_vector_files_completed(1);

        let active = runtime_wake_summary(1, 0);
        assert!(active.useful_resume_latency_samples >= 1);
        assert!(active.last_useful_resume_at_ms > 0);
    }

    #[test]
    fn test_runtime_wake_summary_tracks_quiescent_exit_reason() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_runtime_wakeup(RuntimeWakeSource::Background, 0, 0);
        record_runtime_wakeup(RuntimeWakeSource::Background, 0, 1);

        let active = runtime_wake_summary(0, 1);
        assert_eq!(active.last_quiescent_exit_reason, "active_backlog");
        assert!(active.exit_due_to_active_backlog_total >= 1);
    }

    #[test]
    fn test_runtime_wake_summary_tracks_last_wake_source() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_runtime_wakeup(RuntimeWakeSource::Background, 0, 0);
        record_runtime_wakeup(RuntimeWakeSource::SemanticVector, 0, 1);

        let summary = runtime_wake_summary(0, 1);
        assert_eq!(summary.last_wake_source, "semantic_vector");
        assert!(summary.wake_source_background_total >= 1);
        assert!(summary.wake_source_semantic_vector_total >= 1);
    }

    #[test]
    fn test_vector_stage_latency_summaries_compute_recent_percentiles() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();
        for latency in [5, 10, 15, 20, 25] {
            record_vector_stage_ms(VectorStageKind::Fetch, latency);
        }

        let summaries = vector_runtime_latency_summaries();
        assert_eq!(summaries.fetch.samples, 5);
        assert_eq!(summaries.fetch.p50_ms, 15);
        assert_eq!(summaries.fetch.p95_ms, 25);
        assert_eq!(summaries.fetch.max_ms, 25);
        assert_eq!(summaries.embed.samples, 0);
    }

    #[test]
    fn test_vector_embed_breakdown_totals_accumulate() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();
        record_vector_embed_breakdown(13, 7);
        record_vector_embed_breakdown(17, 11);

        let metrics = vector_runtime_metrics();
        assert_eq!(metrics.embed_transform_ms_total, 30);
        assert_eq!(metrics.embed_export_ms_total, 18);
    }

    #[test]
    fn test_vector_pipeline_stage_telemetry_exposes_tensorrt_ready_names() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_vector_prepare_reply_wait_ms(3);
        record_vector_prepare_send_wait_ms(5);
        record_vector_prepare_queue_wait_ms(7);
        record_vector_gpu_idle_wait_ms(11);
        record_vector_embed_breakdown(13, 17);
        record_vector_stage_ms(VectorStageKind::DbWrite, 19);
        record_vector_persist_send_wait_ms(23);
        record_vector_persist_queue_wait_ms(29);
        record_vector_stage_ms(VectorStageKind::CompletionCheck, 31);
        record_vector_stage_ms(VectorStageKind::MarkDone, 37);
        record_vector_finalize_send_wait_ms(41);
        record_vector_finalize_queue_wait_ms(43);

        let telemetry = vector_pipeline_stage_telemetry();
        assert_eq!(telemetry.prepare_ms_total, 15);
        assert_eq!(telemetry.ready_wait_ms_total, 11);
        assert_eq!(telemetry.inference_ms_total, 13);
        assert_eq!(telemetry.output_extract_ms_total, 17);
        assert_eq!(telemetry.persist_ms_total, 71);
        assert_eq!(telemetry.finalize_ms_total, 152);
    }

    #[test]
    fn test_vector_embed_attempt_tracks_inflight_state_until_finished() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();
        record_vector_embed_attempt(48, 53_652);

        let inflight = vector_runtime_metrics();
        assert_eq!(inflight.embed_attempts_total, 1);
        assert!(inflight.embed_inflight_started_at_ms > 0);
        assert_eq!(inflight.embed_inflight_texts_current, 48);
        assert_eq!(inflight.embed_inflight_text_bytes_current, 53_652);

        record_vector_embed_attempt_finished();

        let finished = vector_runtime_metrics();
        assert_eq!(finished.embed_attempts_total, 1);
        assert_eq!(finished.embed_inflight_started_at_ms, 0);
        assert_eq!(finished.embed_inflight_texts_current, 0);
        assert_eq!(finished.embed_inflight_text_bytes_current, 0);
    }

    #[test]
    fn test_vector_worker_liveness_tracks_start_heartbeat_and_stop() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_vector_worker_started();
        let started = vector_runtime_metrics();
        assert_eq!(started.vector_workers_started_total, 1);
        assert_eq!(started.vector_workers_active_current, 1);
        assert!(started.vector_worker_heartbeat_at_ms > 0);
        assert_eq!(started.vector_lane_state, VectorLaneState::Starting);

        record_vector_worker_heartbeat();
        let heartbeat = vector_runtime_metrics();
        assert_eq!(heartbeat.vector_workers_active_current, 1);
        assert!(heartbeat.vector_worker_heartbeat_at_ms > 0);

        record_vector_worker_stopped();
        let stopped = vector_runtime_metrics();
        assert_eq!(stopped.vector_workers_stopped_total, 1);
        assert_eq!(stopped.vector_workers_active_current, 0);
    }

    #[test]
    fn test_graph_worker_liveness_tracks_start_heartbeat_and_stop() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_graph_worker_started();
        assert_eq!(graph_workers_started_total(), 1);
        assert_eq!(graph_workers_active_current(), 1);
        assert!(graph_worker_heartbeat_at_ms() > 0);

        record_graph_worker_heartbeat();
        assert!(graph_worker_heartbeat_at_ms() > 0);

        record_graph_worker_stopped();
        assert_eq!(graph_workers_active_current(), 0);
    }

    #[test]
    fn test_vector_chunk_embedding_rate_tracks_recent_throughput() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_vector_embed_call(64, 2);

        let metrics = vector_runtime_metrics();
        assert_eq!(metrics.chunks_embedded_total, 64);
        assert_eq!(
            metrics.chunk_embeddings_rate_window_ms,
            VECTOR_EMBED_THROUGHPUT_WINDOW_MS
        );
        assert!(metrics.chunk_embeddings_per_second >= 64.0);
    }

    #[test]
    fn test_vector_lane_state_tracks_restart_success_and_fault() {
        let _guard = TEST_GUARD.lock().unwrap();
        reset_for_tests();

        record_vector_worker_restart();
        record_vector_lane_success();
        let healthy = vector_runtime_metrics();
        assert_eq!(healthy.vector_worker_restarts_total, 1);
        assert_eq!(healthy.vector_lane_state, VectorLaneState::Healthy);
        assert!(healthy.vector_lane_last_success_at_ms > 0);

        record_vector_lane_fault();
        let degraded = vector_runtime_metrics();
        assert_eq!(degraded.vector_lane_state, VectorLaneState::Degraded);
        assert!(degraded.vector_lane_last_fault_at_ms > 0);
    }
}
