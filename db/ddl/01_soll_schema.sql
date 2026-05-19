-- Axon canonical schema — SOLL intent layer (DEC-AXO-082).
-- Cross-project nodes + edges + revisions + audit jobs + traceability.
-- Idempotent: safe to re-run on every startup.

CREATE SCHEMA IF NOT EXISTS soll;

-- Project registry — REQ-AXO-247. Lives in soll (not public) to match
-- the consumer code path. Columns mirror the DuckDB-era ALTER chain so
-- axon_init_project / soll_validate / axon_commit_work round-trip.
CREATE TABLE IF NOT EXISTS soll.ProjectCodeRegistry (
    project_code TEXT PRIMARY KEY,
    project_name TEXT,
    project_path TEXT,
    session_pointer_json TEXT,
    registered_at_ms BIGINT NOT NULL DEFAULT (extract(epoch from now()) * 1000)::BIGINT
);
-- REQ-AXO-90003 cleanup migration: live DBs may still carry the legacy
-- `project_slug` column from pre-2026-04 schemas. Drop it idempotently
-- (ProjectCodeRegistry) and rename it to project_code (Registry +
-- RevisionPreview). Wrapped in DO blocks so re-running the script after
-- migration is a no-op.
ALTER TABLE soll.ProjectCodeRegistry DROP COLUMN IF EXISTS project_slug;
ALTER TABLE soll.ProjectCodeRegistry ADD COLUMN IF NOT EXISTS session_pointer_json TEXT;
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

CREATE UNIQUE INDEX IF NOT EXISTS soll_project_code_registry_code_idx
    ON soll.ProjectCodeRegistry(project_code);

-- Per-project canonical-ID counter (last_vis / last_pil / last_req / …).
CREATE TABLE IF NOT EXISTS soll.Registry (
    project_code TEXT PRIMARY KEY DEFAULT 'AXON_GLOBAL',
    id TEXT NOT NULL DEFAULT 'AXON_GLOBAL',
    last_vis BIGINT NOT NULL DEFAULT 0,
    last_pil BIGINT NOT NULL DEFAULT 0,
    last_req BIGINT NOT NULL DEFAULT 0,
    last_cpt BIGINT NOT NULL DEFAULT 0,
    last_dec BIGINT NOT NULL DEFAULT 0,
    last_mil BIGINT NOT NULL DEFAULT 0,
    last_val BIGINT NOT NULL DEFAULT 0,
    last_stk BIGINT NOT NULL DEFAULT 0,
    last_gui BIGINT NOT NULL DEFAULT 0,
    last_ski BIGINT NOT NULL DEFAULT 0,
    last_prt BIGINT NOT NULL DEFAULT 0,
    last_prv BIGINT NOT NULL DEFAULT 0,
    last_rev BIGINT NOT NULL DEFAULT 0
);
-- REQ-AXO-91578: SKI (Skill) entity type counter — additive migration
-- for existing live DBs where Registry was created before SKI was added.
ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_ski BIGINT NOT NULL DEFAULT 0;
-- REQ-AXO-91579: PRT (PromptTemplate) entity type counter.
ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_prt BIGINT NOT NULL DEFAULT 0;

-- Intent graph: nodes + edges. Both carry `project_code` so a single
-- DB hosts multi-tenant SOLL.
CREATE TABLE IF NOT EXISTS soll.Node (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    title TEXT,
    description TEXT,
    status TEXT,
    metadata JSONB
);

-- DEC-AXO-085: canonical ID format enforcement.
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

-- DEC-AXO-085: project_code invariant across SOLL tables.
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

CREATE TABLE IF NOT EXISTS soll.Edge (
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    metadata JSONB,
    PRIMARY KEY (source_id, target_id, relation_type)
);

-- Revision audit trail.
CREATE TABLE IF NOT EXISTS soll.Revision (
    revision_id TEXT PRIMARY KEY,
    project_code TEXT NOT NULL DEFAULT '',
    author TEXT,
    source TEXT,
    summary TEXT,
    status TEXT,
    created_at BIGINT,
    committed_at BIGINT
);

CREATE TABLE IF NOT EXISTS soll.RevisionChange (
    revision_id TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    action TEXT NOT NULL,
    before_json JSONB,
    after_json JSONB,
    created_at BIGINT
);

CREATE TABLE IF NOT EXISTS soll.RevisionPreview (
    preview_id TEXT PRIMARY KEY,
    author TEXT,
    project_code TEXT NOT NULL DEFAULT '',
    payload JSONB,
    created_at BIGINT
);

-- Evidence attachments (artifact_ref pointing at files, commits, etc.).
CREATE TABLE IF NOT EXISTS soll.Traceability (
    id TEXT PRIMARY KEY,
    soll_entity_type TEXT NOT NULL,
    soll_entity_id TEXT NOT NULL,
    artifact_type TEXT NOT NULL,
    artifact_ref TEXT NOT NULL,
    confidence DOUBLE PRECISION,
    metadata JSONB,
    created_at BIGINT
);

-- REQ-AXO-320 — filesystem-state-in-DB for evidence artifacts.
-- Eliminates the per-requirement Path::exists() N+1 in
-- broken_file_evidence_counts_by_requirement: instead of a syscall per
-- artifact_ref, we read from this column. Refreshed by a lazy sweeper
-- (TTL-driven on read, or explicitly via maintenance call). Values:
-- 'present', 'broken', 'directory', 'unknown'.
ALTER TABLE soll.Traceability
    ADD COLUMN IF NOT EXISTS artifact_status TEXT,
    ADD COLUMN IF NOT EXISTS artifact_checked_at TIMESTAMPTZ;
CREATE INDEX IF NOT EXISTS soll_traceability_status_idx
    ON soll.Traceability (artifact_status)
    WHERE artifact_status IS NOT NULL;

-- REQ-AXO-247 — McpJob mirror of DuckDB-era init_schema:1385.
-- axon_commit_work + soll_apply_plan persist async-job state here;
-- without it those tools fail under PG.
CREATE TABLE IF NOT EXISTS soll.McpJob (
    job_id TEXT PRIMARY KEY,
    tool_name TEXT,
    status TEXT,
    submitted_at BIGINT,
    started_at BIGINT,
    finished_at BIGINT,
    request_json JSONB,
    reserved_ids_json JSONB,
    result_json JSONB,
    error_text TEXT,
    project_code TEXT
);

-- ── Indexes for hot SOLL multi-tenant lookups ────────────────────────

CREATE INDEX IF NOT EXISTS soll_mcp_job_status_idx
    ON soll.McpJob (status, submitted_at);
CREATE INDEX IF NOT EXISTS soll_mcp_job_project_idx
    ON soll.McpJob (project_code, status);

CREATE INDEX IF NOT EXISTS soll_node_project_idx
    ON soll.Node (project_code, type);
CREATE INDEX IF NOT EXISTS soll_node_status_idx
    ON soll.Node (status) WHERE status IS NOT NULL;

-- DEC-AXO-082 follow-up: hot-path indexes on Node for typed lookups
-- (status filter is partial, so add an unconditional type index for
-- broad scans like soll_query_context("all DEC")).
CREATE INDEX IF NOT EXISTS soll_node_type_idx
    ON soll.Node (type);
-- Trigram indexes are optional (depend on pg_trgm extension being
-- available). Skipped gracefully if the extension is missing.
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
-- DEC-AXO-082 follow-up: relation_type filter is the second-hottest
-- after source_id (used by REFINES / SOLVES / SUPERSEDES walks).
CREATE INDEX IF NOT EXISTS soll_edge_relation_idx
    ON soll.Edge (relation_type);

CREATE INDEX IF NOT EXISTS soll_revision_project_idx
    ON soll.Revision (project_code, created_at);
CREATE INDEX IF NOT EXISTS soll_revision_change_project_idx
    ON soll.RevisionChange (revision_id);
-- DEC-AXO-082 follow-up: lookups of "all changes touching entity X"
-- are common in soll_query_context impact-of-change paths.
CREATE INDEX IF NOT EXISTS soll_revision_change_entity_idx
    ON soll.RevisionChange (entity_id, entity_type);

CREATE INDEX IF NOT EXISTS soll_traceability_entity_idx
    ON soll.Traceability (soll_entity_id, soll_entity_type);
-- DEC-AXO-082 follow-up: artifact_ref reverse-lookup ("which SOLL
-- nodes carry this commit as evidence") used by axon_commit_work.
CREATE INDEX IF NOT EXISTS soll_traceability_artifact_idx
    ON soll.Traceability (artifact_ref);

-- pg_trgm is created in 00_extensions.sql (loaded first).

-- ── MIL-AXO-020 slice 1 — LLM-blind allocation + id-segment trigger ──
-- Atomic per-(type, project_code) counter, single round-trip. Supersedes
-- the read-modify-write SELECT/UPDATE pair in storage.rs that allowed
-- concurrent callers to read the same value. Ensures the Registry row
-- exists, bumps the type-specific column, and returns the canonical id.
CREATE OR REPLACE FUNCTION soll.allocate_node_id(
    p_type TEXT,
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
        WHEN 'Skill'          THEN 'SKI'  -- REQ-AXO-91578
        WHEN 'PromptTemplate' THEN 'PRT'  -- REQ-AXO-91579
        ELSE NULL
    END;
    IF v_prefix IS NULL THEN
        RAISE EXCEPTION 'unknown_node_type:%', p_type;
    END IF;
    v_col := 'last_' || lower(v_prefix);

    INSERT INTO soll.Registry (project_code, id)
    VALUES (p_project_code, 'AXON_GLOBAL')
    ON CONFLICT (project_code) DO NOTHING;

    -- REQ-AXO-90006 — gap-skipping allocator. The counter may have been
    -- polluted by past fixtures (e.g. REQ-AXO-9001/9999/90001) leaving
    -- gaps. After resetting the counter to a low canonical value, the
    -- loop transparently skips any slot already occupied by a soll.Node
    -- (rejected or otherwise) so callers always get a free id without
    -- PK collisions. Bounded at 1000 attempts to avoid runaway loops on
    -- a fully saturated counter range — well above any realistic gap
    -- cluster (AXO max fixture cluster = 6 ids).
    LOOP
        EXECUTE format(
            'UPDATE soll.Registry SET %I = %I + 1 WHERE project_code = $1 RETURNING %I',
            v_col, v_col, v_col
        ) INTO v_next USING p_project_code;
        IF v_next IS NULL THEN
            RAISE EXCEPTION 'project_code_not_registered:%', p_project_code;
        END IF;

        -- MIL-AXO-020 rule 7: `N` zéro-paddé 3 chiffres min, largeur
        -- naturelle au-delà de 999. `lpad(text, 3, '0')` TRUNCATES
        -- inputs longer than 3 chars (PG semantics), so guard with a
        -- length check before padding to preserve the natural width
        -- past 999.
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

-- Defense-in-depth: any INSERT into soll.Node whose id-segment does not
-- match the row's project_code is rejected. Brain enforces the LLM
-- contract first (slice 2) — this trigger catches admin/direct-SQL
-- bypasses. NOT VALID style: existing legacy rows are not re-checked.
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

-- REQ-AXO-91562 — atomic CREATE OR REPLACE TRIGGER (PG 14+) replaces
-- the legacy DROP + CREATE pair so concurrent test bootstrap calls
-- don't race on the "trigger already exists" symptom.
CREATE OR REPLACE TRIGGER soll_node_id_segment_check
    BEFORE INSERT ON soll.Node
    FOR EACH ROW EXECUTE FUNCTION soll.reject_id_project_mismatch();
