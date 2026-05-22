-- Axon canonical schema — SOLL intent layer.
-- Cross-project graph: Node + Edge + Revision + Evidence + audit jobs.
-- Multi-tenant via `project_code` columns ('^[A-Z][A-Z0-9]{2}$').
-- Canonical ID format `TYPE-PROJ-N` enforced by trigger + CHECK (DEC-AXO-085).
-- Idempotent: safe to re-run on every startup.
--
-- Ordering invariant: CREATE TABLE → ALTER (additive columns +
-- constraints) → CREATE INDEX → CREATE FUNCTION → CREATE TRIGGER. The
-- post-CREATE ALTER block is what lets fresh `psql -v ON_ERROR_STOP=1`
-- bootstrap and live in-place upgrades both succeed.

CREATE SCHEMA IF NOT EXISTS soll;

-- ── Tables ───────────────────────────────────────────────────────────

-- Project registry. Lives in `soll` (not `public`) to colocate with
-- consumer code paths (axon_init_project / soll_validate / axon_commit_work).
CREATE TABLE IF NOT EXISTS soll.ProjectCodeRegistry (
    project_code         TEXT PRIMARY KEY,
    project_name         TEXT,
    project_path         TEXT,
    session_pointer_json TEXT,
    registered_at_ms     BIGINT NOT NULL DEFAULT (extract(epoch from now()) * 1000)::BIGINT
);

-- Per-project canonical-ID counter. One row per project_code; counters
-- bumped atomically by `soll.allocate_node_id`.
CREATE TABLE IF NOT EXISTS soll.Registry (
    project_code TEXT PRIMARY KEY DEFAULT 'AXON_GLOBAL',
    id           TEXT   NOT NULL DEFAULT 'AXON_GLOBAL',
    last_vis     BIGINT NOT NULL DEFAULT 0,
    last_pil     BIGINT NOT NULL DEFAULT 0,
    last_req     BIGINT NOT NULL DEFAULT 0,
    last_cpt     BIGINT NOT NULL DEFAULT 0,
    last_dec     BIGINT NOT NULL DEFAULT 0,
    last_mil     BIGINT NOT NULL DEFAULT 0,
    last_val     BIGINT NOT NULL DEFAULT 0,
    last_stk     BIGINT NOT NULL DEFAULT 0,
    last_gui     BIGINT NOT NULL DEFAULT 0,
    last_ski     BIGINT NOT NULL DEFAULT 0,
    last_prt     BIGINT NOT NULL DEFAULT 0,
    last_prv     BIGINT NOT NULL DEFAULT 0,
    last_rev     BIGINT NOT NULL DEFAULT 0
);

-- Intent graph: nodes.
CREATE TABLE IF NOT EXISTS soll.Node (
    id           TEXT PRIMARY KEY,
    type         TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    title        TEXT,
    description  TEXT,
    status       TEXT,
    metadata     JSONB
);

-- Intent graph: edges. Composite PK so the same source/target pair may
-- carry multiple typed relations (REFINES, SUPERSEDES, …).
CREATE TABLE IF NOT EXISTS soll.Edge (
    source_id     TEXT NOT NULL,
    target_id     TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    project_code  TEXT NOT NULL DEFAULT '',
    metadata      JSONB,
    PRIMARY KEY (source_id, target_id, relation_type)
);

-- Revision audit trail. Each `Revision` groups N `RevisionChange` rows
-- (one per touched SOLL entity) so soll_rollback_revision is atomic.
CREATE TABLE IF NOT EXISTS soll.Revision (
    revision_id  TEXT PRIMARY KEY,
    project_code TEXT NOT NULL DEFAULT '',
    author       TEXT,
    source       TEXT,
    summary      TEXT,
    status       TEXT,
    created_at   BIGINT,
    committed_at BIGINT
);

CREATE TABLE IF NOT EXISTS soll.RevisionChange (
    revision_id  TEXT NOT NULL,
    entity_type  TEXT NOT NULL,
    entity_id    TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    action       TEXT NOT NULL,
    before_json  JSONB,
    after_json   JSONB,
    created_at   BIGINT
);

CREATE TABLE IF NOT EXISTS soll.RevisionPreview (
    preview_id   TEXT PRIMARY KEY,
    author       TEXT,
    project_code TEXT NOT NULL DEFAULT '',
    payload      JSONB,
    created_at   BIGINT
);

-- Evidence artifacts (commit shas, file paths, dashboards, metrics).
CREATE TABLE IF NOT EXISTS soll.Traceability (
    id                  TEXT PRIMARY KEY,
    soll_entity_type    TEXT NOT NULL,
    soll_entity_id      TEXT NOT NULL,
    artifact_type       TEXT NOT NULL,
    artifact_ref        TEXT NOT NULL,
    confidence          DOUBLE PRECISION,
    metadata            JSONB,
    created_at          BIGINT,
    artifact_status     TEXT,
    artifact_checked_at TIMESTAMPTZ
);

-- Async job state for MCP tools (axon_commit_work, soll_apply_plan, …).
CREATE TABLE IF NOT EXISTS soll.McpJob (
    job_id            TEXT PRIMARY KEY,
    tool_name         TEXT,
    status            TEXT,
    submitted_at      BIGINT,
    started_at        BIGINT,
    finished_at       BIGINT,
    request_json      JSONB,
    reserved_ids_json JSONB,
    result_json       JSONB,
    error_text        TEXT,
    project_code      TEXT
);

-- ── Additive column migrations (live DBs created before columns existed) ──

ALTER TABLE soll.ProjectCodeRegistry DROP COLUMN IF EXISTS project_slug;
ALTER TABLE soll.ProjectCodeRegistry ADD  COLUMN IF NOT EXISTS session_pointer_json TEXT;

-- Pre-2026-04 schemas named the column `project_slug`; rename to
-- `project_code` on both Registry and RevisionPreview where applicable.
DO $migrate$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema='soll' AND table_name='registry' AND column_name='project_slug'
    ) THEN
        EXECUTE 'ALTER TABLE soll.Registry RENAME COLUMN project_slug TO project_code';
    END IF;
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema='soll' AND table_name='revisionpreview' AND column_name='project_slug'
    ) THEN
        EXECUTE 'ALTER TABLE soll.RevisionPreview RENAME COLUMN project_slug TO project_code';
    END IF;
END
$migrate$;

ALTER TABLE soll.Registry     ADD COLUMN IF NOT EXISTS last_ski BIGINT NOT NULL DEFAULT 0;
ALTER TABLE soll.Registry     ADD COLUMN IF NOT EXISTS last_prt BIGINT NOT NULL DEFAULT 0;
ALTER TABLE soll.Traceability ADD COLUMN IF NOT EXISTS artifact_status     TEXT;
ALTER TABLE soll.Traceability ADD COLUMN IF NOT EXISTS artifact_checked_at TIMESTAMPTZ;

-- ── Constraints (post-CREATE TABLE so fresh apply succeeds) ──────────

-- Canonical ID shape `TYPE-PROJ-N` (DEC-AXO-085). NOT VALID so existing
-- rows are not re-checked when bootstrap replays.
ALTER TABLE soll.Node DROP CONSTRAINT IF EXISTS soll_node_canonical_id_range;
DO $canonical_id$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.table_constraints
        WHERE constraint_schema = 'soll'
          AND table_name = 'node'
          AND constraint_name = 'soll_node_canonical_id_format'
    ) THEN
        ALTER TABLE soll.Node
            ADD CONSTRAINT soll_node_canonical_id_format
            CHECK (id ~ '^[A-Z]{3}-[A-Z][A-Z0-9]{2}-[0-9]{3,}$')
            NOT VALID;
    END IF;
END
$canonical_id$;

-- project_code shape invariant across SOLL tables.
DO $project_code_canonical$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.table_constraints
                   WHERE constraint_schema='soll' AND constraint_name='soll_node_project_code_canonical') THEN
        ALTER TABLE soll.Node ADD CONSTRAINT soll_node_project_code_canonical
            CHECK (project_code ~ '^[A-Z][A-Z0-9]{2}$') NOT VALID;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.table_constraints
                   WHERE constraint_schema='soll' AND constraint_name='soll_edge_project_code_canonical') THEN
        ALTER TABLE soll.Edge ADD CONSTRAINT soll_edge_project_code_canonical
            CHECK (project_code ~ '^[A-Z][A-Z0-9]{2}$') NOT VALID;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.table_constraints
                   WHERE constraint_schema='soll' AND constraint_name='soll_revision_project_code_canonical') THEN
        ALTER TABLE soll.Revision ADD CONSTRAINT soll_revision_project_code_canonical
            CHECK (project_code ~ '^[A-Z][A-Z0-9]{2}$') NOT VALID;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.table_constraints
                   WHERE constraint_schema='soll' AND constraint_name='soll_revchange_project_code_canonical') THEN
        ALTER TABLE soll.RevisionChange ADD CONSTRAINT soll_revchange_project_code_canonical
            CHECK (project_code ~ '^[A-Z][A-Z0-9]{2}$') NOT VALID;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.table_constraints
                   WHERE constraint_schema='soll' AND constraint_name='soll_revprev_project_code_canonical') THEN
        ALTER TABLE soll.RevisionPreview ADD CONSTRAINT soll_revprev_project_code_canonical
            CHECK (project_code ~ '^[A-Z][A-Z0-9]{2}$') NOT VALID;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.table_constraints
                   WHERE constraint_schema='soll' AND constraint_name='soll_mcpjob_project_code_canonical') THEN
        ALTER TABLE soll.McpJob ADD CONSTRAINT soll_mcpjob_project_code_canonical
            CHECK (project_code IS NULL OR project_code ~ '^[A-Z][A-Z0-9]{2}$') NOT VALID;
    END IF;
END
$project_code_canonical$;

-- ── Indexes ──────────────────────────────────────────────────────────

CREATE UNIQUE INDEX IF NOT EXISTS soll_project_code_registry_code_idx
    ON soll.ProjectCodeRegistry (project_code);

CREATE INDEX IF NOT EXISTS soll_node_project_idx
    ON soll.Node (project_code, type);
CREATE INDEX IF NOT EXISTS soll_node_status_idx
    ON soll.Node (status) WHERE status IS NOT NULL;
CREATE INDEX IF NOT EXISTS soll_node_type_idx
    ON soll.Node (type);

-- Trigram indexes only when pg_trgm is loaded (see 00_extensions.sql).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_trgm') THEN
        CREATE INDEX IF NOT EXISTS soll_node_title_trgm_idx
            ON soll.Node USING GIN (title gin_trgm_ops);
        CREATE INDEX IF NOT EXISTS soll_node_description_trgm_idx
            ON soll.Node USING GIN (description gin_trgm_ops);
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS soll_edge_project_source_idx
    ON soll.Edge (project_code, source_id);
CREATE INDEX IF NOT EXISTS soll_edge_project_target_idx
    ON soll.Edge (project_code, target_id);
-- REFINES / SUPERSEDES / SOLVES walks scan by relation_type.
CREATE INDEX IF NOT EXISTS soll_edge_relation_idx
    ON soll.Edge (relation_type);

CREATE INDEX IF NOT EXISTS soll_revision_project_idx
    ON soll.Revision (project_code, created_at);
CREATE INDEX IF NOT EXISTS soll_revision_change_project_idx
    ON soll.RevisionChange (revision_id);
-- "All revisions touching entity X" — soll_query_context impact path.
CREATE INDEX IF NOT EXISTS soll_revision_change_entity_idx
    ON soll.RevisionChange (entity_id, entity_type);

CREATE INDEX IF NOT EXISTS soll_traceability_entity_idx
    ON soll.Traceability (soll_entity_id, soll_entity_type);
-- Reverse lookup ("which SOLL nodes carry this commit as evidence").
CREATE INDEX IF NOT EXISTS soll_traceability_artifact_idx
    ON soll.Traceability (artifact_ref);
CREATE INDEX IF NOT EXISTS soll_traceability_status_idx
    ON soll.Traceability (artifact_status)
    WHERE artifact_status IS NOT NULL;

CREATE INDEX IF NOT EXISTS soll_mcp_job_status_idx
    ON soll.McpJob (status, submitted_at);
CREATE INDEX IF NOT EXISTS soll_mcp_job_project_idx
    ON soll.McpJob (project_code, status);

-- ── Functions ────────────────────────────────────────────────────────

-- Atomic per-(type, project_code) canonical-id allocator. Single round
-- trip: bumps the type-specific counter, formats `TYPE-PROJ-N` with
-- 3-digit min width naturally extending past 999, and skips any slot
-- already occupied by a soll.Node (fixture pollution / rejected ids).
-- Bounded at 1000 attempts to fail loud on a saturated counter range.
CREATE OR REPLACE FUNCTION soll.allocate_node_id(
    p_type         TEXT,
    p_project_code TEXT
) RETURNS TEXT LANGUAGE plpgsql AS $allocate_node_id$
DECLARE
    v_prefix    TEXT;
    v_col       TEXT;
    v_next      BIGINT;
    v_candidate TEXT;
    v_attempts  INT := 0;
BEGIN
    v_prefix := CASE p_type
        WHEN 'Vision'         THEN 'VIS'
        WHEN 'Pillar'         THEN 'PIL'
        WHEN 'Requirement'    THEN 'REQ'
        WHEN 'Concept'        THEN 'CPT'
        WHEN 'Decision'       THEN 'DEC'
        WHEN 'Milestone'      THEN 'MIL'
        WHEN 'Validation'     THEN 'VAL'
        WHEN 'Stakeholder'    THEN 'STK'
        WHEN 'Guideline'      THEN 'GUI'
        WHEN 'Skill'          THEN 'SKI'
        WHEN 'PromptTemplate' THEN 'PRT'
        ELSE NULL
    END;
    IF v_prefix IS NULL THEN
        RAISE EXCEPTION 'unknown_node_type:%', p_type;
    END IF;
    v_col := 'last_' || lower(v_prefix);

    INSERT INTO soll.Registry (project_code, id)
    VALUES (p_project_code, 'AXON_GLOBAL')
    ON CONFLICT (project_code) DO NOTHING;

    LOOP
        EXECUTE format(
            'UPDATE soll.Registry SET %I = %I + 1 WHERE project_code = $1 RETURNING %I',
            v_col, v_col, v_col
        ) INTO v_next USING p_project_code;
        IF v_next IS NULL THEN
            RAISE EXCEPTION 'project_code_not_registered:%', p_project_code;
        END IF;

        -- `lpad(text, 3, '0')` truncates inputs > 3 chars (PG semantics);
        -- guard with a length check to preserve natural width past 999.
        v_candidate := format('%s-%s-%s', v_prefix, p_project_code,
                      CASE WHEN v_next > 999 THEN v_next::TEXT
                           ELSE lpad(v_next::TEXT, 3, '0')
                      END);

        EXIT WHEN NOT EXISTS (SELECT 1 FROM soll.Node WHERE id = v_candidate);

        v_attempts := v_attempts + 1;
        IF v_attempts > 1000 THEN
            RAISE EXCEPTION 'allocate_node_id: too many collisions for %, last candidate=%', p_type, v_candidate;
        END IF;
    END LOOP;

    RETURN v_candidate;
END
$allocate_node_id$;

-- Defense-in-depth: reject any soll.Node INSERT whose id-segment does
-- not match the row's project_code. Brain enforces the contract at the
-- storage layer; this trigger catches direct-SQL / admin bypasses.
CREATE OR REPLACE FUNCTION soll.reject_id_project_mismatch()
RETURNS TRIGGER LANGUAGE plpgsql AS $reject_id_project_mismatch$
BEGIN
    IF split_part(NEW.id, '-', 2) <> NEW.project_code THEN
        RAISE EXCEPTION
            'id_project_mismatch: id=% project_code=%',
            NEW.id, NEW.project_code;
    END IF;
    RETURN NEW;
END
$reject_id_project_mismatch$;

-- ── Triggers ─────────────────────────────────────────────────────────

-- Atomic `CREATE OR REPLACE TRIGGER` (PG 14+) avoids the DROP+CREATE
-- race between concurrent bootstrap callers.
CREATE OR REPLACE TRIGGER soll_node_id_segment_check
    BEFORE INSERT ON soll.Node
    FOR EACH ROW EXECUTE FUNCTION soll.reject_id_project_mismatch();
