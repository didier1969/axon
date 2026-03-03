---
phase: 02-qualite-parsers-features
plan: 04
subsystem: mcp, daemon
tags: [readline, socket, batch, lru-cache, env-var]

requires:
  - phase: 02-qualite-parsers-features-01-03
    provides: stable MCP server and daemon infrastructure

provides:
  - makefile readline for newline-framed socket protocol
  - axon_batch partial failure summary [BATCH WARNING]
  - AXON_LRU_SIZE env var for daemon cache tuning

affects: []

tech-stack:
  added: []
  patterns:
    - "sock.makefile('rb').readline() for newline-framed JSON protocol"
    - "[BATCH WARNING: K/N failed: indices [...]] for partial batch failure reporting"
    - "int(os.environ.get('VAR', 'default')) pattern for env-var-backed CLI defaults"

key-files:
  modified:
    - src/axon/mcp/server.py
    - src/axon/daemon/__main__.py
    - tests/mcp/test_server.py
    - tests/daemon/test_server.py

key-decisions:
  - "readline() over recv(4096): idiomatic for newline-framed protocols; eliminates buffer fragmentation edge case"
  - "AXON_LRU_SIZE as env var default, CLI --max-dbs still overrides: user-tunable without rebuild"
  - "BATCH WARNING as appended footer, not raised exception: non-fatal, lets caller decide"

patterns-established:
  - "sock.makefile('rb').readline() is the correct read pattern for axon daemon socket calls"

duration: ~1h
started: 2026-03-03T13:00:00Z
completed: 2026-03-04T14:18:00Z
---

# Phase 02 Plan 04: readline, BATCH WARNING, AXON_LRU_SIZE — Summary

**Socket reads corrected to makefile readline, batch failures now surface a WARNING footer, daemon LRU size tunable via AXON_LRU_SIZE env var.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~1h |
| Started | 2026-03-03 |
| Completed | 2026-03-04 |
| Tasks | 4 completed |
| Files modified | 4 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: readline() replaces recv loop | Pass | `makefile("rb").readline()` in both `_try_daemon_call` and `_batch_daemon_call` |
| AC-2: axon_batch partial failure summary (daemon path) | Pass | `resp.get("error")` → `failed_indices` → `[BATCH WARNING: K/N failed: indices [...]]` |
| AC-3: axon_batch partial failure summary (direct path) | Pass | `startswith("Error: ")` or `startswith("Unknown tool:")` detection in `call_tool` fallback |
| AC-4: AXON_LRU_SIZE env var | Pass | `int(os.environ.get("AXON_LRU_SIZE", "5"))` as argparse default; CLI `--max-dbs` overrides |

## Accomplishments

- Replaced fragile `recv(4096)` accumulation loops with `sock.makefile("rb").readline()` — correct for newline-framed JSON protocol
- Added `[BATCH WARNING: K/N failed: indices [...]]` footer in `_batch_daemon_call` (daemon path) and `call_tool` direct fallback — partial failures now observable by callers
- `AXON_LRU_SIZE` env var controls `--max-dbs` default in daemon; CLI flag still wins when specified
- 9 new tests covering all three behaviours; 5 existing recv-mock tests updated to makefile-mock pattern

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| Tasks 1–4 (all tasks) | `2b961ac` | feat | readline, BATCH WARNING, AXON_LRU_SIZE (+9 tests) |
| Handoff | `ba329d5` | chore | pause handoff — APPLY done, UNIFY pending |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/mcp/server.py` | Modified | readline in `_try_daemon_call` + `_batch_daemon_call`; BATCH WARNING in both paths |
| `src/axon/daemon/__main__.py` | Modified | AXON_LRU_SIZE env var as `--max-dbs` default |
| `tests/mcp/test_server.py` | Modified | 5 recv→makefile mock updates + TestBatchSocketReadline + TestBatchPartialFailure (7 new tests) |
| `tests/daemon/test_server.py` | Modified | TestAxonLruSizeEnvVar (3 new tests) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| `sock.makefile("rb").readline()` per call (not shared file) in `_batch_daemon_call` | Shared `f = sock.makefile("rb")` works; per-call is safer re: buffering state between requests | Each call gets fresh file view; no read-ahead issues |
| BATCH WARNING appended as footer, not raised | Partial failure is non-fatal; callers may tolerate it; warning gives visibility without forcing error handling | Callers can inspect the string to detect partial failures |
| Error detection: `startswith("Error: ")` / `startswith("Unknown tool:")` | These are the two error prefix conventions already in direct path | Consistent with existing direct-path error handling |

## Deviations from Plan

None — plan executed exactly as specified.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- Phase 02 all 4 plans complete — v0.7 Quality & Security milestone complete
- 884 tests passing; no regressions

**Concerns:**
- None

**Blockers:**
- None

---
*Phase: 02-qualite-parsers-features, Plan: 04*
*Completed: 2026-03-04*
