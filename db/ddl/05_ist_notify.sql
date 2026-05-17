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

DROP TRIGGER IF EXISTS trg_ist_notify_symbol ON public.symbol;
CREATE TRIGGER trg_ist_notify_symbol
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

DROP TRIGGER IF EXISTS trg_ist_notify_edge ON public.edge;
CREATE TRIGGER trg_ist_notify_edge
AFTER INSERT OR UPDATE OR DELETE ON public.edge
FOR EACH ROW EXECUTE FUNCTION public.fn_ist_notify_edge();

-- ── chunk_pending_embed trigger (REQ-AXO-90009 Slice 1, DEC-AXO-086) ──────
-- Fires on every Chunk INSERT or content_hash UPDATE. Payload is the
-- bare chunk id so the Rust listener can `EmbedderRuntimeState::mark_pending`
-- without parsing JSON. Zero row writes (`PERFORM` only) ; design goal is
-- "1 trigger, 0 row writes" per DEC-AXO-086 to keep the embedding pipeline
-- additive on the WAL.
CREATE OR REPLACE FUNCTION public.fn_notify_chunk_pending()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  PERFORM pg_notify('chunk_pending_embed', NEW.id);
  RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS trg_chunk_notify_pending ON public.Chunk;
CREATE TRIGGER trg_chunk_notify_pending
AFTER INSERT OR UPDATE OF content_hash ON public.Chunk
FOR EACH ROW EXECUTE FUNCTION public.fn_notify_chunk_pending();
