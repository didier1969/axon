-- REQ-AXO-902328 — ce fichier est appliqué par le brain à CHAQUE boot.
-- Il ne l'était PAS avant le 2026-08-25 : il manquait à la liste `include_str!`
-- écrite à la main de `postgres/ddl.rs` (9 fichiers sur 25 absents), donc il
-- n'avait jamais reçu la discipline de REQ-AXO-902339 — « ne pas réclamer un
-- verrou avant de tester si l'on a quelque chose à faire ». `ADD COLUMN IF NOT
-- EXISTS`, `CREATE INDEX IF NOT EXISTS`, `DROP INDEX/TRIGGER IF EXISTS` prennent
-- tous leur verrou AVANT le test d'existence : sur `axon.practice` et
-- `axon.mailbox_message`, écrites en continu, c'est une famine, pas une course.
-- Les `ADD COLUMN` de ce fichier passent désormais par `add_column_if_absent`.
-- REQ-AXO-902475 — l'ensemble des `CREATE INDEX IF NOT EXISTS` et `DROP` sur les
-- 25 fichiers passe désormais par les gardes catalogue lock-free
-- `create_index_if_absent`, `drop_index_if_present`, `drop_trigger_if_present`.

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
SELECT public.create_index_if_absent('axon', 'mailbox_lease_resource_idx', $idx$
    CREATE INDEX mailbox_lease_resource_idx ON axon.mailbox_lease (resource)
$idx$);
