-- Axon canonical schema — ProjectCodeRegistry mutation NOTIFY channel.
-- REQ-AXO-901675 (PIL-AXO-008).
--
-- Trigger on soll.ProjectCodeRegistry fires pg_notify('axon_registry_changed', json)
-- on every INSERT/UPDATE. The Rust listener (src/registry_notify_listener.rs)
-- enqueues a scan_subtree(project_path) so the indexer picks up the newly
-- registered project without a manual restart.
--
-- Payload shape:
--   {"op":"insert|update","project_code":"AXO","project_path":"/home/.../axon"}
--
-- DELETE is intentionally NOT notified: registry rows are preserved per
-- SOLL discipline (CPT-AXO-019 / DEC-AXO-085 — preserve above all).
--
-- Idempotent: safe to re-run on every startup.

CREATE OR REPLACE FUNCTION soll.fn_registry_notify()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  payload jsonb;
  op_kind text;
BEGIN
  IF TG_OP = 'INSERT' THEN
    op_kind := 'insert';
  ELSIF TG_OP = 'UPDATE' THEN
    -- Skip notify when no operationally interesting field changed.
    -- project_path drives the indexer scope ; project_name is cosmetic.
    IF OLD.project_path IS NOT DISTINCT FROM NEW.project_path THEN
      RETURN NULL;
    END IF;
    op_kind := 'update';
  ELSE
    RETURN NULL;
  END IF;
  payload := jsonb_build_object(
    'op',           op_kind,
    'project_code', COALESCE(NEW.project_code, ''),
    'project_path', COALESCE(NEW.project_path, '')
  );
  PERFORM pg_notify('axon_registry_changed', payload::text);
  RETURN NULL;
END;
$$;

CREATE OR REPLACE TRIGGER trg_registry_notify
AFTER INSERT OR UPDATE ON soll.ProjectCodeRegistry
FOR EACH ROW EXECUTE FUNCTION soll.fn_registry_notify();
