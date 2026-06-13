-- Axon canonical schema — IST (Indexed Symbol Tree).
-- REQ-AXO-901860: the IST now lives in its OWN schema `ist` (symmetric to
-- `soll` for intent), NOT in `public`. Table identifiers are preserved
-- verbatim (only the schema changes public→ist) so the code migration is a
-- pure schema-qualification, not a rename.
--
-- Every table carries a `project_code` that is a NOT NULL FOREIGN KEY to
-- ist.Project(code): a row cannot exist without a registered project, so
-- the old silent `UNK` bucket is impossible (fail-loud at enrolment).
-- Pre-launch full-reindex rewrite: NO data migration; the indexer
-- repopulates ist from source.
--
-- Embedding dimension is hard-coded to 1024 (BGE-Large 1024-d, see
-- src/axon-core/src/embedding_contract.rs::DIMENSION). Any model swap
-- must update this file AND the Rust constant in lockstep.
--
-- Idempotent: safe to re-run on every startup.

CREATE SCHEMA IF NOT EXISTS ist;
-- Role-level search_path is set in 00_extensions.sql (before 01). This
-- per-session SET only covers THIS file's own CREATE statements.
SET search_path = ist, "$user", public;

-- ── Project registry ─────────────────────────────────────────────────
-- Canonical per-project root; FK target for every IST table's
-- `project_code`. Enriched vs the old name-only public.Project so the
-- scanner can resolve path→project and telemetry reports per-project
-- roots. Populated by the scanner BEFORE enrolling files.
CREATE TABLE IF NOT EXISTS ist.Project (
    code           TEXT PRIMARY KEY,
    name           TEXT NOT NULL DEFAULT '',
    root_path      TEXT NOT NULL DEFAULT '',
    watch_root     TEXT NOT NULL DEFAULT '',
    status         TEXT NOT NULL DEFAULT 'active',
    enrolled_at_ms BIGINT NOT NULL DEFAULT 0,
    CONSTRAINT project_status_check CHECK (status IN ('active', 'paused', 'retired'))
);

-- ── Runtime build metadata (KV) ──────────────────────────────────────
-- Probed by scripts/start.sh as the schema gate.
CREATE TABLE IF NOT EXISTS ist.RuntimeMetadata (
    key   TEXT PRIMARY KEY,
    value TEXT
);

-- ── Indexed files (durable discovery queue) ──────────────────────────
-- DEC-AXO-901619: scanner writes 'discovered', A3 promotes to 'indexed'.
-- REQ-AXO-901831: status models the FULL lifecycle incl. exclusions
-- ('failed'/'skipped' + skip_reason) so the eligible→enrolled gap is never
-- silent. REQ-AXO-901860: project_code FK (was structurally absent — the
-- root of indexed_files=0 per project).
CREATE TABLE IF NOT EXISTS ist.IndexedFile (
    path            TEXT   PRIMARY KEY,
    project_code    TEXT   NOT NULL REFERENCES ist.Project(code) ON DELETE CASCADE,
    content_hash    TEXT   NOT NULL DEFAULT '',
    last_seen_ms    BIGINT NOT NULL,
    status          TEXT   NOT NULL DEFAULT 'discovered',
    skip_reason     TEXT,
    discovered_ms   BIGINT NOT NULL DEFAULT 0,
    mtime_ms        BIGINT NOT NULL DEFAULT 0,
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    retry_count     INT    NOT NULL DEFAULT 0,
    last_attempt_ms BIGINT,
    -- REQ-AXO-901897 (DBQ slice 1) — claim lease. The DB IS the durable A
    -- work queue: pipeline A claims 'discovered' rows with FOR UPDATE SKIP
    -- LOCKED + a lease (mirrors demand_pull_b for chunks). lease_until_ms is
    -- the epoch-ms after which an in-flight 'parsing' claim is reclaimable
    -- (worker crashed mid-parse). 0 = no active lease.
    lease_until_ms  BIGINT NOT NULL DEFAULT 0,
    CONSTRAINT indexedfile_status_check
        CHECK (status IN (
            'discovered', 'parsing', 'parsed', 'ready',
            'parse_failed', 'skipped', 'deleted', 'indexed'
        ))
);

-- REQ-AXO-901897 (DBQ slice 1) — idempotent ALTERs so an EXISTING live table
-- (30k rows) is migrated forward at the next boot's `apply_canonical_ddl`
-- (scripts/lib/ensure-runtime.sh runs every db/ddl/NN_*.sql with
-- ON_ERROR_STOP=1, so each statement below MUST be safe to re-run).
--
-- 1. lease_until_ms column — ADD COLUMN IF NOT EXISTS is a PG-catalog-only
--    metadata change in PG11+ when a non-volatile DEFAULT is given (no table
--    rewrite, no row-by-row lock on the 30k rows): the default is folded into
--    the catalog and materialised lazily on read. Non-blocking.
ALTER TABLE ist.IndexedFile
    ADD COLUMN IF NOT EXISTS lease_until_ms BIGINT NOT NULL DEFAULT 0;

-- REQ-AXO-901897 hardening — bound the brief ACCESS EXCLUSIVE lock the CONSTRAINT
-- DROP/ADD below takes, so a live boot's apply_canonical_ddl can't head-of-line
-- block behind an open connection holding a lock on ist.indexedfile. Fail fast
-- (3s) rather than stall the boot. Applies to the rest of this psql session.
SET lock_timeout = '3s';

-- 2. Widen the status CHECK to the full A-lifecycle vocabulary. DROP+ADD the
--    NAMED constraint inside a DO block guarded on existence so re-running is
--    a no-op. Adding a CHECK takes a brief ACCESS EXCLUSIVE lock to validate
--    existing rows once; after step 3 below every legacy value already
--    satisfies the new set, so validation passes. (On a busy table this is a
--    short metadata lock, not a rewrite.)
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'indexedfile_status_check'
          AND conrelid = 'ist.indexedfile'::regclass
    ) THEN
        ALTER TABLE ist.IndexedFile DROP CONSTRAINT indexedfile_status_check;
    END IF;
END $$;

-- 3. Migrate legacy status values to the new vocabulary BEFORE re-adding the
--    CHECK (idempotent: the UPDATEs match nothing once already migrated).
--    'indexed'→'ready' would be the eventual target, but 'indexed' is kept
--    transitional in the CHECK set so a half-rolled-out binary that still
--    writes 'indexed' does not violate the constraint. We only normalise the
--    retired 'failed' value (not in the new core set except as legacy).
UPDATE ist.IndexedFile SET status = 'parse_failed' WHERE status = 'failed';

ALTER TABLE ist.IndexedFile
    ADD CONSTRAINT indexedfile_status_check
    CHECK (status IN (
        'discovered', 'parsing', 'parsed', 'ready',
        'parse_failed', 'skipped', 'deleted', 'indexed'
    ));

-- REQ-AXO-901897 — claimable partial index. Replaces the discovered-only
-- index: the A claimer's hot predicate is `status IN ('discovered','parsing')`
-- (a stale-lease 'parsing' row is reclaimable), ordered by discovered_ms.
DROP INDEX IF EXISTS ist.idx_indexedfile_discovered;
CREATE INDEX IF NOT EXISTS idx_indexedfile_claimable
    ON ist.IndexedFile (discovered_ms) INCLUDE (path, content_hash, retry_count, lease_until_ms)
    WHERE status IN ('discovered', 'parsing');
CREATE INDEX IF NOT EXISTS idx_indexedfile_project_status
    ON ist.IndexedFile (project_code, status);

-- DEC-AXO-901620: NOTIFY pipeline A when new files are discovered.
CREATE OR REPLACE FUNCTION ist.fn_notify_file_discovered() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.status = 'discovered' THEN
        PERFORM pg_notify('file_discovered', NEW.path);
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_notify_file_discovered ON ist.IndexedFile;
CREATE TRIGGER trg_notify_file_discovered
    AFTER INSERT OR UPDATE ON ist.IndexedFile
    FOR EACH ROW EXECUTE FUNCTION ist.fn_notify_file_discovered();

-- ── Symbols ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS ist.Symbol (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    kind         TEXT,
    tested       BOOLEAN NOT NULL DEFAULT FALSE,
    is_public    BOOLEAN NOT NULL DEFAULT FALSE,
    is_nif       BOOLEAN NOT NULL DEFAULT FALSE,
    is_unsafe    BOOLEAN NOT NULL DEFAULT FALSE,
    project_code TEXT    NOT NULL REFERENCES ist.Project(code) ON DELETE CASCADE,
    embedding    vector(1024)
);

-- ── Chunks (1 symbol → 1+ chunks) ────────────────────────────────────
-- file_path FK to IndexedFile: a chunk cannot outlive its file.
CREATE TABLE IF NOT EXISTS ist.Chunk (
    id               TEXT PRIMARY KEY,
    source_type      TEXT,
    source_id        TEXT,
    project_code     TEXT NOT NULL REFERENCES ist.Project(code) ON DELETE CASCADE,
    file_path        TEXT REFERENCES ist.IndexedFile(path) ON DELETE CASCADE,
    kind             TEXT,
    content          TEXT,
    content_hash     TEXT,
    start_line       BIGINT,
    end_line         BIGINT,
    chunk_part_index BIGINT,
    chunk_part_count BIGINT,
    chunk_path       TEXT,
    token_count      INTEGER,
    embed_status     TEXT NOT NULL DEFAULT 'pending',
    CONSTRAINT chunk_embed_status_check CHECK (embed_status IN ('pending', 'embedded', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_chunk_pending_embed
    ON ist.Chunk (token_count) WHERE embed_status = 'pending';

-- FTS tsvector. 06_pgmq_tsv_async.sql may DROP the GENERATED expression on
-- the canonical install so a worker populates it out-of-band.
ALTER TABLE ist.Chunk
    ADD COLUMN IF NOT EXISTS content_tsv tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('simple',  coalesce(chunk_path, '')), 'A') ||
        setweight(to_tsvector('simple',  coalesce(kind,       '')), 'A') ||
        setweight(to_tsvector('english', coalesce(content,    '')), 'B') ||
        setweight(to_tsvector('simple',  coalesce(file_path,  '')), 'C')
    ) STORED;

-- ── Chunk embeddings (pgvector 1024-d cosine, HNSW) ──────────────────
-- PK (chunk_id, model_id) so multiple models co-exist during migrations.
-- chunk_id FK so an embedding cannot outlive its chunk.
CREATE TABLE IF NOT EXISTS ist.ChunkEmbedding (
    chunk_id        TEXT NOT NULL REFERENCES ist.Chunk(id) ON DELETE CASCADE,
    model_id        TEXT NOT NULL,
    project_code    TEXT NOT NULL REFERENCES ist.Project(code) ON DELETE CASCADE,
    source_hash     TEXT NOT NULL,
    embedding       vector(1024) NOT NULL,
    embedded_at_ms  BIGINT NOT NULL,
    PRIMARY KEY (chunk_id, model_id)
);

-- ── Structural edges (IST graph) ─────────────────────────────────────
CREATE TABLE IF NOT EXISTS ist.Edge (
    source_id     TEXT NOT NULL,
    target_id     TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    project_code  TEXT NOT NULL REFERENCES ist.Project(code) ON DELETE CASCADE,
    metadata      JSONB,
    created_at_ms BIGINT NOT NULL,
    PRIMARY KEY (source_id, target_id, relation_type, project_code)
);

CREATE TABLE IF NOT EXISTS ist.EmbeddingModel (
    id          TEXT PRIMARY KEY,
    kind        TEXT,
    model_name  TEXT,
    dimension   BIGINT,
    version     TEXT,
    created_at  BIGINT
);

-- ── Graph traversal caches ───────────────────────────────────────────
CREATE TABLE IF NOT EXISTS ist.GraphProjection (
    anchor_type        TEXT NOT NULL,
    anchor_id          TEXT NOT NULL,
    target_type        TEXT,
    target_id          TEXT,
    edge_kind          TEXT,
    distance           BIGINT,
    radius             BIGINT NOT NULL,
    project_code       TEXT   NOT NULL REFERENCES ist.Project(code) ON DELETE CASCADE,
    projection_version TEXT,
    created_at         BIGINT
);

CREATE TABLE IF NOT EXISTS ist.GraphProjectionState (
    anchor_type        TEXT NOT NULL,
    anchor_id          TEXT NOT NULL,
    radius             BIGINT NOT NULL,
    project_code       TEXT   NOT NULL REFERENCES ist.Project(code) ON DELETE CASCADE,
    source_signature   TEXT,
    projection_version TEXT,
    updated_at         BIGINT,
    PRIMARY KEY (anchor_type, anchor_id, radius, project_code)
);

CREATE TABLE IF NOT EXISTS ist.GraphEmbedding (
    anchor_type        TEXT NOT NULL,
    anchor_id          TEXT NOT NULL,
    radius             BIGINT NOT NULL,
    model_id           TEXT NOT NULL,
    project_code       TEXT NOT NULL REFERENCES ist.Project(code) ON DELETE CASCADE,
    source_signature   TEXT,
    projection_version TEXT,
    embedding          vector(1024),
    updated_at         BIGINT,
    PRIMARY KEY (anchor_type, anchor_id, radius, model_id, project_code)
);

-- ── Per-file lifecycle event log (fail-loud ledger) ──────────────────
-- REQ-AXO-901831: every stage transition incl. exclusion (reason) so the
-- eligible→enrolled gap is observable, never silent.
CREATE TABLE IF NOT EXISTS ist.FileLifecycleEvent (
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

-- ── Hourly vectorization throughput rollup ───────────────────────────
CREATE TABLE IF NOT EXISTS ist.HourlyVectorizationRollup (
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

-- ── Indexes ──────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS symbol_project_kind_idx
    ON ist.Symbol (project_code, kind);
CREATE INDEX IF NOT EXISTS symbol_project_name_idx
    ON ist.Symbol (project_code, name);
CREATE INDEX IF NOT EXISTS symbol_embedding_present_idx
    ON ist.Symbol (project_code) WHERE embedding IS NOT NULL;

CREATE INDEX IF NOT EXISTS chunk_project_source_idx
    ON ist.Chunk (project_code, source_type, source_id);
CREATE INDEX IF NOT EXISTS chunk_project_file_idx
    ON ist.Chunk (project_code, file_path);
CREATE INDEX IF NOT EXISTS chunk_content_hash_idx
    ON ist.Chunk (content_hash);
CREATE INDEX IF NOT EXISTS idx_chunk_project_code
    ON ist.Chunk (project_code);
CREATE INDEX IF NOT EXISTS idx_chunk_token_count
    ON ist.Chunk (token_count);
CREATE INDEX IF NOT EXISTS idx_chunk_content_tsv
    ON ist.Chunk USING GIN (content_tsv);

CREATE INDEX IF NOT EXISTS chunk_embedding_project_idx
    ON ist.ChunkEmbedding (project_code);
CREATE INDEX IF NOT EXISTS chunk_embedding_source_hash_idx
    ON ist.ChunkEmbedding (source_hash);
CREATE INDEX IF NOT EXISTS chunk_embedding_embedded_at_idx
    ON ist.ChunkEmbedding (embedded_at_ms);
CREATE INDEX IF NOT EXISTS chunk_embedding_hnsw_idx
    ON ist.ChunkEmbedding USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX IF NOT EXISTS edge_fwd_idx
    ON ist.Edge (source_id, relation_type, target_id);
CREATE INDEX IF NOT EXISTS edge_rev_idx
    ON ist.Edge (target_id, relation_type, source_id);
CREATE INDEX IF NOT EXISTS edge_proj_idx
    ON ist.Edge (project_code, relation_type);
-- No GIN on ist.Edge.metadata: the column is unpopulated and no query filters
-- on it (jsonb_path_ops idx_scan=0) — audited + EXPLAIN-proven (REQ-AXO-901881).

CREATE INDEX IF NOT EXISTS file_lifecycle_project_at_idx
    ON ist.FileLifecycleEvent (project_code, at_ms);
CREATE INDEX IF NOT EXISTS file_lifecycle_stage_status_idx
    ON ist.FileLifecycleEvent (stage, status);

-- ── FK-covering indexes (REQ-AXO-901860) ─────────────────────────────
-- PostgreSQL does NOT auto-index the referencing side of a FOREIGN KEY.
-- Without these, every ON DELETE CASCADE from ist.Project / ist.IndexedFile
-- triggers a sequential scan of the child table, and FK-join lookups are
-- unindexed. project_code FKs on the big tables (Symbol/Chunk/Edge/
-- ChunkEmbedding) are already covered by their project-leading indexes
-- above; these fill the remaining gaps.
CREATE INDEX IF NOT EXISTS idx_chunk_file_path
    ON ist.Chunk (file_path);
CREATE INDEX IF NOT EXISTS idx_graph_projection_project
    ON ist.GraphProjection (project_code);
CREATE INDEX IF NOT EXISTS idx_graph_projection_state_project
    ON ist.GraphProjectionState (project_code);
CREATE INDEX IF NOT EXISTS idx_graph_embedding_project
    ON ist.GraphEmbedding (project_code);

-- ── NOTIFY chunk pending (vectorization signalling) ──────────────────
CREATE OR REPLACE FUNCTION ist.fn_notify_chunk_pending() RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify('chunk_pending_embed', NEW.id);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE TRIGGER trg_chunk_notify_pending
    AFTER INSERT OR UPDATE OF content_hash ON ist.Chunk
    FOR EACH ROW EXECUTE FUNCTION ist.fn_notify_chunk_pending();

-- ── Canonical per-project telemetry view (the ONE source) ────────────
-- The single projection that dashboard + MCP tools read — NOT in-memory
-- counters, NOT scattered ad-hoc rollups, NOT the filesystem walk.
--
-- Coverage is measured by REALITY, not by a status column: REQ-AXO-289
-- retired the discovered/indexing/indexed state machine (the only persisted
-- trace is IndexedFile(path, content_hash, last_seen_ms)), so the old
-- `status='indexed'` filter reported a meaningless near-zero count while
-- the pipeline had actually produced chunks for ~11k files. The honest,
-- monotone funnel is therefore:
--   files_total   = enrolled in IndexedFile
--   files_chunked = enrolled files that produced >=1 chunk (real A-pipeline
--                   coverage ; the remainder = non-code/config files +
--                   files attributed to unresolved projects)
-- files_total >= files_chunked always holds (chunked is a subset).
-- DROP+CREATE (not CREATE OR REPLACE): the column set changed (dropped the
-- retired status-derived columns), which CREATE OR REPLACE VIEW forbids.
-- CASCADE is safe — the dashboard_state functions query this view by name
-- at call time (no hard catalog dependency), so they are not dropped.
DROP VIEW IF EXISTS ist.project_telemetry CASCADE;
CREATE VIEW ist.project_telemetry AS
SELECT
    p.code AS project_code,
    p.name,
    p.root_path,
    COALESCE(f.files_total, 0)      AS files_total,
    COALESCE(f.files_chunked, 0)    AS files_chunked,
    -- REQ-AXO-901890 — files A-processed (parser ran, content_hash set). The
    -- dashboard funnel splits "Indexed = Chunked + No symbols" from
    -- "Remaining = To process - Indexed". files_total counts ALL enrolled
    -- (discovered+parsed); files_indexed is the parsed subset.
    COALESCE(f.files_indexed, 0)    AS files_indexed,
    COALESCE(s.symbols, 0)          AS symbols,
    COALESCE(c.chunks_total, 0)     AS chunks_total,
    COALESCE(c.chunks_embedded, 0)  AS chunks_embedded,
    COALESCE(c.chunks_pending, 0)   AS chunks_pending,
    COALESCE(c.chunks_fts, 0)       AS chunks_fts,
    COALESCE(e.edges, 0)            AS edges
FROM ist.Project p
LEFT JOIN (
    SELECT i.project_code,
           count(*)                                          AS files_total,
           count(*) FILTER (WHERE ch.file_path IS NOT NULL)  AS files_chunked,
           -- REQ-AXO-901890 — "Indexed" = A-processed (parser ran). The marker
           -- is a populated content_hash (A3 sets it on parse), NOT status
           -- (='indexed' is a late embedding-completion flag, lags chunking:
           -- empirically 59 'indexed' vs 10k chunked). content_hash set ⊇
           -- chunked, so Indexed = Chunked + No symbols holds.
           count(*) FILTER (WHERE i.content_hash IS NOT NULL AND i.content_hash <> '') AS files_indexed
    FROM ist.IndexedFile i
    LEFT JOIN (SELECT DISTINCT file_path FROM ist.Chunk) ch ON ch.file_path = i.path
    GROUP BY i.project_code
) f ON f.project_code = p.code
LEFT JOIN (
    SELECT project_code, count(*) AS symbols FROM ist.Symbol GROUP BY project_code
) s ON s.project_code = p.code
LEFT JOIN (
    SELECT project_code,
           count(*)                                          AS chunks_total,
           count(*) FILTER (WHERE embed_status = 'embedded') AS chunks_embedded,
           count(*) FILTER (WHERE embed_status = 'pending')  AS chunks_pending,
           count(*) FILTER (WHERE content_tsv IS NOT NULL)   AS chunks_fts
    FROM ist.Chunk GROUP BY project_code
) c ON c.project_code = p.code
LEFT JOIN (
    SELECT project_code, count(*) AS edges FROM ist.Edge GROUP BY project_code
) e ON e.project_code = p.code;
