use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
static VECTOR_READY_QUEUE_DEPTH_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_READY_QUEUE_DEPTH_MAX: AtomicU64 = AtomicU64::new(0);
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
static VECTOR_WORKERS_STARTED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_WORKERS_STOPPED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VECTOR_WORKERS_ACTIVE_CURRENT: AtomicU64 = AtomicU64::new(0);
static VECTOR_WORKER_HEARTBEAT_AT_MS: AtomicU64 = AtomicU64::new(0);
static VECTOR_STAGE_LATENCY_WINDOWS: OnceLock<Mutex<VectorStageLatencyWindows>> = OnceLock::new();
static VECTOR_BACKLOG_SIGNAL: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();

const SERVICE_SAMPLE_TTL_MS: u64 = 5_000;
const SERVICE_RECOVERY_WINDOW_MS: u64 = 15_000;
const INTERACTIVE_PRIORITY_IDLE_MS: u64 = 2_500;
const VECTOR_STAGE_WINDOW_CAPACITY: usize = 256;

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
pub enum VectorStageKind {
    Fetch,
    Embed,
    DbWrite,
    CompletionCheck,
    MarkDone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    pub ready_queue_depth_current: u64,
    pub ready_queue_depth_max: u64,
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
    pub vector_workers_started_total: u64,
    pub vector_workers_stopped_total: u64,
    pub vector_workers_active_current: u64,
    pub vector_worker_heartbeat_at_ms: u64,
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
    VECTOR_EMBED_ATTEMPTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_STARTED_AT_MS.store(now_ms(), Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXTS_CURRENT.store(texts, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXT_BYTES_CURRENT.store(text_bytes, Ordering::Relaxed);
}

pub fn record_vector_embed_attempt_finished() {
    VECTOR_EMBED_INFLIGHT_STARTED_AT_MS.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXTS_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_EMBED_INFLIGHT_TEXT_BYTES_CURRENT.store(0, Ordering::Relaxed);
}

pub fn record_vector_worker_started() {
    VECTOR_WORKERS_STARTED_TOTAL.fetch_add(1, Ordering::Relaxed);
    VECTOR_WORKERS_ACTIVE_CURRENT.fetch_add(1, Ordering::Relaxed);
    VECTOR_WORKER_HEARTBEAT_AT_MS.store(now_ms(), Ordering::Relaxed);
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

pub fn notify_vector_backlog_activity() {
    let (lock, cvar) = VECTOR_BACKLOG_SIGNAL.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let mut generation = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    *generation = generation.saturating_add(1);
    cvar.notify_all();
}

pub fn wait_for_vector_backlog_signal(timeout: Duration) {
    let (lock, cvar) = VECTOR_BACKLOG_SIGNAL.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let generation = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    let observed = *generation;
    let _ = cvar.wait_timeout_while(generation, timeout, |current| *current == observed);
}

pub fn record_vector_files_completed(count: u64) {
    VECTOR_FILES_COMPLETED_TOTAL.fetch_add(count, Ordering::Relaxed);
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

pub fn record_vector_finalize_queue_depth(depth: u64) {
    VECTOR_FINALIZE_QUEUE_DEPTH_CURRENT.store(depth, Ordering::Relaxed);
    update_atomic_max(&VECTOR_FINALIZE_QUEUE_DEPTH_MAX, depth);
}

pub fn record_vector_ready_queue_depth(depth: u64) {
    VECTOR_READY_QUEUE_DEPTH_CURRENT.store(depth, Ordering::Relaxed);
    update_atomic_max(&VECTOR_READY_QUEUE_DEPTH_MAX, depth);
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

pub fn vector_runtime_metrics() -> VectorRuntimeMetrics {
    VectorRuntimeMetrics {
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
        ready_queue_depth_current: VECTOR_READY_QUEUE_DEPTH_CURRENT.load(Ordering::Relaxed),
        ready_queue_depth_max: VECTOR_READY_QUEUE_DEPTH_MAX.load(Ordering::Relaxed),
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
        vector_workers_started_total: VECTOR_WORKERS_STARTED_TOTAL.load(Ordering::Relaxed),
        vector_workers_stopped_total: VECTOR_WORKERS_STOPPED_TOTAL.load(Ordering::Relaxed),
        vector_workers_active_current: VECTOR_WORKERS_ACTIVE_CURRENT.load(Ordering::Relaxed),
        vector_worker_heartbeat_at_ms: VECTOR_WORKER_HEARTBEAT_AT_MS.load(Ordering::Relaxed),
    }
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
    VECTOR_READY_QUEUE_DEPTH_CURRENT.store(0, Ordering::Relaxed);
    VECTOR_READY_QUEUE_DEPTH_MAX.store(0, Ordering::Relaxed);
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
    *vector_stage_latency_windows()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = VectorStageLatencyWindows::default();
}

fn vector_stage_latency_windows() -> &'static Mutex<VectorStageLatencyWindows> {
    VECTOR_STAGE_LATENCY_WINDOWS.get_or_init(|| Mutex::new(VectorStageLatencyWindows::default()))
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

        record_vector_worker_heartbeat();
        let heartbeat = vector_runtime_metrics();
        assert_eq!(heartbeat.vector_workers_active_current, 1);
        assert!(heartbeat.vector_worker_heartbeat_at_ms > 0);

        record_vector_worker_stopped();
        let stopped = vector_runtime_metrics();
        assert_eq!(stopped.vector_workers_stopped_total, 1);
        assert_eq!(stopped.vector_workers_active_current, 0);
    }
}
