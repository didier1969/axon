-- REQ-AXO-902119 (MBX-7) — MAILBOX TTL / dead-letter sweep.
-- The MVP store (db/ddl/15_mailbox.sql) carries an optional retention horizon
-- `ttl_at` (NULL = keep forever). This slice adds the archival half: a soft
-- `archived_at` watermark + an idempotent sweep that stamps it on every expired
-- row. Soft-archive (not DELETE) keeps the event-sourced append-only log
-- (PIL-AXO-9004) intact — readers filter on `archived_at IS NULL`, operators
-- can still audit or replay. Applied to live by the canonical DDL loop
-- (scripts/lib/ensure-runtime.sh apply_canonical_ddl / promote) and baked into
-- every test clone (apply_sql_dir) — both read this dir lexically, so no
-- include_str! registration is required.

-- Soft-archive watermark. IF NOT EXISTS so re-apply over an MVP store (which
-- predates this column) is a no-op.
ALTER TABLE axon.mailbox_message
    ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;

-- Partial index: the sweep and inbox reads both want "live" rows (not yet
-- archived). Keeps the hot path off the archived tail as the log grows.
CREATE INDEX IF NOT EXISTS mailbox_message_live_idx
    ON axon.mailbox_message (to_project, id)
    WHERE archived_at IS NULL;

-- MBX-7 — TTL sweep. Stamps `archived_at = now()` on every row whose retention
-- horizon has passed and that is not already archived. Idempotent (a second
-- call within the same instant archives nothing new) and returns the number of
-- rows it archived this pass, so the `mailbox_sweep` tool can report a count.
CREATE OR REPLACE FUNCTION axon.mailbox_sweep() RETURNS bigint AS $$
DECLARE
    swept bigint;
BEGIN
    WITH expired AS (
        UPDATE axon.mailbox_message
           SET archived_at = now()
         WHERE ttl_at IS NOT NULL
           AND ttl_at < now()
           AND archived_at IS NULL
        RETURNING 1
    )
    SELECT count(*) INTO swept FROM expired;
    RETURN swept;
END;
$$ LANGUAGE plpgsql;
