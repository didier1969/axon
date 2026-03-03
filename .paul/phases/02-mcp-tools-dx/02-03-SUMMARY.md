---
phase: 02-mcp-tools-dx
plan: 03
subsystem: mcp
tags: [axon_summarize, mcp-tools, graph-query, symbol-inventory]

requires:
  - phase: 02-mcp-tools-dx plan 02
    provides: axon_lint, community cohesion — established handle pattern

provides:
  - axon_summarize MCP tool (file path → symbol inventory, symbol name → dependency summary)
  - _summarize_file helper (File node → classes/functions/interfaces with quality flags)
  - _summarize_symbol helper (symbol → callers/callees counts, methods, tested/exported/centrality)

affects: 02-04 (multi-repo — can leverage summarize pattern for progress display)

tech-stack:
  added: []
  patterns:
    - "file-first resolution: ENDS WITH file path match, fallback to _resolve_symbol"
    - "side_effect list for multi-call execute_raw mocks"

key-files:
  created: []
  modified:
    - src/axon/mcp/tools.py
    - src/axon/mcp/server.py
    - tests/mcp/test_tools.py

key-decisions:
  - "File-first resolution order (file path before symbol name) — avoids ambiguity when short path matches symbol"
  - "10-item truncation per section in file summary — prevents token overflow for large files"
  - "20-method cap for CLASS symbol summary — handles god classes gracefully"

patterns-established:
  - "handle_X(storage, ...) in tools.py → Tool() in server.py TOOLS → elif in _dispatch_tool"
  - "execute_raw side_effect list for ordered multi-call mocking in tests"

duration: ~1h
started: 2026-03-07T00:00:00Z
completed: 2026-03-07T00:00:00Z
---

# Phase 02 Plan 03: axon_summarize Summary

**Structural file/symbol overview tool — agents get symbol inventory or dependency summary without reading entire files.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~1 session |
| Completed | 2026-03-07 |
| Tasks | 3 completed |
| Files modified | 3 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: File path → symbol inventory | Pass | Classes/Functions/Interfaces sections, 10-item cap |
| AC-2: Symbol name → class/function summary | Pass | Callers count, methods, tested/exported/centrality |
| AC-3: Not-found → descriptive message | Pass | "not found as a file path or symbol name" |
| AC-4: File with 0 symbols | Pass | Shows "0 symbols" with file header |

## Accomplishments

- `handle_summarize` in tools.py: file-first resolution (ENDS WITH match), falls back to `_resolve_symbol`; file summary groups children by label prefix; symbol summary fetches callers/callees counts and method children for CLASS nodes
- `axon_summarize` registered in server.py TOOLS list and `_dispatch_tool` (after axon_lint)
- 4 tests in `TestHandleSummarize`: file path, class symbol, not-found, empty file — all using MagicMock storage

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| Task 1+2+3 combined | `e388d68` | feat | axon_summarize (handle_summarize + server registration + 4 tests) |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/mcp/tools.py` | Modified | Added handle_summarize, _summarize_file, _summarize_symbol helpers |
| `src/axon/mcp/server.py` | Modified | Registered axon_summarize in TOOLS + _dispatch_tool |
| `tests/mcp/test_tools.py` | Modified | Added TestHandleSummarize (4 tests) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| File-first resolution order | File paths and symbol names can overlap; file check first avoids wrong result type | All callers get expected result type based on input |
| No daemon protocol extension | handle_summarize uses only execute_raw + get_node — same as other tools; no special routing needed | Simpler implementation, consistent with existing tools |
| 10-item truncation per section | Large files (100+ symbols) would overflow MCP context; agents need overview not exhaustive list | Stays within token budget; agents can drill with axon_context |

## Deviations from Plan

None — plan executed as specified. All tasks combined in one commit (atomic by feature rather than by task).

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- 928 tests passing, clean git state
- axon_summarize fully functional and tested
- 02-04 (multi-repo DEPENDS_ON + analyze --progress) can proceed

**Concerns:**
- None

**Blockers:**
- None

---
*Phase: 02-mcp-tools-dx, Plan: 03*
*Completed: 2026-03-07*
