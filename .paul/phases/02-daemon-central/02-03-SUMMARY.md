---
phase: 02-daemon-central
plan: 03
subsystem: mcp
tags: [axon_batch, socket, daemon, mcp, tool]

requires:
  - phase: 02-daemon-central/02-02
    provides: MCP proxy with daemon-first routing and max_tokens support

provides:
  - axon_batch MCP tool: N calls on 1 socket connection
  - _batch_daemon_call helper: sequential send/recv, formatted result
  - TestBatchTool: 6 tests covering daemon path, fallback, max_tokens, schema

affects: [v0.7 hybrid search, any future multi-call optimization]

tech-stack:
  added: []
  patterns:
    - "Batch via single socket: send N → recv N on same connection"
    - "MCP special-case before generic path in call_tool()"
    - "per-sub-result max_tokens (not total)"

key-files:
  modified:
    - src/axon/mcp/server.py
    - tests/mcp/test_server.py

key-decisions:
  - "axon_batch is MCP-layer only — daemon receives individual calls, never axon_batch"
  - "max_tokens truncates per sub-result, not total output"
  - "Fallback mirrors daemon path: iterates calls via _dispatch_tool()"

patterns-established:
  - "call_tool() special-cases axon_batch before generic slug/daemon_args resolution"
  - "_batch_daemon_call returns None (not raises) on any socket error → triggers fallback"

duration: ~15min
started: 2026-03-02T00:00:00Z
completed: 2026-03-02T00:00:00Z
---

# Phase 2 Plan 03: axon_batch Tool Summary

**`axon_batch` MCP tool added: N sequential calls on 1 Unix socket, with daemon-first routing, direct fallback, and per-sub-result max_tokens truncation.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~15 min |
| Tasks | 2 completed |
| Files modified | 2 |
| Tests before | 812 |
| Tests after | 818 (+6) |
| Test failures | 0 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Batch routes through one daemon socket | Pass | _batch_daemon_call opens 1 socket, N sequential send/recv |
| AC-2: Batch falls back to direct dispatch | Pass | None return → iterate calls via _dispatch_tool() |
| AC-3: max_tokens truncates each sub-result | Pass | Applied per-result inside _batch_daemon_call and fallback path |
| AC-4: Result format is readable | Pass | `### tool (i/N)\nresult` sections joined by `\n\n` |
| AC-5: Schema is correct | Pass | calls required, max_tokens optional, items require tool+args |
| AC-6: Empty calls returns empty string | Pass | Early return `""` before socket check |
| AC-7: All existing tests pass | Pass | 818 passed, 0 failures (target was 820+, actual +6 = 818) |

## Accomplishments

- `_batch_daemon_call()`: opens 1 socket, strips `repo` per call, sends N encode_request / reads N decode_request, formats as `### tool (i/N)\nresult`, returns None on any error
- `call_tool()` updated: `axon_batch` branch runs before generic path; daemon path via `asyncio.to_thread`; fallback iterates `_dispatch_tool()` with lock support; `max_tokens` applied per sub-result
- `axon_batch` added to `TOOLS` with correct schema (`calls` required array, `max_tokens` optional)
- 6 new tests in `TestBatchTool`: empty list, absent socket, success (2 calls), max_tokens truncation, connection error, schema validation
- All pre-existing ruff E501 warnings fixed (4 lines split) — 0 lint errors now

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/mcp/server.py` | Modified | Added _batch_daemon_call, updated call_tool(), added axon_batch to TOOLS; fixed E501s |
| `tests/mcp/test_server.py` | Modified | Added _batch_daemon_call import, TestBatchTool class (6 tests) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| axon_batch is MCP-layer only | Daemon already handles individual calls via while-True; no protocol change needed | Daemon unchanged, zero risk |
| max_tokens per sub-result (not total) | Agents need full results for each query; total truncation would lose later results silently | Each sub-result independently bounded |
| None return from _batch_daemon_call triggers fallback | Consistent with _try_daemon_call pattern; clean separation of concerns | Fallback always available |

## Deviations from Plan

| Type | Count | Impact |
|------|-------|--------|
| Scope additions | 1 | Minor improvement |
| Deferred | 0 | — |

**Auto-fixed:** 4 pre-existing ruff E501 errors in `server.py` (long description strings in Tool definitions). Fixed by splitting string literals across lines. Not in plan scope but zero risk and improves lint hygiene.

**AC-7 target:** Plan said 820+ tests; actual is 818 (812 + 6 new). Target was an overestimate — only 6 new tests were specified in the plan scope. All pass, 0 failures.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- Phase 2 complete: daemon (02-01) + MCP proxy (02-02) + axon_batch (02-03) all done
- Full test suite at 818, 0 failures
- Phase 3: Watch & filtrage (file watcher + selective re-indexing)

**Concerns:**
- None

**Blockers:**
- None

---
*Phase: 02-daemon-central, Plan: 03*
*Completed: 2026-03-02*
