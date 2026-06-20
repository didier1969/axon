-- Axon canonical schema — runtime telemetry + audit.
-- Indexer hot-path tables that don't belong to any project's IST namespace.
-- They live in the `axon` schema (PIL-AXO-001 ownership: ist=code,
-- soll=intent, axon=runtime). REQ-AXO-901854 consolidated the former
-- `axon_runtime` schema into `axon` — the `_runtime` suffix was a naming
-- smell (GUI-PRO-108) and runtime truth belongs to the `axon` namespace.
-- Idempotent: safe to re-run on every startup.

CREATE SCHEMA IF NOT EXISTS axon;

-- ── REQ-AXO-901854: one-time consolidation axon_runtime → axon ─────────
-- Move any surviving tables into `axon` (preserving DATA — e.g. the
-- watchman_clock reconciliation cursor), then drop the legacy schema with
-- CASCADE (its functions/views are recreated in `axon` by 08/09 — the
-- dashboard functions resolve their dependencies by name at call time, no
-- hard catalog dependency). No-op once the schema is gone.
DO $$
DECLARE r record;
BEGIN
  IF EXISTS (SELECT 1 FROM information_schema.schemata WHERE schema_name = 'axon_runtime') THEN
    FOR r IN SELECT tablename FROM pg_tables WHERE schemaname = 'axon_runtime' LOOP
      IF EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = 'axon' AND tablename = r.tablename) THEN
        -- axon.<table> already canonical (a prior migration moved it); the
        -- axon_runtime copy is stale leftover — drop it, never the canonical one.
        EXECUTE format('DROP TABLE axon_runtime.%I CASCADE', r.tablename);
      ELSE
        EXECUTE format('ALTER TABLE axon_runtime.%I SET SCHEMA axon', r.tablename);
      END IF;
    END LOOP;
    DROP SCHEMA axon_runtime CASCADE;
  END IF;
END $$;

-- ── Tables ───────────────────────────────────────────────────────────

-- Vector lane fatal-fault log. Captures stage + reason + provider so
-- the optimiser can rate-limit retries on a specific lane/worker.
CREATE TABLE IF NOT EXISTS axon.VectorWorkerFault (
    fault_id         TEXT PRIMARY KEY,
    lane             TEXT,
    worker_id        BIGINT,
    fatal_stage      TEXT,
    fatal_reason_raw TEXT,
    fatal_class      TEXT,
    provider         TEXT,
    batch_id         TEXT,
    texts_count      BIGINT NOT NULL DEFAULT 0,
    input_bytes      BIGINT NOT NULL DEFAULT 0,
    vram_used_mb     BIGINT NOT NULL DEFAULT 0,
    occurred_at_ms   BIGINT,
    restart_attempt  BIGINT NOT NULL DEFAULT 0
);

-- Per-lane current state (KV by lane). Touched by both the embedder
-- and the optimiser.
CREATE TABLE IF NOT EXISTS axon.VectorLaneState (
    lane                TEXT PRIMARY KEY,
    state               TEXT,
    reason              TEXT,
    updated_at_ms       BIGINT,
    worker_id           BIGINT,
    restart_attempt     BIGINT NOT NULL DEFAULT 0,
    last_success_at_ms  BIGINT,
    last_fault_id       TEXT
);

-- Persist outbox bridging GPU embedding output and the PG ChunkEmbedding
-- writer. Lease columns implement single-writer claim semantics.
CREATE TABLE IF NOT EXISTS axon.VectorPersistOutbox (
    outbox_id              TEXT PRIMARY KEY,
    run_id                 TEXT,
    model_id               TEXT,
    status                 TEXT   NOT NULL DEFAULT 'queued',
    attempts               BIGINT NOT NULL DEFAULT 0,
    queued_at_ms           BIGINT,
    claimed_at_ms          BIGINT,
    completed_at_ms        BIGINT,
    last_error_reason      TEXT,
    claim_token            TEXT,
    lease_heartbeat_at_ms  BIGINT,
    lease_owner            TEXT,
    lease_epoch            BIGINT NOT NULL DEFAULT 0,
    chunk_count            BIGINT NOT NULL DEFAULT 0,
    file_count             BIGINT NOT NULL DEFAULT 0,
    input_bytes            BIGINT NOT NULL DEFAULT 0,
    fetch_ms               BIGINT NOT NULL DEFAULT 0,
    embed_ms               BIGINT NOT NULL DEFAULT 0,
    payload_json           TEXT
);

-- Bench / production batch-run telemetry. Lower-case identifier matches
-- the column naming used by axon-bench-pipeline-v2 CSV exporters.
CREATE TABLE IF NOT EXISTS axon.vector_batch_run (
    run_id                       TEXT   PRIMARY KEY,
    prepare_started_at_ms        BIGINT NOT NULL DEFAULT 0,
    prepare_finished_at_ms       BIGINT NOT NULL DEFAULT 0,
    ready_enqueued_at_ms         BIGINT NOT NULL DEFAULT 0,
    started_at_ms                BIGINT NOT NULL,
    finished_at_ms               BIGINT NOT NULL,
    gpu_started_at_ms            BIGINT NOT NULL DEFAULT 0,
    gpu_finished_at_ms           BIGINT NOT NULL DEFAULT 0,
    persist_enqueued_at_ms       BIGINT NOT NULL DEFAULT 0,
    persist_started_at_ms        BIGINT NOT NULL DEFAULT 0,
    persist_finished_at_ms       BIGINT NOT NULL DEFAULT 0,
    finalize_enqueued_at_ms      BIGINT NOT NULL DEFAULT 0,
    finalize_finished_at_ms      BIGINT NOT NULL DEFAULT 0,
    wall_ms                      BIGINT NOT NULL,
    instance_kind                TEXT   NOT NULL,
    runtime_mode                 TEXT   NOT NULL,
    provider                     TEXT   NOT NULL,
    provider_effective           TEXT   NOT NULL,
    runner_kind                  TEXT   NOT NULL DEFAULT '',
    model_id                     TEXT   NOT NULL,
    vector_workers               BIGINT NOT NULL,
    graph_workers                BIGINT NOT NULL,
    ready_queue_depth            BIGINT NOT NULL,
    prepare_pipeline_depth       BIGINT NOT NULL,
    prepare_workers_per_vector   BIGINT NOT NULL,
    micro_batch_max_items        BIGINT NOT NULL,
    micro_batch_max_total_tokens BIGINT NOT NULL,
    max_embed_batch_bytes        BIGINT NOT NULL,
    chunk_count                  BIGINT NOT NULL,
    file_count                   BIGINT NOT NULL,
    input_bytes                  BIGINT NOT NULL,
    total_tokens                 BIGINT NOT NULL DEFAULT 0
);

-- Cross-process visibility for the GPU embedder sleep/wake state. The
-- indexer (writer of public.ChunkEmbedding) owns the singleton that
-- actually loads / drops the TensorRT session; the brain (MCP server)
-- needs to observe that state without sharing the process. Each role
-- UPSERTs its row every `heartbeat_ms`; readers should treat rows older
-- than ~2x heartbeat_ms as stale.
CREATE TABLE IF NOT EXISTS axon.EmbedderLifecycleHeartbeat (
    process_role   TEXT   PRIMARY KEY,   -- 'indexer' | 'brain'
    phase          TEXT   NOT NULL,      -- 'ready' | 'sleeping'
    last_used_ms   BIGINT NOT NULL,
    wake_count     BIGINT NOT NULL DEFAULT 0,
    sleep_count    BIGINT NOT NULL DEFAULT 0,
    pending_count  BIGINT NOT NULL DEFAULT 0,
    heartbeat_ms   BIGINT NOT NULL
);
-- DEC-AXO-901626: the indexer OBSERVES ITS OWN GPU footprint (nvidia-smi on
-- its own pid) and publishes the binary verdict here. Observation happens
-- where the observed thing lives — the brain only READS this row, never
-- cross-references a remote pid. `compute` ∈ {GPU,CPU}; `compute_source` ∈
-- {nvidia_smi,unknown}. `build_id` lets the brain confirm the paired indexer
-- runs the same release. Idempotent for existing instances.
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS compute        TEXT;
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS compute_source TEXT;
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS build_id       TEXT;

-- REQ-AXO-902047 slice 1b: cross-process B3 (embedding persist) health. The
-- indexer publishes its in-RAM `pipeline_v2::stage_health::b3_health()`
-- snapshot every heartbeat tick so the brain's `embedding_status` surfaces the
-- real PG error + systemic-failure verdict in ONE MCP call, without log access
-- (the REQ-AXO-902046 incident took gdb + 4 h because the error was
-- process-local). `b3_last_error` is the full anyhow chain (root PG message +
-- SQLSTATE), deduped by signature. Idempotent for existing instances.
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS b3_consecutive_failures    BIGINT NOT NULL DEFAULT 0;
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS b3_total_failures          BIGINT NOT NULL DEFAULT 0;
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS b3_total_successes         BIGINT NOT NULL DEFAULT 0;
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS b3_last_error              TEXT;
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS b3_last_error_count        BIGINT NOT NULL DEFAULT 0;
ALTER TABLE axon.EmbedderLifecycleHeartbeat
    ADD COLUMN IF NOT EXISTS b3_last_error_last_seen_ms BIGINT NOT NULL DEFAULT 0;

-- REQ-AXO-901854: cross-process indexer runtime truth. Rates/workers/queues
-- were previously sourced from a brain-LOCAL telemetry snapshot (empty under
-- brain_only — the indexer, not the brain, runs the pipeline). The indexer
-- now publishes values observed at the OWNER every heartbeat tick (~5 s, one
-- UPSERT/role, NOT per-file); the brain READS this row and projects it
-- (PIL-AXO-001 — one canonical truth, observed at owner). One row per
-- process_role, like EmbedderLifecycleHeartbeat.
--
-- CANONICAL SOURCES ONLY (every column resolves to the real pipeline_v2 owner
-- state, never a brain-local proxy or a dead v1 counter):
--   * graph_workers_active        = Σ inflight of pipeline A stages A1/A2/A3
--                                   (pipeline_v2 StageMetrics — busy graph workers).
--   * chunk_embeddings_per_second = the indexer's own embed-rate accessor.
--   * in_flight_*                 = RAM in-flight registry (REQ-AXO-901919,
--                                   commit 0b860166) — oldest item + count.
--   * *_queue_depth               = pipeline_v2 owner channel depths.
-- A cumulative "workers_started" gauge is intentionally NOT published: with
-- fixed pipeline_v2 worker pools there is no canonical owner source for it
-- (the legacy GRAPH_WORKERS_STARTED_TOTAL counter is dead under pipeline_v2),
-- so publishing it would be a non-canonical output.
CREATE TABLE IF NOT EXISTS axon.indexer_runtime_truth (
    process_role                TEXT   PRIMARY KEY,        -- 'indexer'
    heartbeat_ms                BIGINT NOT NULL,           -- publish wall-clock; readers gate on freshness
    graph_workers_active        BIGINT NOT NULL DEFAULT 0, -- Σ inflight of pipeline A (busy graph workers)
    chunk_embeddings_per_second DOUBLE PRECISION NOT NULL DEFAULT 0,  -- pipeline B embed throughput
    -- REQ-AXO-901919 in-flight gauge (RAM registry, commit 0b860166)
    in_flight_count             BIGINT NOT NULL DEFAULT 0, -- items currently in flight across A
    oldest_in_flight_path       TEXT,                      -- path of the earliest-started item (NULL when idle)
    oldest_in_flight_stage      TEXT,                      -- its stage (A1/A2/A3)
    oldest_in_flight_age_ms     BIGINT NOT NULL DEFAULT 0, -- how long it has been in flight
    -- REQ-AXO-901854 queue depths (pipeline_v2 owner gauges; ready_queue_chunks
    -- is the dashboard funnel's canonical backlog signal)
    ready_queue_chunks          BIGINT NOT NULL DEFAULT 0, -- chunks queued A→B (ready queue)
    persist_queue_depth         BIGINT NOT NULL DEFAULT 0  -- queued VectorPersistOutbox rows
);
-- Additive column evolution for rows migrated from the foundation slice.
ALTER TABLE axon.indexer_runtime_truth
    ADD COLUMN IF NOT EXISTS in_flight_count         BIGINT NOT NULL DEFAULT 0;
ALTER TABLE axon.indexer_runtime_truth
    ADD COLUMN IF NOT EXISTS oldest_in_flight_path   TEXT;
ALTER TABLE axon.indexer_runtime_truth
    ADD COLUMN IF NOT EXISTS oldest_in_flight_stage  TEXT;
ALTER TABLE axon.indexer_runtime_truth
    ADD COLUMN IF NOT EXISTS oldest_in_flight_age_ms BIGINT NOT NULL DEFAULT 0;
ALTER TABLE axon.indexer_runtime_truth
    ADD COLUMN IF NOT EXISTS ready_queue_chunks      BIGINT NOT NULL DEFAULT 0;
ALTER TABLE axon.indexer_runtime_truth
    ADD COLUMN IF NOT EXISTS persist_queue_depth     BIGINT NOT NULL DEFAULT 0;

-- REQ-AXO-901893: Watchman reconciliation cursor, one row per watched root.
-- The indexer threads `clock_json` back into the next `since` subscription so
-- Watchman returns the exact cumulative delta since the last checkpoint (or a
-- safe full rebuild when `is_fresh = true`). Persisted AFTER a batch is fed to
-- pipeline A (checkpoint-after-commit): a crash between feed and checkpoint
-- replays the batch on restart (idempotent via the IndexedFile dedup cache) —
-- it can never SKIP a delta. Replaces the inotify event stream whose dropped
-- events were unrecoverable. `clock` is an opaque Watchman clockspec string
-- (`c:PID:N` / SCM-aware fat clock) — stored verbatim, never parsed.
CREATE TABLE IF NOT EXISTS axon.watchman_clock (
    root        TEXT        PRIMARY KEY,            -- absolute resolved project root
    clock_json  JSONB       NOT NULL,               -- serialized watchman_client::Clock
    is_fresh    BOOLEAN     NOT NULL DEFAULT false,  -- last result was a fresh-instance rebuild
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Indexes ──────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS vector_persist_outbox_status_idx
    ON axon.VectorPersistOutbox (status, queued_at_ms);
CREATE INDEX IF NOT EXISTS vector_persist_outbox_lease_idx
    ON axon.VectorPersistOutbox (lease_owner, lease_heartbeat_at_ms);
CREATE INDEX IF NOT EXISTS vector_persist_outbox_claim_idx
    ON axon.VectorPersistOutbox (claim_token);

CREATE INDEX IF NOT EXISTS vector_batch_run_kind_started_idx
    ON axon.vector_batch_run (instance_kind, runtime_mode, started_at_ms);
