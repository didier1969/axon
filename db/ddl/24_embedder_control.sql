SET search_path = ist, public, "$user";

-- Axon canonical schema — embedder CONTROL plane (REQ-AXO-902234 VOLET 1).
--
-- DESIRED state, brain → indexer. Strictly the mirror-image of
-- `axon.EmbedderLifecycleHeartbeat` (02_axon_runtime.sql), which carries the
-- OBSERVED state indexer → brain and which the brain may only READ
-- (DEC-AXO-901626: observation happens where the observed thing lives). Control
-- and observation MUST stay separate tables: folding a desired-state column into
-- the heartbeat row would make the indexer both writer and reader of its own
-- control signal, and the brain a writer of an observation row.
--
-- WHY a PG row instead of an in-process atomic: the idle-drop watchdog
-- (`pipeline/embedder_gpu.rs::spawn_idle_watchdog`) runs inside `axon-indexer`,
-- while the `idle_drop` MCP tool runs inside `axon-brain` — two OS processes.
-- The `embed_provider` pattern (AtomicU8 in `embedder.rs`) works only because the
-- query worker is co-located with the MCP dispatch; it cannot cross this
-- boundary. A durable row ALSO fixes the original defect (REQ-AXO-902234): the
-- opt-in was an env var read once at boot, so an activation was lost on every
-- restart/reboot.
--
-- Seeding (D1, operator decision): the indexer seeds this row from
-- `AXON_EMBEDDER_IDLE_DROP` / `AXON_EMBEDDER_IDLE_SECONDS` with
-- `ON CONFLICT DO NOTHING` — never an UPDATE, or every restart would clobber a
-- setting made at runtime. The env therefore stays a boot-time SEED (belt and
-- braces on a fresh DB) while the row is authoritative once it exists.
--
-- Payload shape (channel `embedder_control`):
--   {"process_role":"indexer","idle_drop_enabled":true,"idle_seconds":20}
--
-- Deliberately NO CHECK constraint on `process_role` / vocabulary: a vocabulary
-- CHECK in bootstrap DDL contaminates the shared test PG and fails closed on any
-- new role (feedback_ddl_constraint_contaminates_shared_test_pg). Validation of
-- accepted values belongs to the MCP tool schema (GUI-AXO-1026 invariant 3:
-- enforced at dispatch), not to the storage layer.
--
-- Idempotent: safe to re-run on every startup.

CREATE TABLE IF NOT EXISTS axon.EmbedderControl (
    process_role      TEXT    PRIMARY KEY,   -- 'indexer' | 'brain'
    idle_drop_enabled BOOLEAN NOT NULL,
    idle_seconds      INT     NOT NULL,
    updated_ms        BIGINT  NOT NULL,
    updated_by        TEXT                   -- 'mcp:idle_drop' | 'boot_seed:env' | NULL
);

-- Fires on the brain's write so the indexer flips its process-global atomics
-- without a restart. Trigger shape mirrors ist.fn_ist_notify_symbol
-- (05_ist_notify.sql): AFTER, FOR EACH ROW, RETURN NULL.
CREATE OR REPLACE FUNCTION axon.fn_embedder_control_notify()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  payload jsonb;
BEGIN
  payload := jsonb_build_object(
    'process_role',      COALESCE(NEW.process_role, ''),
    'idle_drop_enabled', NEW.idle_drop_enabled,
    'idle_seconds',      NEW.idle_seconds
  );
  PERFORM pg_notify('embedder_control', payload::text);
  RETURN NULL;
END;
$$;

CREATE OR REPLACE TRIGGER trg_embedder_control_notify
AFTER INSERT OR UPDATE ON axon.EmbedderControl
FOR EACH ROW EXECUTE FUNCTION axon.fn_embedder_control_notify();
