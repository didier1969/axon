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
SELECT public.add_column_if_absent('axon', 'mailbox_message', 'archived_at', 'TIMESTAMPTZ');

-- Partial index: the sweep and inbox reads both want "live" rows (not yet
-- archived). Keeps the hot path off the archived tail as the log grows.
CREATE INDEX IF NOT EXISTS mailbox_message_live_idx
    ON axon.mailbox_message (to_project, id)
    WHERE archived_at IS NULL;

-- MBX-7 — TTL sweep. Stamps `archived_at = now()` on every row whose retention
-- horizon has passed and that is not already archived. Idempotent (a second
-- call within the same instant archives nothing new) and returns the number of
-- rows it archived this pass, so the `mailbox_sweep` tool can report a count.
--
-- REQ-AXO-902306 — `priority='high'` is EXEMPT. The TTL is an ABSOLUTE clock, so
-- without this a project dormant longer than the horizon silently loses a notice
-- it never read. An important message never disappears on its own: it takes a
-- deliberate gesture. Ordinary messages keep expiring, which is what stops the
-- accumulation REQ-AXO-902304 measured (8217 unpurgeable broadcasts).
--
-- Note for whoever adds a caller: "urgent on arrival" and "important to keep" are
-- different things. Promote notices were sent `high` for the former reason and had
-- to be downgraded here, or they would have become immortal.
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
           AND COALESCE(priority, '') <> 'high'
        RETURNING 1
    )
    SELECT count(*) INTO swept FROM expired;
    RETURN swept;
END;
$$ LANGUAGE plpgsql;
