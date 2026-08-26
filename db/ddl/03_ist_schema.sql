-- Axon canonical schema — IST (Indexed Symbol Tree).
-- REQ-AXO-901860: the IST now lives in its OWN schema `ist` (symmetric to
-- `soll` for intent), NOT in `public`. Table identifiers are preserved
-- verbatim (only the schema changes public→ist) so the code migration is a
-- pure schema-qualification, not a rename.
--
-- Every table carries a `project_code` that is a NOT NULL FOREIGN KEY to
-- axon.Project(code): a row cannot exist without a registered project, so
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

-- ── Project registry / runtime metadata: MOVED to `axon` ─────────────
-- REQ-AXO-901914: axon.Project, axon.RuntimeMetadata and axon.EmbeddingModel
-- are DURABLE config and live in the `axon` schema (created in 02), so the
-- `ist` schema stays entirely disposable (TRUNCATE/DROP-blind). Every IST
-- table's `project_code` FK below targets axon.Project(code). The migration
-- that moves pre-existing rows is in 02_axon_runtime.sql.

-- ── Indexed files (durable discovery queue) ──────────────────────────
-- DEC-AXO-901619: scanner writes 'discovered', A3 promotes to 'indexed'.
-- REQ-AXO-901831: status models the FULL lifecycle incl. exclusions
-- ('failed'/'skipped' + skip_reason) so the eligible→enrolled gap is never
-- silent. REQ-AXO-901860: project_code FK (was structurally absent — the
-- root of indexed_files=0 per project).
CREATE TABLE IF NOT EXISTS ist.IndexedFile (
    path            TEXT   PRIMARY KEY,
    project_code    TEXT   NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
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

-- REQ-AXO-902522 — detector evidence is not structural code. Keeping typed,
-- redacted findings beside IndexedFile prevents secret regex hits from being
-- represented as Symbol rows (and from corrupting graph/debt analytics).
CREATE TABLE IF NOT EXISTS ist.SecurityFinding (
    project_code     TEXT   NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
    file_path        TEXT   NOT NULL REFERENCES ist.IndexedFile(path) ON DELETE CASCADE,
    rule_id          TEXT   NOT NULL,
    line             BIGINT NOT NULL,
    severity         TEXT   NOT NULL,
    redacted_excerpt TEXT   NOT NULL,
    detected_ms      BIGINT NOT NULL,
    PRIMARY KEY (project_code, file_path, rule_id, line)
);
CREATE INDEX IF NOT EXISTS security_finding_project_idx
    ON ist.SecurityFinding (project_code, severity, file_path, line);

-- REQ-AXO-901897 (DBQ slice 1) — idempotent ALTERs so an EXISTING live table
-- (30k rows) is migrated forward at the next boot's `apply_canonical_ddl`
-- (scripts/lib/ensure-runtime.sh runs every db/ddl/NN_*.sql with
-- ON_ERROR_STOP=1, so each statement below MUST be safe to re-run).
--
-- 1. lease_until_ms column. Le DEFAULT non volatile évite la RÉÉCRITURE de la
--    table (PG11+ : le défaut est replié dans le catalogue et matérialisé à la
--    lecture). Ce que cela n'évite PAS, et que ce fichier a longtemps prétendu
--    « non-blocking » : le VERROU. `ADD COLUMN IF NOT EXISTS` prend ACCESS
--    EXCLUSIVE avant de tester l'existence — donc même quand il n'a rien à
--    faire. REQ-AXO-902339 : passer par la garde catalogue.
SELECT public.add_column_if_absent(
    'ist', 'indexedfile', 'lease_until_ms', 'BIGINT NOT NULL DEFAULT 0');

-- REQ-AXO-901897 hardening — bound the brief ACCESS EXCLUSIVE lock the CONSTRAINT
-- DROP/ADD below takes, so a live boot's apply_canonical_ddl can't head-of-line
-- block behind an open connection holding a lock on ist.indexedfile. Fail fast
-- (3s) rather than stall the boot. Applies to the rest of this psql session.
SET lock_timeout = '3s';

-- 2. Widen the status CHECK to the full A-lifecycle vocabulary.
--    REQ-AXO-902339 — l'ancienne forme gardait le DROP sur l'existence de la
--    contrainte, mais rejouait DROP+ADD à CHAQUE boot dès qu'elle existait :
--    deux ACCESS EXCLUSIVE sur une table écrite en continu, pour un vocabulaire
--    déjà correct. La garde porte désormais sur le CONTENU : on ne touche à la
--    contrainte que si un des 8 statuts canoniques lui manque. Quand c'est le
--    cas, attendre le verrou est légitime — c'est une vraie migration.
DO $status_check$
DECLARE
    v_def   text;
    v_value text;
    v_stale boolean := false;
BEGIN
    SELECT pg_get_constraintdef(oid) INTO v_def
      FROM pg_constraint
     WHERE conname  = 'indexedfile_status_check'
       AND conrelid = 'ist.indexedfile'::regclass;

    IF v_def IS NOT NULL THEN
        FOREACH v_value IN ARRAY ARRAY[
            'discovered', 'parsing', 'parsed', 'ready',
            'parse_failed', 'skipped', 'deleted', 'indexed'
        ] LOOP
            IF position('''' || v_value || '''' IN v_def) = 0 THEN
                v_stale := true;
            END IF;
        END LOOP;

        IF NOT v_stale THEN
            RETURN;  -- vocabulaire déjà complet : aucun verrou pris
        END IF;

        ALTER TABLE ist.IndexedFile DROP CONSTRAINT indexedfile_status_check;
    END IF;

    -- 3. Migrer les valeurs héritées AVANT de (re)poser le CHECK (idempotent :
    --    l'UPDATE ne matche plus rien une fois la migration faite).
    UPDATE ist.IndexedFile SET status = 'parse_failed' WHERE status = 'failed';

    ALTER TABLE ist.IndexedFile
        ADD CONSTRAINT indexedfile_status_check
        CHECK (status IN (
            'discovered', 'parsing', 'parsed', 'ready',
            'parse_failed', 'skipped', 'deleted', 'indexed'
        ));
END
$status_check$;

-- NOTE sur l'étape 3 (migration des valeurs héritées) : elle a été déplacée
-- DANS le bloc ci-dessus. Elle n'a de sens qu'au moment où la contrainte est
-- effectivement reposée ; hors de là, c'est un balayage séquentiel de la table
-- à chaque boot pour zéro ligne. 'indexed'→'ready' serait la cible finale, mais
-- 'indexed' reste transitoire dans le CHECK pour qu'un binaire à moitié déployé
-- qui l'écrit encore ne viole pas la contrainte.

-- REQ-AXO-901897 — claimable partial index. Replaces the discovered-only
-- index: the A claimer's hot predicate is `status IN ('discovered','parsing')`
-- (a stale-lease 'parsing' row is reclaimable), ordered by discovered_ms.
-- `DROP INDEX IF EXISTS` résout le nom AVANT de verrouiller : sur un index
-- absent il ne prend rien, il reste donc tel quel (REQ-AXO-902339).
DROP INDEX IF EXISTS ist.idx_indexedfile_discovered;
SELECT public.create_index_if_absent('ist', 'idx_indexedfile_claimable', $idx$
    CREATE INDEX idx_indexedfile_claimable
        ON ist.IndexedFile (discovered_ms) INCLUDE (path, content_hash, retry_count, lease_until_ms)
        WHERE status IN ('discovered', 'parsing')
$idx$);
SELECT public.create_index_if_absent('ist', 'idx_indexedfile_project_status', $idx$
    CREATE INDEX idx_indexedfile_project_status
        ON ist.IndexedFile (project_code, status)
$idx$);

-- DEC-AXO-901620: NOTIFY pipeline A when new files are discovered.
CREATE OR REPLACE FUNCTION ist.fn_notify_file_discovered() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.status = 'discovered' THEN
        PERFORM pg_notify('file_discovered', NEW.path);
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- REQ-AXO-902339 — l'ancien couple DROP+CREATE prenait DEUX fois ACCESS
-- EXCLUSIVE à chaque boot : sur une base déjà bootstrappée le trigger existe,
-- donc le DROP verrouille pour de bon, et le CREATE reverrouille derrière.
-- (Mesuré : `DROP TRIGGER IF EXISTS` sur un trigger ABSENT, lui, ne verrouille
-- rien — il résout le trigger avant la table. C'est l'existence qui coûte.)
-- Le corps du trigger reste chaud-modifiable via le CREATE OR REPLACE FUNCTION
-- ci-dessus, qui ne verrouille aucune table.
SELECT public.create_trigger_if_absent(
    'ist', 'indexedfile', 'trg_notify_file_discovered', $trg$
    CREATE TRIGGER trg_notify_file_discovered
        AFTER INSERT OR UPDATE ON ist.IndexedFile
        FOR EACH ROW EXECUTE FUNCTION ist.fn_notify_file_discovered()
$trg$);

-- ── Symbols ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS ist.Symbol (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    kind         TEXT,
    tested       BOOLEAN NOT NULL DEFAULT FALSE,
    is_public    BOOLEAN NOT NULL DEFAULT FALSE,
    is_nif       BOOLEAN NOT NULL DEFAULT FALSE,
    is_unsafe    BOOLEAN NOT NULL DEFAULT FALSE,
    is_entry_point BOOLEAN NOT NULL DEFAULT FALSE,
    project_code TEXT    NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
    embedding    vector(1024),
    cyclomatic_complexity INTEGER
);
-- REQ-AXO-902185 (god-objects) — additive for live DBs whose ist.Symbol
-- predates the column (CREATE TABLE IF NOT EXISTS above is a no-op when the
-- table already exists). NULL = not yet computed by this parser/language
-- (pre-migration rows, or a language whose counting slice hasn't landed
-- yet) — treated as "not god-object" by the SHI classifier, never as 0.
SELECT public.add_column_if_absent(
    'ist', 'symbol', 'cyclomatic_complexity', 'INTEGER');
-- REQ-AXO-902227 (@impl entry-points) — additive for live DBs whose ist.Symbol
-- predates the column. Structural entry-point flag set by the parser (@impl
-- annotation / framework callback / NIF), consumed by orphan_clusters/wiring
-- reachability seeding so runtime-invoked callbacks aren't false orphans.
-- Default FALSE; repopulated on reindex.
SELECT public.add_column_if_absent(
    'ist', 'symbol', 'is_entry_point', 'BOOLEAN NOT NULL DEFAULT FALSE');

-- ── Chunks (1 symbol → 1+ chunks) ────────────────────────────────────
-- file_path FK to IndexedFile: a chunk cannot outlive its file.
CREATE TABLE IF NOT EXISTS ist.Chunk (
    id               TEXT PRIMARY KEY,
    source_type      TEXT,
    source_id        TEXT,
    project_code     TEXT NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
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
    -- REQ-AXO-902012 — bounded embed retry. Incremented on each B2/B3 failure;
    -- at the cap the chunk flips embed_status='failed' so the sorted drain
    -- (WHERE embed_status='pending') stops re-feeding it (anti poison-pill).
    embed_attempts   INTEGER NOT NULL DEFAULT 0,
    -- REQ-AXO-902260 (Q2) — when this chunk row was FIRST written. The table
    -- carried no temporal column at all, which is why Q2 is undecidable: when
    -- LLL stalled at 25 of 434 files, nothing could tell whether those 25 formed
    -- one contiguous window (an interrupted batch) or a scatter (a poison item).
    -- NULLABLE on purpose: rows written before this column exist have no honest
    -- value, and a backfill would invent one. NULL reads as "unknown", which is
    -- the truth. ON CONFLICT DO UPDATE never touches it, so a re-chunk preserves
    -- first-seen time.
    created_at_ms    BIGINT DEFAULT (EXTRACT(EPOCH FROM now()) * 1000)::BIGINT,
    CONSTRAINT chunk_embed_status_check CHECK (embed_status IN ('pending', 'embedded', 'failed'))
);
-- REQ-AXO-902012 — additive for live DBs whose ist.Chunk predates the column
-- (CREATE TABLE IF NOT EXISTS above is a no-op when the table already exists).
SELECT public.add_column_if_absent(
    'ist', 'chunk', 'embed_attempts', 'INTEGER NOT NULL DEFAULT 0');

-- REQ-AXO-902260 — additive in TWO statements, deliberately, and the order is
-- the whole point. `ADD COLUMN ... DEFAULT <volatile expr>` makes PostgreSQL
-- rewrite the table and evaluate now() PER EXISTING ROW: on a live IST that is
-- both a full-table lock and a fabricated timestamp on every historical chunk.
-- Adding the column bare is instant and leaves them NULL; SET DEFAULT then
-- applies to FUTURE inserts only. Fresh installs get the same nullable column
-- with the same default from the CREATE above — one schema, not two.
SELECT public.add_column_if_absent(
    'ist', 'chunk', 'created_at_ms', 'BIGINT');
SELECT public.set_column_default_if_absent(
    'ist', 'chunk', 'created_at_ms', '(EXTRACT(EPOCH FROM now()) * 1000)::BIGINT');

SELECT public.create_index_if_absent('ist', 'idx_chunk_pending_embed', $idx$
    CREATE INDEX idx_chunk_pending_embed
        ON ist.Chunk (token_count) WHERE embed_status = 'pending'
$idx$);

-- REQ-AXO-902260 — the chunk time window is a DIAGNOSTIC read (diagnose_indexing)
-- over a multi-million-row table; without this, every MIN/MAX is a seq scan and
-- the tool that exists to explain a stall becomes a reason not to run it.
SELECT public.create_index_if_absent('ist', 'idx_chunk_created_at', $idx$
    CREATE INDEX idx_chunk_created_at
        ON ist.Chunk (project_code, created_at_ms)
$idx$);

-- FTS tsvector. 06_pgmq_tsv_async.sql may DROP the GENERATED expression on
-- the canonical install so a worker populates it out-of-band.
SELECT public.add_column_if_absent('ist', 'chunk', 'content_tsv', $col$
    tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('simple',  coalesce(chunk_path, '')), 'A') ||
        setweight(to_tsvector('simple',  coalesce(kind,       '')), 'A') ||
        setweight(to_tsvector('english', coalesce(content,    '')), 'B') ||
        setweight(to_tsvector('simple',  coalesce(file_path,  '')), 'C')
    ) STORED
$col$);

-- ── Chunk embeddings (pgvector 1024-d cosine, HNSW) ──────────────────
-- PK (chunk_id, model_id) so multiple models co-exist during migrations.
-- chunk_id FK so an embedding cannot outlive its chunk.
CREATE TABLE IF NOT EXISTS ist.ChunkEmbedding (
    chunk_id        TEXT NOT NULL REFERENCES ist.Chunk(id) ON DELETE CASCADE,
    model_id        TEXT NOT NULL,
    project_code    TEXT NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
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
    project_code  TEXT NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
    metadata      JSONB,
    created_at_ms BIGINT NOT NULL,
    PRIMARY KEY (source_id, target_id, relation_type, project_code)
);

-- ist.EmbeddingModel MOVED to axon.EmbeddingModel (REQ-AXO-901914, see 02).

-- ── Graph traversal caches ───────────────────────────────────────────
CREATE TABLE IF NOT EXISTS ist.GraphProjection (
    anchor_type        TEXT NOT NULL,
    anchor_id          TEXT NOT NULL,
    target_type        TEXT,
    target_id          TEXT,
    edge_kind          TEXT,
    distance           BIGINT,
    radius             BIGINT NOT NULL,
    project_code       TEXT   NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
    projection_version TEXT,
    created_at         BIGINT
);

CREATE TABLE IF NOT EXISTS ist.GraphProjectionState (
    anchor_type        TEXT NOT NULL,
    anchor_id          TEXT NOT NULL,
    radius             BIGINT NOT NULL,
    project_code       TEXT   NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
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
    project_code       TEXT NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
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
-- REQ-AXO-902339 — `CREATE INDEX IF NOT EXISTS` prend un SHARE sur la TABLE
-- avant de constater que l'index est déjà là. SHARE entre en conflit avec le
-- ROW EXCLUSIVE de l'indexeur : rejoué sur une base à jour, chacun de ces
-- énoncés attend un écrivain continu pour ne rien faire. `create_index_if_absent`
-- lit d'abord `pg_class`.
--
-- Sémantiquement identique à l'ancienne forme : PostgreSQL ne compare que le
-- NOM de l'index, jamais sa définition — changer la définition en gardant le nom
-- était déjà sans effet. Le nom passé en 2e argument DOIT donc rester en phase
-- avec celui du CREATE ; c'est la seule contrainte de cohérence de cette forme,
-- et `ddl_lock_tests::every_declared_index_exists_after_bootstrap` la vérifie.
SELECT public.create_index_if_absent('ist', 'symbol_project_kind_idx', $idx$
    CREATE INDEX symbol_project_kind_idx ON ist.Symbol (project_code, kind)
$idx$);
SELECT public.create_index_if_absent('ist', 'symbol_project_name_idx', $idx$
    CREATE INDEX symbol_project_name_idx ON ist.Symbol (project_code, name)
$idx$);
SELECT public.create_index_if_absent('ist', 'symbol_embedding_present_idx', $idx$
    CREATE INDEX symbol_embedding_present_idx
        ON ist.Symbol (project_code) WHERE embedding IS NOT NULL
$idx$);

SELECT public.create_index_if_absent('ist', 'chunk_project_source_idx', $idx$
    CREATE INDEX chunk_project_source_idx
        ON ist.Chunk (project_code, source_type, source_id)
$idx$);
SELECT public.create_index_if_absent('ist', 'chunk_project_file_idx', $idx$
    CREATE INDEX chunk_project_file_idx ON ist.Chunk (project_code, file_path)
$idx$);
SELECT public.create_index_if_absent('ist', 'chunk_content_hash_idx', $idx$
    CREATE INDEX chunk_content_hash_idx ON ist.Chunk (content_hash)
$idx$);
SELECT public.create_index_if_absent('ist', 'idx_chunk_project_code', $idx$
    CREATE INDEX idx_chunk_project_code ON ist.Chunk (project_code)
$idx$);
SELECT public.create_index_if_absent('ist', 'idx_chunk_token_count', $idx$
    CREATE INDEX idx_chunk_token_count ON ist.Chunk (token_count)
$idx$);
SELECT public.create_index_if_absent('ist', 'idx_chunk_content_tsv', $idx$
    CREATE INDEX idx_chunk_content_tsv ON ist.Chunk USING GIN (content_tsv)
$idx$);

SELECT public.create_index_if_absent('ist', 'chunk_embedding_project_idx', $idx$
    CREATE INDEX chunk_embedding_project_idx ON ist.ChunkEmbedding (project_code)
$idx$);
SELECT public.create_index_if_absent('ist', 'chunk_embedding_source_hash_idx', $idx$
    CREATE INDEX chunk_embedding_source_hash_idx ON ist.ChunkEmbedding (source_hash)
$idx$);
SELECT public.create_index_if_absent('ist', 'chunk_embedding_embedded_at_idx', $idx$
    CREATE INDEX chunk_embedding_embedded_at_idx ON ist.ChunkEmbedding (embedded_at_ms)
$idx$);
SELECT public.create_index_if_absent('ist', 'chunk_embedding_hnsw_idx', $idx$
    CREATE INDEX chunk_embedding_hnsw_idx
        ON ist.ChunkEmbedding USING hnsw (embedding vector_cosine_ops)
        WITH (m = 16, ef_construction = 64)
$idx$);

SELECT public.create_index_if_absent('ist', 'edge_fwd_idx', $idx$
    CREATE INDEX edge_fwd_idx ON ist.Edge (source_id, relation_type, target_id)
$idx$);
SELECT public.create_index_if_absent('ist', 'edge_rev_idx', $idx$
    CREATE INDEX edge_rev_idx ON ist.Edge (target_id, relation_type, source_id)
$idx$);
SELECT public.create_index_if_absent('ist', 'edge_proj_idx', $idx$
    CREATE INDEX edge_proj_idx ON ist.Edge (project_code, relation_type)
$idx$);
-- No GIN on ist.Edge.metadata: the column is unpopulated and no query filters
-- on it (jsonb_path_ops idx_scan=0) — audited + EXPLAIN-proven (REQ-AXO-901881).

SELECT public.create_index_if_absent('ist', 'file_lifecycle_project_at_idx', $idx$
    CREATE INDEX file_lifecycle_project_at_idx
        ON ist.FileLifecycleEvent (project_code, at_ms)
$idx$);
SELECT public.create_index_if_absent('ist', 'file_lifecycle_stage_status_idx', $idx$
    CREATE INDEX file_lifecycle_stage_status_idx
        ON ist.FileLifecycleEvent (stage, status)
$idx$);

-- ── FK-covering indexes (REQ-AXO-901860) ─────────────────────────────
-- PostgreSQL does NOT auto-index the referencing side of a FOREIGN KEY.
-- Without these, every ON DELETE CASCADE from axon.Project / ist.IndexedFile
-- triggers a sequential scan of the child table, and FK-join lookups are
-- unindexed. project_code FKs on the big tables (Symbol/Chunk/Edge/
-- ChunkEmbedding) are already covered by their project-leading indexes
-- above; these fill the remaining gaps.
SELECT public.create_index_if_absent('ist', 'idx_chunk_file_path', $idx$
    CREATE INDEX idx_chunk_file_path ON ist.Chunk (file_path)
$idx$);
SELECT public.create_index_if_absent('ist', 'idx_graph_projection_project', $idx$
    CREATE INDEX idx_graph_projection_project ON ist.GraphProjection (project_code)
$idx$);
SELECT public.create_index_if_absent('ist', 'idx_graph_projection_state_project', $idx$
    CREATE INDEX idx_graph_projection_state_project
        ON ist.GraphProjectionState (project_code)
$idx$);
SELECT public.create_index_if_absent('ist', 'idx_graph_embedding_project', $idx$
    CREATE INDEX idx_graph_embedding_project ON ist.GraphEmbedding (project_code)
$idx$);

-- ── NOTIFY chunk pending (vectorization signalling) ──────────────────
CREATE OR REPLACE FUNCTION ist.fn_notify_chunk_pending() RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify('chunk_pending_embed', NEW.id);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- REQ-AXO-902339 — `CREATE OR REPLACE TRIGGER` est atomique, mais il prend
-- ACCESS EXCLUSIVE sur ist.Chunk à chaque boot, y compris quand le trigger est
-- déjà identique. Le corps reste chaud-modifiable via la fonction ci-dessus.
SELECT public.create_trigger_if_absent(
    'ist', 'chunk', 'trg_chunk_notify_pending', $trg$
    CREATE TRIGGER trg_chunk_notify_pending
        AFTER INSERT OR UPDATE OF content_hash ON ist.Chunk
        FOR EACH ROW EXECUTE FUNCTION ist.fn_notify_chunk_pending()
$trg$);

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
DROP VIEW IF EXISTS axon.project_telemetry CASCADE;
CREATE VIEW axon.project_telemetry AS
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
    -- REQ-AXO-902382: 'failed' is TERMINAL and nothing in the runtime retries it —
    -- the sorted-drain reservoir only ever SELECTs embed_status='pending'. Consumers
    -- were deriving "pending" as total-embedded, which silently merged a dead
    -- population into an active queue: on 2026-08-21 that read 228 968 "pending"
    -- where the truth was 228 942 dead + 140 waiting, and sent an operator hunting
    -- for a service mechanism that does not exist. Exposed so the two can never be
    -- summed by accident again.
    COALESCE(c.chunks_failed, 0)    AS chunks_failed,
    COALESCE(c.chunks_fts, 0)       AS chunks_fts,
    COALESCE(e.edges, 0)            AS edges
FROM axon.Project p
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
           count(*) FILTER (WHERE embed_status = 'failed')    AS chunks_failed,
           count(*) FILTER (WHERE content_tsv IS NOT NULL)   AS chunks_fts
    FROM ist.Chunk GROUP BY project_code
) c ON c.project_code = p.code
LEFT JOIN (
    SELECT project_code, count(*) AS edges FROM ist.Edge GROUP BY project_code
) e ON e.project_code = p.code;

-- REQ-AXO-158 (DEC-AXO-901650) — architectural drift continuous monitoring.
-- One row per (project, layer_pair) per recorded wave. `score` = violation
-- count for that layer-pair at wave time; `ewma` = exponentially-weighted
-- moving average of the score (smooths inter-wave noise without needing a
-- stable variance estimate, unlike a Z-score on a young/volatile corpus);
-- `alert` = score exceeded ewma * k. Append-only history → heatmap + trend.
CREATE TABLE IF NOT EXISTS ist.drift_history (
    id           BIGSERIAL PRIMARY KEY,
    project_code TEXT        NOT NULL,
    layer_pair   TEXT        NOT NULL,
    wave_ts      TIMESTAMPTZ NOT NULL DEFAULT now(),
    score        INTEGER     NOT NULL,
    ewma         DOUBLE PRECISION NOT NULL,
    alert        BOOLEAN     NOT NULL DEFAULT false
);
SELECT public.create_index_if_absent('ist', 'idx_drift_history_lookup', $idx$
    CREATE INDEX idx_drift_history_lookup
        ON ist.drift_history (project_code, layer_pair, wave_ts DESC)
$idx$);
