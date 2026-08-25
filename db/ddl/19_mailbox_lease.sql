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

-- REQ-AXO-902120 (MBX-8) — advisory leases / cooperative edit locks.
-- Anti-collision for multi-LLM editing: a project announces its INTENT to work
-- on a `resource` (a file path, a SOLL id, a symbol, a worktree…) so peer agents
-- can SEE the conflict before they collide. This is COOPERATIVE / advisory only:
-- acquire ALWAYS grants (never blocks) but reports the live conflicting holders
-- so the caller decides. Runtime data (not SOLL intent) → `axon` schema, fully
-- reconstructible.
--
-- Why a table and NOT pg_advisory_lock: pg_advisory_lock is session-scoped and
-- vanishes the instant the connection returns to the pool (every MCP call borrows
-- a pooled conn), so a lock would never survive a single tool call. A persisted
-- row with an explicit `expires_at` is the only horizon that outlives the conn:
-- a crashed holder's lease simply ages out (expires_at < now()), which is the
-- ONLY automatic release path for a holder that never calls `release`.
CREATE SCHEMA IF NOT EXISTS axon;

CREATE TABLE IF NOT EXISTS axon.mailbox_lease (
    lease_id       BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- Opaque resource handle the holder claims an advisory lease over
    -- (file path / SOLL id / symbol / worktree name — caller-defined namespace).
    resource       TEXT        NOT NULL,
    -- Project code holding the lease (cwd-resolved or explicit `holder`).
    holder_project TEXT        NOT NULL,
    -- Free-text declared intent ("refactor tools_mailbox", "promote live"…).
    intent         TEXT        NOT NULL DEFAULT '',
    acquired_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Hard horizon: a lease with expires_at < now() is DEAD (crashed/abandoned
    -- holder). Live-holder queries filter on expires_at > now().
    expires_at     TIMESTAMPTZ NOT NULL
);

-- acquire/check scan live holders of one resource → index the hot lookup column.
CREATE INDEX IF NOT EXISTS mailbox_lease_resource_idx
    ON axon.mailbox_lease (resource);
