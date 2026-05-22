use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileVectorizationWork {
    pub file_path: String,
    pub resumed_after_interactive_pause: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileVectorizationLeaseSnapshot {
    pub file_path: String,
    pub claim_token: String,
    pub lease_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLifecycleEvent {
    pub file_path: String,
    pub project_code: String,
    pub stage: String,
    pub status: String,
    pub reason: Option<String>,
    pub at_ms: i64,
    pub worker_id: Option<i64>,
    pub trace_id: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorBatchRun {
    pub run_id: String,
    #[serde(default)]
    pub prepare_started_at_ms: i64,
    #[serde(default)]
    pub prepare_finished_at_ms: i64,
    #[serde(default)]
    pub ready_enqueued_at_ms: i64,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    #[serde(default)]
    pub gpu_started_at_ms: i64,
    #[serde(default)]
    pub gpu_finished_at_ms: i64,
    #[serde(default)]
    pub persist_enqueued_at_ms: i64,
    #[serde(default)]
    pub persist_started_at_ms: i64,
    #[serde(default)]
    pub persist_finished_at_ms: i64,
    #[serde(default)]
    pub finalize_enqueued_at_ms: i64,
    #[serde(default)]
    pub finalize_finished_at_ms: i64,
    pub provider: String,
    #[serde(default)]
    pub runner_kind: String,
    pub model_id: String,
    pub chunk_count: u64,
    pub file_count: u64,
    pub input_bytes: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub max_item_tokens: u64,
    #[serde(default)]
    pub avg_item_tokens: f64,
    #[serde(default)]
    pub micro_batch_count: u64,
    #[serde(default)]
    pub max_micro_batch_tokens: u64,
    #[serde(default)]
    pub avg_micro_batch_tokens: f64,
    #[serde(default)]
    pub effective_vector_workers_admitted: u64,
    #[serde(default)]
    pub ready_queue_depth_at_gpu_start: u64,
    #[serde(default)]
    pub prepare_inflight_at_gpu_start: u64,
    #[serde(default)]
    pub ready_queue_chunks_at_gpu_start: u64,
    #[serde(default)]
    pub prepare_inflight_chunks_at_gpu_start: u64,
    #[serde(default)]
    pub vector_worker_admission_reason: String,
    #[serde(default)]
    pub allowed_gpu_workers: u64,
    #[serde(default)]
    pub batch_wait_for_ready_ms: u64,
    #[serde(default)]
    pub persist_queue_wait_ms: u64,
    #[serde(default)]
    pub finalize_queue_wait_ms: u64,
    #[serde(default)]
    pub batch_lane: String,
    #[serde(default)]
    pub batch_shape: String,
    #[serde(default)]
    pub lane_small_max_tokens: u64,
    #[serde(default)]
    pub lane_medium_max_tokens: u64,
    pub fetch_ms: u64,
    pub embed_ms: u64,
    pub db_write_ms: u64,
    pub mark_done_ms: u64,
    pub success: bool,
    pub error_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorWorkerFault {
    pub fault_id: String,
    pub lane: String,
    pub worker_id: i64,
    pub fatal_stage: String,
    pub fatal_reason_raw: String,
    pub fatal_class: String,
    pub provider: String,
    pub batch_id: Option<String>,
    pub texts_count: u64,
    pub input_bytes: u64,
    pub vram_used_mb: u64,
    pub occurred_at_ms: i64,
    pub restart_attempt: u64,
}

/// REQ-AXO-91572 option B — cross-process embedder state row read from
/// `axon_runtime.EmbedderLifecycleHeartbeat`. The brain consumes
/// indexer-written rows in `embedding_status` so MCP callers see the
/// real runtime state instead of the brain's own (unused) singleton.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbedderLifecycleHeartbeatRecord {
    pub process_role: String,
    pub phase: String,
    pub last_used_ms: i64,
    pub wake_count: i64,
    pub sleep_count: i64,
    pub pending_count: i64,
    pub heartbeat_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorLaneStateRecord {
    pub lane: String,
    pub state: String,
    pub reason: Option<String>,
    pub updated_at_ms: i64,
    pub worker_id: Option<i64>,
    pub restart_attempt: u64,
    pub last_success_at_ms: Option<i64>,
    pub last_fault_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorPersistOutboxUpdate {
    pub chunk_id: String,
    pub source_hash: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorPersistOutboxPayload {
    pub updates: Vec<VectorPersistOutboxUpdate>,
    pub completed_works: Vec<FileVectorizationWork>,
    pub completed_lease_snapshots: Vec<FileVectorizationLeaseSnapshot>,
    pub batch_run: VectorBatchRun,
}

#[derive(Debug, Clone)]
pub struct VectorPersistOutboxWork {
    pub outbox_id: String,
    pub payload: VectorPersistOutboxPayload,
}

#[derive(Debug, Clone, Default)]
pub struct IgnoreReconcileStats {
    pub scanned: usize,
    pub newly_ignored: usize,
    pub newly_included: usize,
    pub dry_run: bool,
}
