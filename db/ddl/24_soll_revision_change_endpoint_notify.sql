SET search_path = soll, public, "$user";

-- REQ-AXO-902466 — a Revision announces its owning/source project through
-- 13_soll_revision_notify.sql.  For a cross-project edge, the other endpoint
-- must invalidate its snapshot too.  RevisionChange already carries the exact
-- edge payload, so derive the target tenant here instead of teaching every
-- journal subscriber how to parse relation ids.

CREATE OR REPLACE FUNCTION soll.fn_soll_revision_change_endpoint_notify()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  edge_payload jsonb;
  target_id text;
  target_project text;
BEGIN
  IF NEW.entity_type <> 'edge' THEN
    RETURN NULL;
  END IF;

  edge_payload := COALESCE(NEW.after_json, NEW.before_json, '{}'::jsonb);
  target_id := COALESCE(edge_payload->>'target_id', '');
  target_project := split_part(target_id, '-', 2);

  -- The source project has already been emitted by soll.Revision.  Emit only
  -- the distinct endpoint, avoiding duplicate same-project notifications.
  IF target_project <> '' AND target_project <> COALESCE(NEW.project_code, '') THEN
    PERFORM pg_notify(
      'soll_revision_committed',
      jsonb_build_object(
        'project_code', target_project,
        'revision_id', COALESCE(NEW.revision_id, '')
      )::text
    );
  END IF;
  RETURN NULL;
END;
$$;

CREATE OR REPLACE TRIGGER trg_soll_revision_change_endpoint_notify
AFTER INSERT ON soll.RevisionChange
FOR EACH ROW EXECUTE FUNCTION soll.fn_soll_revision_change_endpoint_notify();
