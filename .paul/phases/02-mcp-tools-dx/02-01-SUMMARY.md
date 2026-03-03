---
phase: 02-mcp-tools-dx
plan: 01
subsystem: mcp
tags: [axon_find_usages, mcp-tools, tool-descriptions, cypher, call-sites]

requires:
  - phase: 01-graph-intelligence
    provides: CALLS/IMPORTS edges in KuzuDB, handle_find_similar pattern

provides:
  - handle_find_usages in mcp/tools.py (exhaustive call-site + import-site listing)
  - axon_find_usages registered in MCP TOOLS list with dispatch
  - Improved descriptions for 5 existing tools (axon_context, axon_find_similar, axon_detect_changes, axon_dead_code, axon_cypher)

affects: 02-02, 02-03, 02-04 (tool registration pattern to follow)

tech-stack:
  added: []
  patterns:
    - "execute_raw with parameters={} for CALLS/IMPORTS edge queries"
    - "deduplicate importers by file_path before formatting"

key-files:
  modified:
    - src/axon/mcp/tools.py
    - src/axon/mcp/server.py
    - tests/mcp/test_tools.py

key-decisions:
  - "IMPORTS query returns per-file deduplication: one entry per importing file"
  - "limit parameter applied via Cypher LIMIT clause (not server-side cap)"
  - "axon_find_usages is read-only via execute_raw — not added to daemon protocol"

patterns-established:
  - "New tool pattern: handle_X in tools.py + Tool() in TOOLS + elif in _dispatch_tool"
  - "execute_raw side_effect in tests: branch on 'nid' vs 'fp' parameter key"

duration: ~10min
started: 2026-03-06T00:00:00Z
completed: 2026-03-06T00:10:00Z
---

# Phase 2 Plan 01: axon_find_usages + MCP Description Improvements Summary

**`handle_find_usages` added to MCP layer: exhaustive CALLS + IMPORTS call-site listing with 5 improved tool descriptions reducing AI agent hallucinations.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~10 min |
| Tasks | 3 completed |
| Files modified | 3 |
| Tests added | 4 |
| Total tests | 917 (was 913) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: axon_find_usages returns all CALLS call-sites | Pass | Returns name, file, line per caller |
| AC-2: axon_find_usages includes IMPORTS edges | Pass | Deduplicated by file_path |
| AC-3: handles not-found and no-usages gracefully | Pass | "not found" / "No usages found" messages |
| AC-4: tool descriptions include parameter format examples | Pass | All 5 tools improved |

## Accomplishments

- `handle_find_usages(storage, symbol, limit=50, repo=None)` implemented following `handle_find_similar` repo-loading pattern exactly
- `axon_find_usages` registered in TOOLS list with full inputSchema; dispatched via `_dispatch_tool`
- Improved descriptions: axon_context (disambiguation format), axon_find_similar (embedding prerequisite + "No embedding found" warning), axon_detect_changes (raw git diff format), axon_dead_code (grouping + is_exported filter hint), axon_cypher (example query)

## Task Commits

| Task | Commit | Description |
|------|--------|-------------|
| Tasks 1+2+3 | `40fcb2f` | feat: axon_find_usages + improved MCP descriptions + 4 tests |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/mcp/tools.py` | Modified | Added `handle_find_usages` (~80 lines) |
| `src/axon/mcp/server.py` | Modified | Registered tool + improved 5 descriptions |
| `tests/mcp/test_tools.py` | Modified | 4 new tests in `TestHandleFindUsages` class |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Deduplicate IMPORTS by file_path | One file importing N symbols = 1 entry, not N | Cleaner output; avoids noise for heavily-imported files |
| limit applied via Cypher LIMIT only | Consistent with existing pattern; no redundant server cap | Simpler code |
| Not added to daemon protocol | Pure execute_raw read query; daemon overhead not needed | Follows same approach as handle_detect_changes |

## Deviations from Plan

None — plan executed exactly as specified.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- Tool registration pattern (tools.py + server.py + dispatch) confirmed working
- `execute_raw` with `parameters={}` pattern proven for CALLS/IMPORTS queries
- 917 tests passing, clean git state

**Concerns:** None.

**Blockers:** None. Plans 02-02 through 02-04 can proceed.

---
*Phase: 02-mcp-tools-dx, Plan: 01*
*Completed: 2026-03-06*
