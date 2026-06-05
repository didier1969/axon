-- ════════════════════════════════════════════════════════════════════
-- Axon canonical schema — SOLL intent layer (single source of truth).
-- Cross-project intent graph: Node + Edge + Revision + Evidence + audit jobs.
-- Multi-tenant via `project_code` ('^[A-Z][A-Z0-9]{2}$').
-- Canonical ID format `TYPE-PROJ-N` enforced by trigger + CHECK (DEC-AXO-085).
--
-- DEFINE-ONCE: every column/constraint/index lives in (or directly next to)
-- its CREATE TABLE. NO post-CREATE additive ALTER, NO DROP, NO RENAME, NO
-- DO $migrate$ block, NO `project_slug` anywhere. This file fully replaces
-- BOTH the previous ALTER-laden .sql AND the hand-rolled VARCHAR-typed
-- ensure_additive_soll_schema() in graph_bootstrap.rs (which was a
-- type-degraded, incomplete mirror — never an authority for any column).
--
-- Idempotent: CREATE ... IF NOT EXISTS / CREATE OR REPLACE throughout, safe
-- to re-run on every startup. Bootstrap executes each statement separately;
-- CREATE EXTENSION lives in 00_extensions.sql (loaded first).
--
-- CHECK constraints are still added post-CREATE via guarded DO blocks and
-- marked NOT VALID — this is DELIBERATE (so a bootstrap replay / live in-place
-- upgrade never re-validates pre-existing rows) and is NOT migration cruft.
-- The trigram GIN indexes are likewise guarded on pg_extension because
-- pg_trgm is optional on minimal installs.
-- ════════════════════════════════════════════════════════════════════

CREATE SCHEMA IF NOT EXISTS soll;

-- ── Tables (ordered so soft-FK dependencies resolve) ──────────────────

-- Project registry — the soft-FK target for every per-project table's
-- project_code (normalize_* guards verify membership here). Lives in `soll`
-- (not `public`) by design, REQ-AXO-247. Define-once: project_name,
-- project_path, session_pointer_json (REQ-AXO-143) and registered_at_ms are
-- all inline (previously bolted on via ALTER in both the .sql and the Rust
-- path; session_pointer_json + registered_at_ms were the divergent ones).
CREATE TABLE IF NOT EXISTS soll.ProjectCodeRegistry (
    project_code         TEXT PRIMARY KEY,
    project_name         TEXT,
    project_path         TEXT,
    session_pointer_json TEXT,
    registered_at_ms     BIGINT NOT NULL DEFAULT (extract(epoch from now()) * 1000)::BIGINT
);

-- Per-project canonical-ID counter. One row per project_code; counters
-- bumped atomically by soll.allocate_node_id (VIS/PIL/REQ/CPT/DEC/MIL/VAL/
-- STK/GUI/SKI/PRT) and directly by storage.rs for PRV/REV. ALL 15 columns
-- are load-bearing — storage.rs INSERTs the full column list.
CREATE TABLE IF NOT EXISTS soll.Registry (
    project_code TEXT   PRIMARY KEY DEFAULT 'AXON_GLOBAL',
    id           TEXT   NOT NULL DEFAULT 'AXON_GLOBAL',
    last_vis     BIGINT NOT NULL DEFAULT 0,  -- Vision
    last_pil     BIGINT NOT NULL DEFAULT 0,  -- Pillar
    last_req     BIGINT NOT NULL DEFAULT 0,  -- Requirement
    last_cpt     BIGINT NOT NULL DEFAULT 0,  -- Concept
    last_dec     BIGINT NOT NULL DEFAULT 0,  -- Decision
    last_mil     BIGINT NOT NULL DEFAULT 0,  -- Milestone
    last_val     BIGINT NOT NULL DEFAULT 0,  -- Validation
    last_stk     BIGINT NOT NULL DEFAULT 0,  -- Stakeholder
    last_gui     BIGINT NOT NULL DEFAULT 0,  -- Guideline
    last_ski     BIGINT NOT NULL DEFAULT 0,  -- Skill          (REQ-AXO-91578)
    last_prt     BIGINT NOT NULL DEFAULT 0,  -- PromptTemplate  (REQ-AXO-91579)
    last_prv     BIGINT NOT NULL DEFAULT 0,  -- RevisionPreview (storage.rs direct alloc)
    last_rev     BIGINT NOT NULL DEFAULT 0   -- Revision        (storage.rs direct alloc)
);

-- Intent graph: nodes. metadata is JSONB (consumers query
-- metadata->>'logical_key' — hard JSONB requirement, storage.rs:187).
CREATE TABLE IF NOT EXISTS soll.Node (
    id           TEXT PRIMARY KEY,
    type         TEXT NOT NULL,
    project_code TEXT NOT NULL DEFAULT '',
    title        TEXT,
    description  TEXT,
    status       TEXT,
    metadata     JSONB
);

-- Intent graph: edges. Composite PK lets the same source/target pair carry
-- multiple typed relations (REFINES, SUPERSEDES, INHERITS_FROM, …) and drives
-- ON CONFLICT (source_id, target_id, relation_type) DO NOTHING in consumers.
-- project_code is inline here (was create-then-ALTER in the Rust path).
CREATE TABLE IF NOT EXISTS soll.Edge (
    source_id     TEXT NOT NULL,
    target_id     TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    project_code  TEXT NOT NULL DEFAULT '',
    metadata      JSONB,
    PRIMARY KEY (source_id, target_id, relation_type)
);

-- Revision audit trail. Each Revision groups N RevisionChange rows (one per
-- touched SOLL entity) so soll_rollback_revision is atomic.
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

-- Per-entity change log within a revision (no PK by design — N rows per
-- revision_id). before_json/after_json are JSONB.
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

-- Staged (uncommitted) revision payloads. payload is JSONB.
CREATE TABLE IF NOT EXISTS soll.RevisionPreview (
    preview_id   TEXT PRIMARY KEY,
    author       TEXT,
    project_code TEXT NOT NULL DEFAULT '',
    payload      JSONB,
    created_at   BIGINT
);

-- Evidence artifacts (commit shas, file paths, dashboards, metrics).
-- artifact_status / artifact_checked_at (REQ-AXO-320 sweeper) are inline
-- here — they were DDL-only columns the Rust path NEVER created.
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
-- *_json columns are JSONB. project_code is the sole nullable project_code
-- in the schema (its CHECK allows NULL). Defined inline (was create-then-
-- ALTER in the Rust path).
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

-- ── CHECK constraints (post-CREATE, NOT VALID — deliberate, not cruft) ────

-- Canonical ID shape `TYPE-PROJ-N` (DEC-AXO-085).
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

-- project_code shape invariant across SOLL tables. McpJob allows NULL.
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

-- Atomic per-(type, project_code) canonical-id allocator. Bumps the
-- type-specific counter, formats `TYPE-PROJ-N` (3-digit min width, natural
-- past 999), skips slots already occupied in soll.Node, bounded 1000 tries.
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
