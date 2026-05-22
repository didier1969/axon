-- Axon canonical schema — IST (Indexed Symbol Tree).
-- Multi-project: every table lives in `public` with a `project_code`
-- column to scope rows.
-- Idempotent: safe to re-run on every startup.
--
-- Embedding dimension is hard-coded to 1024 (BGE-Large 1024-d, see
-- src/axon-core/src/embedding_contract.rs::DIMENSION). Any model swap
-- must update this file AND the Rust constant in lockstep.

-- ── Tables ───────────────────────────────────────────────────────────

-- KV store for runtime build metadata (active install generation, last
-- promote timestamp, …). Probed by scripts/start.sh as the schema gate.
CREATE TABLE IF NOT EXISTS public.RuntimeMetadata (
    key   TEXT PRIMARY KEY,
    value TEXT
);

-- REQ-AXO-901653 slice-5c — public.File state-machine retired.
-- Pipeline-v2 (REQ-AXO-289 / CPT-AXO-054) writes IndexedFile directly ;
-- the per-file status/stage/ready flags + worker claim metadata are no
-- longer needed (Chunk + ChunkEmbedding presence is the canonical
-- truth). DROP IF EXISTS makes the bootstrap idempotent on legacy DBs.
DROP TABLE IF EXISTS public.File CASCADE;

-- Streaming pipeline v2 watcher filter (REQ-AXO-289).
-- 3 columns only: path PK, content_hash for change detection,
-- last_seen_ms for hygiene. No status machine.
CREATE TABLE IF NOT EXISTS public.IndexedFile (
    path         TEXT PRIMARY KEY,
    content_hash TEXT   NOT NULL,
    last_seen_ms BIGINT NOT NULL
);

-- Code symbols (functions, types, modules, …).
CREATE TABLE IF NOT EXISTS public.Symbol (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    kind         TEXT,
    tested       BOOLEAN NOT NULL DEFAULT FALSE,
    is_public    BOOLEAN NOT NULL DEFAULT FALSE,
    is_nif       BOOLEAN NOT NULL DEFAULT FALSE,
    is_unsafe    BOOLEAN NOT NULL DEFAULT FALSE,
    project_code TEXT    NOT NULL DEFAULT '',
    embedding    vector(1024)
);

-- Sliced code blocks (1 symbol → 1+ chunks). `token_count` stores the
-- BGE-Large estimated token count for length-homogeneous batching.
CREATE TABLE IF NOT EXISTS public.Chunk (
    id               TEXT PRIMARY KEY,
    source_type      TEXT,
    source_id        TEXT,
    project_code     TEXT NOT NULL DEFAULT '',
    file_path        TEXT,
    kind             TEXT,
    content          TEXT,
    content_hash     TEXT,
    start_line       BIGINT,
    end_line         BIGINT,
    chunk_part_index BIGINT,
    chunk_part_count BIGINT,
    chunk_path       TEXT,
    token_count      INTEGER
);

-- FTS tsvector column. Initially declared GENERATED ALWAYS STORED with
-- the 4-setweight expression below (chunk_path A / kind A / content B /
-- file_path C); the pgmq lazy-build path (06_pgmq_tsv_async.sql) DROPs
-- the expression on the canonical install so a worker can populate the
-- column out-of-band. Fresh installs without pgmq keep the GENERATED
-- semantics.
ALTER TABLE public.Chunk
    ADD COLUMN IF NOT EXISTS content_tsv tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('simple',  coalesce(chunk_path, '')), 'A') ||
        setweight(to_tsvector('simple',  coalesce(kind,       '')), 'A') ||
        setweight(to_tsvector('english', coalesce(content,    '')), 'B') ||
        setweight(to_tsvector('simple',  coalesce(file_path,  '')), 'C')
    ) STORED;

-- pgvector storage (1024-d cosine, HNSW). PK is (chunk_id, model_id) so
-- multiple models can co-exist during embedding migrations.
CREATE TABLE IF NOT EXISTS public.ChunkEmbedding (
    chunk_id        TEXT NOT NULL,
    model_id        TEXT NOT NULL,
    project_code    TEXT NOT NULL DEFAULT '',
    source_hash     TEXT NOT NULL,
    embedding       vector(1024) NOT NULL,
    embedded_at_ms  BIGINT NOT NULL,
    PRIMARY KEY (chunk_id, model_id)
);

-- Unified structural edge table backing the IST graph. Composite PK so
-- the same source/target pair may carry multiple typed relations.
-- Graph traversal exposed via the SQL function library (04_graph_functions.sql).
CREATE TABLE IF NOT EXISTS public.Edge (
    source_id     TEXT NOT NULL,
    target_id     TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    project_code  TEXT NOT NULL DEFAULT '',
    metadata      JSONB,
    created_at_ms BIGINT NOT NULL,
    PRIMARY KEY (source_id, target_id, relation_type, project_code)
);

-- Vectorization signalling: pg_notify on Chunk INSERT or content_hash
-- change. The B1 listener maintains an in-memory pending set; a 5s
-- reconciliation tick catches missed NOTIFYs via LEFT JOIN anti-join on
-- ChunkEmbedding. No disk queue.
DROP TABLE IF EXISTS public.FileVectorizationQueue;

-- REQ-AXO-901653 slice-5c — public.GraphProjectionQueue retired.
-- Graph-projection refreshes are now driven inline by pipeline-v2 (no
-- on-disk queue ; the worker pool consumes Chunk/Edge directly).
DROP TABLE IF EXISTS public.GraphProjectionQueue CASCADE;

-- Project registry consumed by the indexer hot path.
CREATE TABLE IF NOT EXISTS public.Project (
    name TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS public.EmbeddingModel (
    id          TEXT PRIMARY KEY,
    kind        TEXT,
    model_name  TEXT,
    dimension   BIGINT,
    version     TEXT,
    created_at  BIGINT
);

-- Graph traversal cache (anchor_type, anchor_id, radius).
CREATE TABLE IF NOT EXISTS public.GraphProjection (
    anchor_type        TEXT NOT NULL,
    anchor_id          TEXT NOT NULL,
    target_type        TEXT,
    target_id          TEXT,
    edge_kind          TEXT,
    distance           BIGINT,
    radius             BIGINT NOT NULL,
    project_code       TEXT   NOT NULL DEFAULT '',
    projection_version TEXT,
    created_at         BIGINT
);

CREATE TABLE IF NOT EXISTS public.GraphProjectionState (
    anchor_type        TEXT NOT NULL,
    anchor_id          TEXT NOT NULL,
    radius             BIGINT NOT NULL,
    project_code       TEXT   NOT NULL DEFAULT '',
    source_signature   TEXT,
    projection_version TEXT,
    updated_at         BIGINT,
    PRIMARY KEY (anchor_type, anchor_id, radius, project_code)
);

CREATE TABLE IF NOT EXISTS public.GraphEmbedding (
    anchor_type        TEXT NOT NULL,
    anchor_id          TEXT NOT NULL,
    radius             BIGINT NOT NULL,
    model_id           TEXT NOT NULL,
    project_code       TEXT NOT NULL DEFAULT '',
    source_signature   TEXT,
    projection_version TEXT,
    embedding          vector(1024),
    updated_at         BIGINT,
    PRIMARY KEY (anchor_type, anchor_id, radius, model_id, project_code)
);

-- Post-decision reward observation log (optimiser feedback loop).
CREATE TABLE IF NOT EXISTS public.RewardObservationLog (
    decision_id                TEXT NOT NULL,
    observed_at_ms             BIGINT,
    window_start_ms            BIGINT,
    window_end_ms              BIGINT,
    reward_json                TEXT,
    throughput_chunks_per_hour DOUBLE PRECISION,
    throughput_files_per_hour  DOUBLE PRECISION,
    constraint_violations_json TEXT,
    pressure_summary_json      TEXT
);

-- Per-file lifecycle event log (one row per stage transition).
CREATE TABLE IF NOT EXISTS public.FileLifecycleEvent (
    file_path    TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    stage        TEXT NOT NULL,
    status       TEXT NOT NULL,
    reason       TEXT,
    at_ms        BIGINT NOT NULL,
    worker_id    BIGINT,
    trace_id     TEXT,
    run_id       TEXT
);

-- Hourly aggregates for vectorization throughput dashboards.
CREATE TABLE IF NOT EXISTS public.HourlyVectorizationRollup (
    bucket_start_ms    BIGINT NOT NULL,
    project_code       TEXT   NOT NULL DEFAULT '',
    model_id           TEXT   NOT NULL,
    chunks_embedded    BIGINT NOT NULL DEFAULT 0,
    files_vector_ready BIGINT NOT NULL DEFAULT 0,
    batches            BIGINT NOT NULL DEFAULT 0,
    fetch_ms_total     BIGINT NOT NULL DEFAULT 0,
    embed_ms_total     BIGINT NOT NULL DEFAULT 0,
    db_write_ms_total  BIGINT NOT NULL DEFAULT 0,
    mark_done_ms_total BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (bucket_start_ms, project_code, model_id)
);

-- ── Additive column migrations (legacy DBs predating columns) ────────

-- REQ-AXO-901653 slice-5c — public.File ALTER block removed alongside
-- the table itself.

ALTER TABLE public.Chunk ADD COLUMN IF NOT EXISTS token_count INTEGER;

-- ── Indexes ──────────────────────────────────────────────────────────

-- REQ-AXO-901653 slice-5c — public.File indexes removed alongside the table.

CREATE INDEX IF NOT EXISTS symbol_project_kind_idx
    ON public.Symbol (project_code, kind);
CREATE INDEX IF NOT EXISTS symbol_project_name_idx
    ON public.Symbol (project_code, name);
-- Partial index keeps the lookup lean on indexer-only deployments that
-- don't compute symbol embeddings.
CREATE INDEX IF NOT EXISTS symbol_embedding_present_idx
    ON public.Symbol (project_code) WHERE embedding IS NOT NULL;

CREATE INDEX IF NOT EXISTS chunk_project_source_idx
    ON public.Chunk (project_code, source_type, source_id);
CREATE INDEX IF NOT EXISTS chunk_project_file_idx
    ON public.Chunk (project_code, file_path);
-- B1 cold-start poll joins Chunk with ChunkEmbedding; keep the join cheap.
CREATE INDEX IF NOT EXISTS chunk_content_hash_idx
    ON public.Chunk (content_hash);
CREATE INDEX IF NOT EXISTS idx_chunk_project_code
    ON public.Chunk (project_code);
CREATE INDEX IF NOT EXISTS idx_chunk_token_count
    ON public.Chunk (token_count);
CREATE INDEX IF NOT EXISTS idx_chunk_content_tsv
    ON public.Chunk USING GIN (content_tsv);

CREATE INDEX IF NOT EXISTS chunk_embedding_project_idx
    ON public.ChunkEmbedding (project_code);
CREATE INDEX IF NOT EXISTS chunk_embedding_source_hash_idx
    ON public.ChunkEmbedding (source_hash);
CREATE INDEX IF NOT EXISTS chunk_embedding_embedded_at_idx
    ON public.ChunkEmbedding (embedded_at_ms);

-- Single global HNSW index (CPT-AXO-041). project_code is post-filter
-- via WHERE; pgvector iterative scan handles it efficiently.
CREATE INDEX IF NOT EXISTS chunk_embedding_hnsw_idx
    ON public.ChunkEmbedding USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Edge: forward (impact / blast_radius), reverse (callers_of / why_chain),
-- project-scoped scans, metadata filtering.
CREATE INDEX IF NOT EXISTS edge_fwd_idx
    ON public.Edge (source_id, relation_type, target_id);
CREATE INDEX IF NOT EXISTS edge_rev_idx
    ON public.Edge (target_id, relation_type, source_id);
CREATE INDEX IF NOT EXISTS edge_proj_idx
    ON public.Edge (project_code, relation_type);
CREATE INDEX IF NOT EXISTS edge_metadata_idx
    ON public.Edge USING GIN (metadata jsonb_path_ops);

-- REQ-AXO-901653 slice-5c — public.GraphProjectionQueue index removed alongside the table.

CREATE INDEX IF NOT EXISTS file_lifecycle_project_at_idx
    ON public.FileLifecycleEvent (project_code, at_ms);
CREATE INDEX IF NOT EXISTS file_lifecycle_stage_status_idx
    ON public.FileLifecycleEvent (stage, status);

-- ── Functions & triggers ─────────────────────────────────────────────

-- NOTIFY on Chunk INSERT or content_hash change. The B1 listener
-- maintains an in-memory pending set; reconciliation tick catches
-- missed NOTIFYs via LEFT JOIN anti-join on ChunkEmbedding.
CREATE OR REPLACE FUNCTION public.fn_notify_chunk_pending() RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify('chunk_pending_embed', NEW.id);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE TRIGGER trg_chunk_notify_pending
    AFTER INSERT OR UPDATE OF content_hash ON public.Chunk
    FOR EACH ROW EXECUTE FUNCTION public.fn_notify_chunk_pending();
