-- Axon canonical schema — IST mutation NOTIFY channel.
--
-- Triggers on public.symbol and public.edge fire pg_notify('ist_mutated', json)
-- on every mutation. The Rust listener (src/ist_snapshot/notify_listener.rs)
-- evicts the affected project from the process IstSnapshotCache so the next
-- read forces a cold reload.
--
-- Payload shape:
--   {"op":"insert|update|delete","project_code":"AXO","table":"symbol|edge"}
--
-- Bulk operations land as one NOTIFY per row (Postgres queues
-- notifications per transaction); the LISTEN side dedups consecutive
-- events for the same project_code.
--
-- The Chunk-pending NOTIFY (`chunk_pending_embed`) is colocated with the
-- `public.Chunk` DDL in 03_ist_schema.sql.

-- ── symbol ───────────────────────────────────────────────────────────

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
    'op',           op_kind,
    'project_code', COALESCE(proj, ''),
    'table',        'symbol'
  );
  PERFORM pg_notify('ist_mutated', payload::text);
  RETURN NULL;
END;
$$;

CREATE OR REPLACE TRIGGER trg_ist_notify_symbol
AFTER INSERT OR UPDATE OR DELETE ON public.symbol
FOR EACH ROW EXECUTE FUNCTION public.fn_ist_notify_symbol();

-- ── edge ─────────────────────────────────────────────────────────────

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
    'op',           op_kind,
    'project_code', COALESCE(proj, ''),
    'table',        'edge'
  );
  PERFORM pg_notify('ist_mutated', payload::text);
  RETURN NULL;
END;
$$;

CREATE OR REPLACE TRIGGER trg_ist_notify_edge
AFTER INSERT OR UPDATE OR DELETE ON public.edge
FOR EACH ROW EXECUTE FUNCTION public.fn_ist_notify_edge();
