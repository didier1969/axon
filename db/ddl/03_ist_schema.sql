-- Axon canonical schema — IST (Indexed Symbol Tree) (DEC-AXO-082).
-- Multi-project IST: every table lives in `public` with a project_code
-- column to scope rows (CPT-AXO-039 supersedure, 2026-05-08).
-- Idempotent: safe to re-run on every startup.
--
-- Embedding dimension is hard-coded to 1024 (BGE-Large 1024-d, see
-- src/axon-core/src/embedding_contract.rs::DIMENSION). Any change to
-- the model must update this file AND the Rust constant in lockstep.

-- ── Core IST tables ──────────────────────────────────────────────────

-- public.File — legacy state machine. Retained for the v2 cut-over
-- window (REQ-AXO-289 S7→S8); empty rows expected in pure-v2 indexer
-- runs because the new pipeline writes IndexedFile instead.
CREATE TABLE IF NOT EXISTS public.File (
    path TEXT PRIMARY KEY,
    project_code TEXT NOT NULL DEFAULT '',
    status TEXT,
    size BIGINT,
    priority BIGINT,
    mtime BIGINT,
    worker_id BIGINT,
    trace_id TEXT,
    needs_reindex BOOLEAN NOT NULL DEFAULT FALSE,
    last_error_reason TEXT,
    status_reason TEXT,
    defer_count BIGINT NOT NULL DEFAULT 0,
    last_deferred_at_ms BIGINT,
    file_stage TEXT NOT NULL DEFAULT 'promoted',
    graph_ready BOOLEAN NOT NULL DEFAULT FALSE,
    vector_ready BOOLEAN NOT NULL DEFAULT FALSE,
    first_seen_at_ms BIGINT,
    indexing_started_at_ms BIGINT,
    graph_ready_at_ms BIGINT,
    vectorization_started_at_ms BIGINT,
    vector_ready_at_ms BIGINT,
    last_state_change_at_ms BIGINT,
    last_error_at_ms BIGINT
);

-- public.IndexedFile — REQ-AXO-289 streaming pipeline v2 watcher
-- filter. 3 columns only: path PK, content_hash for change detection,
-- last_seen_ms for hygiene. NO status machine, NO worker_id, NO claim
-- state.
CREATE TABLE IF NOT EXISTS public.IndexedFile (
    path TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    last_seen_ms BIGINT NOT NULL
);

-- public.Symbol — code symbols (functions, types, modules, ...).
CREATE TABLE IF NOT EXISTS public.Symbol (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT,
    tested BOOLEAN NOT NULL DEFAULT FALSE,
    is_public BOOLEAN NOT NULL DEFAULT FALSE,
    is_nif BOOLEAN NOT NULL DEFAULT FALSE,
    is_unsafe BOOLEAN NOT NULL DEFAULT FALSE,
    project_code TEXT NOT NULL DEFAULT '',
    embedding vector(1024)
);

-- public.Chunk — sliced code blocks (1 symbol → 1+ chunks).
-- REQ-AXO-292 adds the FTS GENERATED column + GIN index below.
CREATE TABLE IF NOT EXISTS public.Chunk (
    id TEXT PRIMARY KEY,
    source_type TEXT,
    source_id TEXT,
    project_code TEXT NOT NULL DEFAULT '',
    file_path TEXT,
    kind TEXT,
    content TEXT,
    content_hash TEXT,
    start_line BIGINT,
    end_line BIGINT,
    chunk_part_index BIGINT,
    chunk_part_count BIGINT,
    chunk_path TEXT
);

-- REQ-AXO-292 — FTS GENERATED column + GIN index. PG recomputes on
-- every Chunk INSERT/UPDATE that touches content / chunk_path / kind /
-- file_path. Weights mirror the hybrid retrieval plan: title-tier
-- (chunk_path + kind) at A, body (content) at B, path metadata
-- (file_path) at C.
ALTER TABLE public.Chunk
    ADD COLUMN IF NOT EXISTS content_tsv tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', coalesce(chunk_path, '')), 'A') ||
        setweight(to_tsvector('simple', coalesce(kind, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(content, '')), 'B') ||
        setweight(to_tsvector('simple', coalesce(file_path, '')), 'C')
    ) STORED;

CREATE INDEX IF NOT EXISTS idx_chunk_content_tsv
    ON public.Chunk USING GIN(content_tsv);
CREATE INDEX IF NOT EXISTS idx_chunk_project_code
    ON public.Chunk(project_code);

-- public.ChunkEmbedding — pgvector storage (1024-d cosine, HNSW).
CREATE TABLE IF NOT EXISTS public.ChunkEmbedding (
    chunk_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    source_hash TEXT NOT NULL,
    embedding vector(1024) NOT NULL,
    embedded_at_ms BIGINT NOT NULL,
    PRIMARY KEY (chunk_id, model_id)
);

-- ── Relation tables — REINTRODUCED (MIL-AXO-017, REQ-AXO-295) ────────
-- Historical context: REQ-AXO-216 (Stop A) dropped the 5 per-type SQL
-- relation tables (CALLS / CALLS_NIF / CONTAINS / IMPACTS /
-- SUBSTANTIATES) in favor of AGE elabels in `axon_graph`. The
-- AGE-based approach is being retired by MIL-AXO-017 (DEC-AXO-083):
-- AGE is replaced by this single unified `public.Edge` table backed by
-- composite B-tree indexes + GIN metadata. Graph traversal patterns
-- will live as a SQL function library (db/ddl/04_graph_functions.sql,
-- REQ-AXO-296) wrapping WITH RECURSIVE queries.
--
-- This slice (REQ-AXO-295) introduces the schema only. The A3 writer
-- starts dual-writing AGE + public.Edge in REQ-AXO-298 (transitional);
-- AGE is dropped entirely in REQ-AXO-301.
CREATE TABLE IF NOT EXISTS public.Edge (
    source_id     TEXT NOT NULL,
    target_id     TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    project_code  TEXT NOT NULL DEFAULT '',
    metadata      JSONB,
    created_at_ms BIGINT NOT NULL,
    PRIMARY KEY (source_id, target_id, relation_type, project_code)
);

-- Forward walks: source → target via relation_type (impact, blast_radius).
CREATE INDEX IF NOT EXISTS edge_fwd_idx
    ON public.Edge (source_id, relation_type, target_id);
-- Reverse walks: target ← source (callers_of, why_chain).
CREATE INDEX IF NOT EXISTS edge_rev_idx
    ON public.Edge (target_id, relation_type, source_id);
-- Project-scoped scans (multi-project queries).
CREATE INDEX IF NOT EXISTS edge_proj_idx
    ON public.Edge (project_code, relation_type);
-- Metadata filtering (anomaly detection, scoped queries on attributes).
CREATE INDEX IF NOT EXISTS edge_metadata_idx
    ON public.Edge USING GIN (metadata jsonb_path_ops);

-- ── Queues (legacy autonomous ingestor, retired with REQ-AXO-289 S7) ─
CREATE TABLE IF NOT EXISTS public.FileVectorizationQueue (
    file_path TEXT PRIMARY KEY,
    project_code TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'queued',
    status_reason TEXT,
    attempts BIGINT NOT NULL DEFAULT 0,
    queued_at BIGINT,
    last_error_reason TEXT,
    last_attempt_at BIGINT,
    next_eligible_at_ms BIGINT,
    interactive_pause_count BIGINT NOT NULL DEFAULT 0,
    claim_token TEXT,
    claimed_at_ms BIGINT,
    lease_heartbeat_at_ms BIGINT,
    lease_owner TEXT,
    lease_epoch BIGINT NOT NULL DEFAULT 0,
    persist_started_at_ms BIGINT
);

CREATE TABLE IF NOT EXISTS public.GraphProjectionQueue (
    anchor_type TEXT NOT NULL,
    anchor_id TEXT NOT NULL,
    radius BIGINT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'queued',
    attempts BIGINT NOT NULL DEFAULT 0,
    queued_at BIGINT,
    last_error_reason TEXT,
    last_attempt_at BIGINT,
    PRIMARY KEY (anchor_type, anchor_id, radius)
);

-- ── Indexer-hot-path tables (DDL parity with DuckDB layout) ──────────
-- Project registry: ingress promoter writes one row per project_code.
CREATE TABLE IF NOT EXISTS public.Project (
    name TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS public.EmbeddingModel (
    id TEXT PRIMARY KEY,
    kind TEXT,
    model_name TEXT,
    dimension BIGINT,
    version TEXT,
    created_at BIGINT
);

-- Graph traversal cache (anchor_type, anchor_id, radius).
CREATE TABLE IF NOT EXISTS public.GraphProjection (
    anchor_type TEXT NOT NULL,
    anchor_id TEXT NOT NULL,
    target_type TEXT,
    target_id TEXT,
    edge_kind TEXT,
    distance BIGINT,
    radius BIGINT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    projection_version TEXT,
    created_at BIGINT
);

CREATE TABLE IF NOT EXISTS public.GraphProjectionState (
    anchor_type TEXT NOT NULL,
    anchor_id TEXT NOT NULL,
    radius BIGINT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    source_signature TEXT,
    projection_version TEXT,
    updated_at BIGINT,
    PRIMARY KEY (anchor_type, anchor_id, radius, project_code)
);

CREATE TABLE IF NOT EXISTS public.GraphEmbedding (
    anchor_type TEXT NOT NULL,
    anchor_id TEXT NOT NULL,
    radius BIGINT NOT NULL,
    model_id TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    source_signature TEXT,
    projection_version TEXT,
    embedding vector(1024),
    updated_at BIGINT,
    PRIMARY KEY (anchor_type, anchor_id, radius, model_id, project_code)
);

CREATE TABLE IF NOT EXISTS public.RewardObservationLog (
    decision_id TEXT NOT NULL,
    observed_at_ms BIGINT,
    window_start_ms BIGINT,
    window_end_ms BIGINT,
    reward_json TEXT,
    throughput_chunks_per_hour DOUBLE PRECISION,
    throughput_files_per_hour DOUBLE PRECISION,
    constraint_violations_json TEXT,
    pressure_summary_json TEXT
);

-- ── Telemetry / lifecycle ────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS public.FileLifecycleEvent (
    file_path TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    stage TEXT NOT NULL,
    status TEXT NOT NULL,
    reason TEXT,
    at_ms BIGINT NOT NULL,
    worker_id BIGINT,
    trace_id TEXT,
    run_id TEXT
);

CREATE TABLE IF NOT EXISTS public.HourlyVectorizationRollup (
    bucket_start_ms BIGINT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    model_id TEXT NOT NULL,
    chunks_embedded BIGINT NOT NULL DEFAULT 0,
    files_vector_ready BIGINT NOT NULL DEFAULT 0,
    batches BIGINT NOT NULL DEFAULT 0,
    fetch_ms_total BIGINT NOT NULL DEFAULT 0,
    embed_ms_total BIGINT NOT NULL DEFAULT 0,
    db_write_ms_total BIGINT NOT NULL DEFAULT 0,
    mark_done_ms_total BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (bucket_start_ms, project_code, model_id)
);

-- ── Indexes (every hot filter leads with project_code) ───────────────

CREATE INDEX IF NOT EXISTS file_project_status_idx
    ON public.File (project_code, status);
CREATE INDEX IF NOT EXISTS file_project_stage_ready_idx
    ON public.File (project_code, file_stage, graph_ready, vector_ready);
-- DEC-AXO-082 follow-up: scheduling needs cheap "next file to claim"
-- + per-priority lookups.
CREATE INDEX IF NOT EXISTS file_status_priority_idx
    ON public.File (status, priority) WHERE status IS NOT NULL;

CREATE INDEX IF NOT EXISTS symbol_project_kind_idx
    ON public.Symbol (project_code, kind);
CREATE INDEX IF NOT EXISTS symbol_project_name_idx
    ON public.Symbol (project_code, name);
-- DEC-AXO-082 follow-up: ANN-pre-filter on entire embedding column.
-- HNSW for the pgvector index lives below alongside ChunkEmbedding.
-- For now Symbol embeddings are rarely queried via ANN — plain B-tree
-- is enough. Add a partial when (embedding IS NOT NULL) to keep the
-- index lean on indexer-only deployments that don't compute symbol
-- embeddings.
CREATE INDEX IF NOT EXISTS symbol_embedding_present_idx
    ON public.Symbol (project_code) WHERE embedding IS NOT NULL;

CREATE INDEX IF NOT EXISTS chunk_project_source_idx
    ON public.Chunk (project_code, source_type, source_id);
CREATE INDEX IF NOT EXISTS chunk_project_file_idx
    ON public.Chunk (project_code, file_path);
-- DEC-AXO-082 follow-up: A3 batched UPSERT looks up chunks by id PK
-- (already covered); but B1 cold-start poll does the heavy join with
-- ChunkEmbedding — add an index to keep the LEFT JOIN cheap.
CREATE INDEX IF NOT EXISTS chunk_content_hash_idx
    ON public.Chunk (content_hash);

CREATE INDEX IF NOT EXISTS chunk_embedding_project_idx
    ON public.ChunkEmbedding (project_code);
-- DEC-AXO-082 follow-up: source_hash drives "is this chunk still
-- up-to-date" lookups — embedded_at_ms helps the optimiser's "stale
-- embedding" queries.
CREATE INDEX IF NOT EXISTS chunk_embedding_source_hash_idx
    ON public.ChunkEmbedding (source_hash);
CREATE INDEX IF NOT EXISTS chunk_embedding_embedded_at_idx
    ON public.ChunkEmbedding (embedded_at_ms);

-- pgvector HNSW (CPT-AXO-041). Single global index covers all
-- projects; project_code filter is applied via WHERE clause and
-- pgvector's iterative scan handles the post-filter efficiently.
CREATE INDEX IF NOT EXISTS chunk_embedding_hnsw_idx
    ON public.ChunkEmbedding USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX IF NOT EXISTS file_vec_queue_project_status_idx
    ON public.FileVectorizationQueue (project_code, status, queued_at);
CREATE INDEX IF NOT EXISTS gp_queue_project_status_idx
    ON public.GraphProjectionQueue (project_code, status, queued_at);

CREATE INDEX IF NOT EXISTS file_lifecycle_project_at_idx
    ON public.FileLifecycleEvent (project_code, at_ms);
-- DEC-AXO-082 follow-up: aggregation queries on stage + status.
CREATE INDEX IF NOT EXISTS file_lifecycle_stage_status_idx
    ON public.FileLifecycleEvent (stage, status);
