-- REQ-AXO-91487 (MIL-AXO-019 slice 3) — IST mutation notification.
--
-- Triggers on public.symbol and public.edge fire pg_notify('ist_mutated', json)
-- on every mutation. The Rust listener (src/ist_snapshot/notify_listener.rs)
-- evicts the affected project from the process IstSnapshotCache so the next
-- read forces a cold reload.
--
-- Payload shape :
--   {"op":"insert|update|delete","project_code":"AXO","table":"symbol|edge"}
--
-- Bulk operations land as one NOTIFY per row (Postgres queues notifications
-- per transaction ; LISTEN-side dedup is responsible for collapsing
-- consecutive events for the same project_code).

-- ── symbol trigger ─────────────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION public.fn_ist_notify_symbol()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  payload jsonb;
  proj    text;
  op_kind text;
BEGIN
  IF TG_OP = 'INSERT' THEN
    proj := NEW.project_code;
    op_kind := 'insert';
  ELSIF TG_OP = 'UPDATE' THEN
    proj := NEW.project_code;
    op_kind := 'update';
  ELSE
    proj := OLD.project_code;
    op_kind := 'delete';
  END IF;
  payload := jsonb_build_object(
    'op', op_kind,
    'project_code', COALESCE(proj, ''),
    'table', 'symbol'
  );
  PERFORM pg_notify('ist_mutated', payload::text);
  RETURN NULL;
END;
$$;

-- REQ-AXO-91562 — `CREATE OR REPLACE TRIGGER` (PG 14+) is atomic ; the
-- legacy `DROP TRIGGER IF EXISTS` + `CREATE TRIGGER` pair raced under
-- parallel test threads (thread B's CREATE failed after thread A's
-- CREATE landed between B's DROP and B's CREATE).
CREATE OR REPLACE TRIGGER trg_ist_notify_symbol
AFTER INSERT OR UPDATE OR DELETE ON public.symbol
FOR EACH ROW EXECUTE FUNCTION public.fn_ist_notify_symbol();

-- ── edge trigger ───────────────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION public.fn_ist_notify_edge()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  payload jsonb;
  proj    text;
  op_kind text;
BEGIN
  IF TG_OP = 'INSERT' THEN
    proj := NEW.project_code;
    op_kind := 'insert';
  ELSIF TG_OP = 'UPDATE' THEN
    proj := NEW.project_code;
    op_kind := 'update';
  ELSE
    proj := OLD.project_code;
    op_kind := 'delete';
  END IF;
  payload := jsonb_build_object(
    'op', op_kind,
    'project_code', COALESCE(proj, ''),
    'table', 'edge'
  );
  PERFORM pg_notify('ist_mutated', payload::text);
  RETURN NULL;
END;
$$;

-- REQ-AXO-91562 — atomic CREATE OR REPLACE TRIGGER, see symbol comment.
CREATE OR REPLACE TRIGGER trg_ist_notify_edge
AFTER INSERT OR UPDATE OR DELETE ON public.edge
FOR EACH ROW EXECUTE FUNCTION public.fn_ist_notify_edge();

-- REQ-AXO-90009 Slice 1 + REQ-AXO-91562 — `trg_chunk_notify_pending`
-- + `fn_notify_chunk_pending` are co-located with the `public.Chunk`
-- DDL in `03_ist_schema.sql` (the canonical home) to keep
-- trigger/function adjacent to the table they observe. Do NOT redefine
-- here ; a duplicate `CREATE OR REPLACE TRIGGER` would lose the
-- atomicity guarantee that REQ-AXO-91562 relies on.
