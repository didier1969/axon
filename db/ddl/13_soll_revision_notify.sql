SET search_path = soll, public, "$user";

-- Axon canonical schema — SOLL revision-committed NOTIFY channel.
-- REQ-AXO-309 / DEC-AXO-901640.
--
-- An AFTER INSERT trigger on soll.Revision fires
-- pg_notify('soll_revision_committed', json) so the derived-projection
-- subscribers (autodoc site today; SOLL-embedding sweep next) regenerate on any
-- SOLL mutation. This is the journal-subscriber model: ONE emitter, N
-- fire-and-forget subscribers, replacing N per-tool hooks wired into soll_manager
-- (dependency inversion — soll_manager no longer needs to know its consumers).
-- The Rust subscriber lives in src/mcp/revision_docs_listener.rs.
--
-- Payload shape:
--   {"project_code":"AXO","revision_id":"REV-AXO-..."}
--
-- soll.Revision is append-only (one row per committed revision), so INSERT is the
-- only event. Bulk commits land as one NOTIFY per row (Postgres queues
-- notifications per transaction); the LISTEN side debounces per project_code so a
-- burst regenerates the site exactly once.

CREATE OR REPLACE FUNCTION soll.fn_soll_revision_notify()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  payload jsonb;
BEGIN
  payload := jsonb_build_object(
    'project_code', COALESCE(NEW.project_code, ''),
    'revision_id',  COALESCE(NEW.revision_id, '')
  );
  PERFORM pg_notify('soll_revision_committed', payload::text);
  RETURN NULL;
END;
$$;

CREATE OR REPLACE TRIGGER trg_soll_revision_notify
AFTER INSERT ON soll.Revision
FOR EACH ROW EXECUTE FUNCTION soll.fn_soll_revision_notify();
