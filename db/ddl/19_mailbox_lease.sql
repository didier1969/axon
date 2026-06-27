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
