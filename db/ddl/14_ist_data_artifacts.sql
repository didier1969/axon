-- Axon canonical schema — IST data artifacts (REQ-AXO-902017).
--
-- A data-centric project (finance / ML / ETL) keeps ~half of an agent's
-- environment-understanding in DATA artifacts (CSV lakes, fixtures, manifests),
-- not code. Slice 1 answered the normalized catalog on demand; this slice
-- PERSISTS those artifacts INTO the IST so they participate in the structural
-- graph: each artifact is an ist.Symbol node (kind='data_artifact') and a code
-- symbol that reads it gets a READS_ARTIFACT edge in ist.Edge. The rich
-- metadata that does not fit the symbol shape (row/col counts, manifest,
-- columns, provenance) lives in this companion table keyed by the same id.
--
-- The code-indexing pipeline never touches .csv files (unsupported extension),
-- so these nodes are owned solely by the data-artifact ingestion pass
-- (data_catalog action=index): it upserts present artifacts and prunes stale
-- ones scoped to kind='data_artifact'. The `ist` schema stays disposable.
--
-- Idempotent: safe to re-run on every startup.

CREATE SCHEMA IF NOT EXISTS ist;
SET search_path = ist, "$user", public;

CREATE TABLE IF NOT EXISTS ist.DataArtifact (
    id             TEXT PRIMARY KEY,
    project_code   TEXT    NOT NULL REFERENCES axon.Project(code) ON DELETE CASCADE,
    name           TEXT    NOT NULL,
    artifact_kind  TEXT,
    file_path      TEXT,
    rows_count     BIGINT,
    cols_count     INTEGER,
    bytes_size     BIGINT,
    manifest_path  TEXT,
    source         TEXT,
    columns        JSONB,
    date_range     JSONB,
    has_manifest   BOOLEAN NOT NULL DEFAULT FALSE,
    discovered_ms  BIGINT  NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS dataartifact_project_kind_idx
    ON ist.DataArtifact (project_code, artifact_kind);
