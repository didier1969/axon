-- REQ-AXO-901961 — MCP per-call telemetry, time-bucketed aggregate.
-- The rollup IS the table: one row per (tool, project, status, hour), upserted
-- per call — bounded by construction (~tools × statuses × projects × hours), no
-- raw-event explosion. Signature-only: tool + ok/error + project, NEVER any
-- argument content (PIL-AXO-9003 commercial privacy). Instantiates
-- PIL-AXO-9004 (event-sourced projection) + PIL-AXO-9006 (closed-loop
-- observability) for PIL-AXO-002 (agent-native surface). Hook: the dispatch
-- chokepoint upserts one bucket per call, best-effort (a telemetry write never
-- affects the tool response). Average latency = latency_sum_ms / call_count;
-- latency_max_ms surfaces tail outliers. Exact percentiles are a later slice
-- (latency histogram) — average + max answer the first analytics need.
CREATE SCHEMA IF NOT EXISTS axon;

CREATE TABLE IF NOT EXISTS axon.mcp_call_stat (
    tool             TEXT        NOT NULL,
    project_code     TEXT        NOT NULL DEFAULT '',
    status           TEXT        NOT NULL DEFAULT 'ok',   -- 'ok' | 'error'
    bucket_hour      TIMESTAMPTZ NOT NULL,
    call_count       BIGINT      NOT NULL DEFAULT 0,
    latency_sum_ms   BIGINT      NOT NULL DEFAULT 0,
    latency_max_ms   INTEGER     NOT NULL DEFAULT 0,
    contract_version TEXT        NOT NULL DEFAULT '',
    PRIMARY KEY (tool, project_code, status, bucket_hour)
);

-- Recent-window scans (analytics projections + retention sweeps).
CREATE INDEX IF NOT EXISTS mcp_call_stat_recent_idx
    ON axon.mcp_call_stat (bucket_hour DESC, tool);
