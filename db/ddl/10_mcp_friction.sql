-- REQ-AXO-901957 — MCP friction log (closed-loop self-improvement).
-- Event-sourced signature table (PIL-AXO-9004): ONE row per distinct problem
-- SHAPE — never any argument content (PIL-AXO-9003 commercial privacy: "Axon
-- improves from your friction without ever seeing your data"). Instantiates
-- PIL-AXO-9006 (closed-loop observability) for PIL-AXO-002 (agent-native
-- surface). Hook: the dispatch layer upserts a signature whenever a tool
-- response carries a non-null problem_class.
CREATE SCHEMA IF NOT EXISTS axon;

CREATE TABLE IF NOT EXISTS axon.mcp_friction (
    id                BIGSERIAL PRIMARY KEY,
    -- signature (the SHAPE of the problem — no arg content ever)
    project_code      TEXT        NOT NULL DEFAULT '',
    tool              TEXT        NOT NULL,
    problem_class     TEXT        NOT NULL,
    field_in_error    TEXT        NOT NULL DEFAULT '',
    -- observation
    first_observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_observed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    occurrence_count  BIGINT      NOT NULL DEFAULT 1,
    contract_version  TEXT        NOT NULL DEFAULT '',
    -- closed-loop lifecycle
    status            TEXT        NOT NULL DEFAULT 'open',
    resolved_at       TIMESTAMPTZ,
    resolved_by_req   TEXT,
    resolved_by_val   TEXT,
    resolution_note   TEXT,
    CONSTRAINT mcp_friction_signature_uniq
        UNIQUE (project_code, tool, problem_class, field_in_error)
);

CREATE INDEX IF NOT EXISTS mcp_friction_open_freq_idx
    ON axon.mcp_friction (status, occurrence_count DESC);
