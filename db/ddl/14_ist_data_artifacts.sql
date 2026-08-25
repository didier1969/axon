-- REQ-AXO-902328 — ce fichier est appliqué par le brain à CHAQUE boot.
-- Il ne l'était PAS avant le 2026-08-25 : il manquait à la liste `include_str!`
-- écrite à la main de `postgres/ddl.rs` (9 fichiers sur 25 absents), donc il
-- n'avait jamais reçu la discipline de REQ-AXO-902339 — « ne pas réclamer un
-- verrou avant de tester si l'on a quelque chose à faire ». `ADD COLUMN IF NOT
-- EXISTS`, `CREATE INDEX IF NOT EXISTS`, `DROP INDEX/TRIGGER IF EXISTS` prennent
-- tous leur verrou AVANT le test d'existence : sur `axon.practice` et
-- `axon.mailbox_message`, écrites en continu, c'est une famine, pas une course.
-- Les `ADD COLUMN` de ce fichier passent désormais par `add_column_if_absent`.
--
-- ⚠️ Les `CREATE INDEX IF NOT EXISTS` NE sont PAS convertis, et c'est délibéré :
-- les 16 fichiers appliqués au boot depuis toujours en portent 26 de la même
-- forme, sans incident mesuré. Les convertir ici seulement donnerait DEUX
-- disciplines pour une seule classe d'énoncé — exactement la divergence que
-- REQ-AXO-902328 ferme. La classe entière (45 CREATE INDEX + 3 DROP nus sur les
-- 25 fichiers) est logée en REQ, à traiter d'un bloc ou pas du tout.

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
