-- Axon canonical schema — runtime telemetry + audit (DEC-AXO-082).
-- Tables consumed by the indexer hot path that don't belong to any
-- project's IST namespace. Kept in `axon_runtime` so they're isolated
-- from SOLL + per-project schemas.

CREATE SCHEMA IF NOT EXISTS axon_runtime;

-- ── OptimizerDecisionLog (post-batch optimisation telemetry) ─────────
CREATE TABLE IF NOT EXISTS axon_runtime.OptimizerDecisionLog (
    decision_id TEXT PRIMARY KEY,
    at_ms BIGINT,
    mode TEXT,
    host_snapshot_json TEXT,
    policy_snapshot_json TEXT,
    signal_snapshot_json TEXT,
    analytics_snapshot_json TEXT,
    action_profile_id TEXT,
    decision_json TEXT,
    constraints_triggered_json TEXT,
    would_apply BOOLEAN,
    applied BOOLEAN,
    evaluation_window_start_ms BIGINT,
    evaluation_window_end_ms BIGINT
);

-- ── Vector lane fault + state + outbox (REQ-AXO-262/251/250) ─────────
CREATE TABLE IF NOT EXISTS axon_runtime.VectorWorkerFault (
    fault_id TEXT PRIMARY KEY,
    lane TEXT,
    worker_id BIGINT,
    fatal_stage TEXT,
    fatal_reason_raw TEXT,
    fatal_class TEXT,
    provider TEXT,
    batch_id TEXT,
    texts_count BIGINT NOT NULL DEFAULT 0,
    input_bytes BIGINT NOT NULL DEFAULT 0,
    vram_used_mb BIGINT NOT NULL DEFAULT 0,
    occurred_at_ms BIGINT,
    restart_attempt BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS axon_runtime.VectorLaneState (
    lane TEXT PRIMARY KEY,
    state TEXT,
    reason TEXT,
    updated_at_ms BIGINT,
    worker_id BIGINT,
    restart_attempt BIGINT NOT NULL DEFAULT 0,
    last_success_at_ms BIGINT,
    last_fault_id TEXT
);

CREATE TABLE IF NOT EXISTS axon_runtime.VectorPersistOutbox (
    outbox_id TEXT PRIMARY KEY,
    run_id TEXT,
    model_id TEXT,
    status TEXT NOT NULL DEFAULT 'queued',
    attempts BIGINT NOT NULL DEFAULT 0,
    queued_at_ms BIGINT,
    claimed_at_ms BIGINT,
    completed_at_ms BIGINT,
    last_error_reason TEXT,
    claim_token TEXT,
    lease_heartbeat_at_ms BIGINT,
    lease_owner TEXT,
    lease_epoch BIGINT NOT NULL DEFAULT 0,
    chunk_count BIGINT NOT NULL DEFAULT 0,
    file_count BIGINT NOT NULL DEFAULT 0,
    input_bytes BIGINT NOT NULL DEFAULT 0,
    fetch_ms BIGINT NOT NULL DEFAULT 0,
    embed_ms BIGINT NOT NULL DEFAULT 0,
    payload_json TEXT
);

CREATE INDEX IF NOT EXISTS vector_persist_outbox_status_idx
    ON axon_runtime.VectorPersistOutbox (status, queued_at_ms);
-- DEC-AXO-082 follow-up: claim-by-token lookups + lease-heartbeat
-- sweeps benefit from these.
CREATE INDEX IF NOT EXISTS vector_persist_outbox_lease_idx
    ON axon_runtime.VectorPersistOutbox (lease_owner, lease_heartbeat_at_ms);
CREATE INDEX IF NOT EXISTS vector_persist_outbox_claim_idx
    ON axon_runtime.VectorPersistOutbox (claim_token);

-- ── vector_batch_run (dev-bench telemetry) ───────────────────────────
-- Lower-case identifier matches the DuckDB definition for grep
-- continuity.
CREATE TABLE IF NOT EXISTS axon_runtime.vector_batch_run (
    run_id TEXT PRIMARY KEY,
    prepare_started_at_ms BIGINT NOT NULL DEFAULT 0,
    prepare_finished_at_ms BIGINT NOT NULL DEFAULT 0,
    ready_enqueued_at_ms BIGINT NOT NULL DEFAULT 0,
    started_at_ms BIGINT NOT NULL,
    finished_at_ms BIGINT NOT NULL,
    gpu_started_at_ms BIGINT NOT NULL DEFAULT 0,
    gpu_finished_at_ms BIGINT NOT NULL DEFAULT 0,
    persist_enqueued_at_ms BIGINT NOT NULL DEFAULT 0,
    persist_started_at_ms BIGINT NOT NULL DEFAULT 0,
    persist_finished_at_ms BIGINT NOT NULL DEFAULT 0,
    finalize_enqueued_at_ms BIGINT NOT NULL DEFAULT 0,
    finalize_finished_at_ms BIGINT NOT NULL DEFAULT 0,
    wall_ms BIGINT NOT NULL,
    instance_kind TEXT NOT NULL,
    runtime_mode TEXT NOT NULL,
    provider TEXT NOT NULL,
    provider_effective TEXT NOT NULL,
    runner_kind TEXT NOT NULL DEFAULT '',
    model_id TEXT NOT NULL,
    vector_workers BIGINT NOT NULL,
    graph_workers BIGINT NOT NULL,
    ready_queue_depth BIGINT NOT NULL,
    prepare_pipeline_depth BIGINT NOT NULL,
    prepare_workers_per_vector BIGINT NOT NULL,
    micro_batch_max_items BIGINT NOT NULL,
    micro_batch_max_total_tokens BIGINT NOT NULL,
    max_embed_batch_bytes BIGINT NOT NULL,
    chunk_count BIGINT NOT NULL,
    file_count BIGINT NOT NULL,
    input_bytes BIGINT NOT NULL,
    total_tokens BIGINT NOT NULL DEFAULT 0
);

-- DEC-AXO-082 follow-up: bench analysis queries usually filter on
-- (instance_kind, runtime_mode) over time windows.
CREATE INDEX IF NOT EXISTS vector_batch_run_kind_started_idx
    ON axon_runtime.vector_batch_run (instance_kind, runtime_mode, started_at_ms);
