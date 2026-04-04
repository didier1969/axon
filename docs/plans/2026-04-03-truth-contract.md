# Axon Truth Contract (Writer vs Reader)

Date: 2026-04-03
Status: active

## Canonical Source

1. All critical counters (`File`, `Symbol`, `CALLS`, `CALLS_NIF`, `CONTAINS`) are canonical on SQL gateway writer-path.
2. Reader-path is a performance path, never the canonical source for operator truth.

## Runtime Rules

1. Reader snapshot is refreshed periodically (`AXON_READER_REFRESH_INTERVAL_MS`, default 5000ms).
2. MCP `debug` must expose reader snapshot age and refresh failures.
3. MCP `truth_check` must expose drift between writer-canonical and reader-path counters.

## Acceptance

1. `truth_check` status is `aligned` in normal operation.
2. Any `drift_detected` must be diagnosable with `diagnose_indexing` + runtime metrics.
3. Release qualification can enforce gate (`scripts/qualify_ingestion_run.py --enforce-gate`).

## Operator Flow

1. Run `truth_check`.
2. If drift: inspect `debug` reader snapshot age/failures.
3. Run `diagnose_indexing` on affected project scope.
4. Confirm counters converge after reader refresh.
