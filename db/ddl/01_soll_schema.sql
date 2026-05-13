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
    project_slug TEXT,
    session_pointer_json TEXT,
    registered_at_ms BIGINT NOT NULL DEFAULT (extract(epoch from now()) * 1000)::BIGINT
);

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
    last_prv BIGINT NOT NULL DEFAULT 0,
    last_rev BIGINT NOT NULL DEFAULT 0
);

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
