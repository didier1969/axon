-- REQ-AXO-901966 — voluntary LLM feedback / doléance.
-- The friction log (10_mcp_friction.sql) is SILENT + signature-only (no
-- argument content, PIL-AXO-9003). This is its VOLUNTARY, content-rich
-- complement: an LLM using Axon MCP self-reports a problem it hit (bug /
-- unclear doc / undocumented / too slow / incomplete / too verbose), its
-- proposed fix, and its satisfaction — to hyper-document real LLM usage and
-- drive product optimization (NOT system-failure capture, NOT a write-to-SOLL
-- facility). Append-only event log (PIL-AXO-9004): one row per call, the
-- server stamps created_at. Serves PIL-AXO-002 (agent-native surface).
CREATE SCHEMA IF NOT EXISTS axon;

CREATE TABLE IF NOT EXISTS axon.llm_feedback (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    llm_identity      TEXT        NOT NULL DEFAULT '',
    -- bug | unclear_doc | undocumented | too_slow | incomplete | too_verbose | other
    category          TEXT        NOT NULL DEFAULT 'other',
    tool              TEXT        NOT NULL DEFAULT '',
    project_code      TEXT        NOT NULL DEFAULT '',
    problem           TEXT        NOT NULL,
    proposed_solution TEXT        NOT NULL DEFAULT '',
    satisfaction      INTEGER,  -- optional 1..5
    -- How serious is it (for triage / prioritisation):
    --   blocking    = the LLM could NOT complete its task
    --   token_cost  = it worked, but cost significant extra tokens / turns / effort
    --   minor       = cosmetic / small annoyance
    severity          TEXT        NOT NULL DEFAULT 'minor',
    contract_version  TEXT        NOT NULL DEFAULT ''
);

-- REQ-AXO-901966 follow-up — `severity` added after the initial ship; ALTER (with
-- IF NOT EXISTS, idempotent) brings pre-existing tables (test template, any
-- unpromoted dev instance) to the new shape without a drop.
ALTER TABLE axon.llm_feedback ADD COLUMN IF NOT EXISTS severity TEXT NOT NULL DEFAULT 'minor';

-- Recent-window + severity scans for the feedback report / triage.
CREATE INDEX IF NOT EXISTS llm_feedback_recent_idx
    ON axon.llm_feedback (created_at DESC, severity, category);
