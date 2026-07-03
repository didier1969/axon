SET search_path = soll, public, "$user";

-- Axon canonical schema — SOLL evidence-changed NOTIFY on soll.Traceability.
-- REQ-AXO-902178 (complète REQ-AXO-902176 / DEC-AXO-901640).
--
-- soll_attach_evidence / soll_remove_evidence write soll.Traceability WITHOUT
-- emitting a soll.Revision, so the trg_soll_revision_notify trigger (13_*.sql)
-- never fires for an evidence change. Cross-process, the RAM SOLL snapshot then
-- stays STALE after an attach/remove — the root of soll_work_plan / verify
-- showing "no evidence attached" for a delivered Requirement that DOES carry
-- evidence in soll.Traceability (mcp_feedback #39/#40, REQ-AXO-902175).
--
-- Fix: an AFTER INSERT OR DELETE row trigger reuses the EXISTING journal channel
-- 'soll_revision_committed' (revision_docs_listener.rs already invalidates the
-- snapshot per project_code — no listener change needed). The project_code is
-- derived from the canonical entity id (`TYPE-PROJ-N` → 2nd dash-segment).
--
-- Payload shape (same as 13_*.sql; revision_id empty — this is an evidence event):
--   {"project_code":"AXO","revision_id":""}
--
-- Idempotent (CREATE OR REPLACE), no CHECK-vocabulary constraint (GUI-PRO-004 /
-- DDL-contaminates-shared-test-PG hygiene). RETURN NULL: AFTER trigger, the row
-- change already happened.

CREATE OR REPLACE FUNCTION soll.fn_soll_traceability_notify()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  entity_id text;
  proj text;
BEGIN
  -- NEW on INSERT, OLD on DELETE (referencing the absent record errors otherwise).
  IF TG_OP = 'DELETE' THEN
    entity_id := COALESCE(OLD.soll_entity_id, '');
  ELSE
    entity_id := COALESCE(NEW.soll_entity_id, '');
  END IF;
  proj := split_part(entity_id, '-', 2);
  IF proj <> '' THEN
    PERFORM pg_notify(
      'soll_revision_committed',
      jsonb_build_object('project_code', proj, 'revision_id', '')::text
    );
  END IF;
  RETURN NULL;
END;
$$;

CREATE OR REPLACE TRIGGER trg_soll_traceability_notify
AFTER INSERT OR DELETE ON soll.Traceability
FOR EACH ROW EXECUTE FUNCTION soll.fn_soll_traceability_notify();
