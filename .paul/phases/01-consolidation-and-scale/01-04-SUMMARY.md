---
phase: 01-consolidation-and-scale
plan: 04
subsystem: platform
tags: [multi-repo, analytics, cli, mcp, events-jsonl, axon-stats]

requires:
  - phase: 01-03
    provides: 12-language parser coverage, stable ingestion pipeline

provides:
  - Multi-repo MCP query routing via optional `repo=` param on axon_query/context/impact
  - Fire-and-forget analytics logging to ~/.axon/events.jsonl on every query and index run
  - `axon stats` CLI command aggregating usage metrics from events.jsonl

affects: [any future MCP tooling, CLI features, observability work]

tech-stack:
  added: []
  patterns:
    - repo= param opens/closes KuzuBackend per request (no caching, safe for read-only)
    - Analytics via log_event() never raises — BLE001 catch-all, debug-logged only
    - events.jsonl global at ~/.axon/events.jsonl (one log for all repos on the machine)

key-files:
  created:
    - src/axon/core/analytics.py
    - tests/core/test_analytics.py
  modified:
    - src/axon/mcp/tools.py
    - src/axon/mcp/server.py
    - src/axon/core/ingestion/pipeline.py
    - src/axon/cli/main.py
    - tests/mcp/test_tools.py
    - tests/cli/test_main.py

key-decisions:
  - "events.jsonl at ~/.axon/events.jsonl (global): one log for all repos on the machine"
  - "log_event() never raises — BLE001 catch-all: analytics failure never blocks main flow"
  - "repo= param opens/closes backend per request (no cache): safe for read-only, no connection leaks"

patterns-established:
  - "MCP optional repo routing: _load_repo_storage() reads meta.json from ~/.axon/repos/{repo}/"
  - "Analytics: log_event(type, **kwargs) → JSONL append, fire-and-forget"
  - "CLI aggregation: parse events.jsonl, group by type/repo, print with rich"

duration: ~20min
started: 2026-02-27T00:00:00Z
completed: 2026-02-27T00:00:00Z
---

# Phase 1 Plan 04: Platform Features Summary

**Multi-repo MCP query routing, fire-and-forget analytics to ~/.axon/events.jsonl, and `axon stats` CLI command added — completing the v0.4 milestone with 751+ tests passing.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~20 min |
| Tasks | 3 completed |
| Files modified | 8 |
| Tests | 751 pass (18 new tests; pre-existing flaky async test excluded) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Multi-repo query routing | Pass | `repo=` param on query/context/impact; missing repo → error string |
| AC-2: Usage analytics event logging | Pass | events.jsonl appended on every MCP call and index run; never raises |
| AC-3: axon stats CLI command | Pass | Reads events.jsonl, prints totals, top queries, per-repo activity |
| AC-4: No regressions | Pass | 751 tests pass; 1 pre-existing flaky test unrelated to this plan |

## Accomplishments

- `_load_repo_storage(repo)` reads `~/.axon/repos/{repo}/meta.json` → opens read-only KuzuBackend; missing repo returns error string rather than exception
- `log_event()` in `src/axon/core/analytics.py` is a single fire-and-forget function; wrapped in `try/except Exception` with `logger.debug` on failure
- `axon stats` command reads events.jsonl, aggregates query counts, unique queries, top-5, index runs, and last activity per repo; handles missing file gracefully

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/analytics.py` | Created | `log_event()` — append to ~/.axon/events.jsonl |
| `src/axon/mcp/tools.py` | Modified | `repo=` routing + analytics calls on query/context/impact |
| `src/axon/mcp/server.py` | Modified | `repo` field in TOOLS inputSchema + dispatch |
| `src/axon/core/ingestion/pipeline.py` | Modified | `log_event("index", ...)` after run_pipeline completes |
| `src/axon/cli/main.py` | Modified | `axon stats` command |
| `tests/core/test_analytics.py` | Created | 6 tests: file creation, append, never-raises, fields |
| `tests/mcp/test_tools.py` | Modified | 8 tests: repo routing, missing repo, _load_repo_storage |
| `tests/cli/test_main.py` | Modified | 5 tests: stats no-file, with events, bad lines |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| events.jsonl global at `~/.axon/events.jsonl` | One log for all repos on the machine; consistent with global registry at `~/.axon/repos/` | Single aggregation point for `axon stats` |
| `log_event()` never raises | Analytics must never block main flow; `BLE001` catch-all with `logger.debug` | Any I/O failure is silently swallowed |
| `repo=` opens/closes backend per request | No connection caching needed for read-only queries; avoids connection leaks | Each MCP call is fully isolated |

## Deviations from Plan

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Auto-fixed | 0 | - |
| Scope additions | 0 | - |
| Deferred | 1 | Pre-existing flaky test noted |

**Total impact:** None — plan executed exactly as written.

### Deferred Items

- Pre-existing flaky test: `tests/core/test_pipeline.py::TestRunPipelineProgressIncludesNewPhases::test_run_pipeline_progress_includes_new_phases` — race condition in async embeddings background thread, present before 01-04 changes. Logged in STATE.md deferred issues.

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| Pre-existing async embeddings race in test_pipeline.py | Confirmed via git stash — predates 01-04; noted in UNIFY deferred items; no fix applied |

## Next Phase Readiness

**Ready:**
- v0.4 milestone complete — all 4 plans executed and unified
- 12 languages, multi-repo MCP queries, analytics, CLI stats all operational
- Codebase at version 0.4.0

**Concerns:**
- Pre-existing flaky test (async embeddings race) remains deferred

**Blockers:**
- None — ready for `/paul:complete-milestone`

---
*Phase: 01-consolidation-and-scale, Plan: 04*
*Completed: 2026-02-27*
