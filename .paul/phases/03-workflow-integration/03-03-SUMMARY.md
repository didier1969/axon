---
phase: 03-workflow-integration
plan: 03
subsystem: mcp
tags: [mcp, tools, api, search, disambiguation]

requires:
  - phase: 03-02
    provides: CI integration and dead-code --exit-code gate

provides:
  - Agent-optimized MCP tool descriptions with workflow chaining guidance
  - Language filter on axon_query for polyglot repos
  - File-path disambiguation on axon_context (file.py:symbol format)
  - SearchResult.language field populated by fts_search and vector_search

affects: 03-04-documentation

tech-stack:
  added: []
  patterns:
    - "file:symbol disambiguation pattern for unambiguous symbol lookup"
    - "post-search language filter applied to SearchResult.language"

key-files:
  created: []
  modified:
    - src/axon/mcp/server.py
    - src/axon/mcp/tools.py
    - src/axon/core/storage/base.py
    - src/axon/core/storage/kuzu_backend.py
    - tests/mcp/test_tools.py

key-decisions:
  - "Language filter is post-search (not inside hybrid_search) — filters SearchResult list by .language"
  - "SearchResult.language added to base.py and populated in fts_search + vector_search"
  - "Disambiguation uses limit=5 candidates, checks distinct file_path values"
  - "_resolve_symbol gets a limit param (default=1) for backward compat"

patterns-established:
  - "axon_context supports 'path/to/file.py:symbol_name' — rpartition on last ':'"
  - "Disambiguation returns early with a list when >1 distinct file_path found"

duration: ~20min
started: 2026-02-26T00:00:00Z
completed: 2026-02-26T00:00:00Z
---

# Phase 3 Plan 03: MCP Query API Refinement — Summary

**Rewrote all 7 MCP tool descriptions for agent ergonomics, added language filter to `axon_query`, and added file-path disambiguation to `axon_context`; 678/678 tests passing.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~20min |
| Completed | 2026-02-26 |
| Tasks | 3 completed |
| Files modified | 5 |
| Tests | 671 → 678 (+7) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Agent-optimized tool descriptions | Pass | All 7 tools rewritten; axon_context no longer claims "community membership" |
| AC-2: Language filter on axon_query | Pass | Post-filters by SearchResult.language; case-insensitive |
| AC-3: File-path disambiguation in axon_context | Pass | Returns disambiguation list with retry hint |
| AC-4: Precise file-path lookup in axon_context | Pass | `path/file.py:symbol` resolves via suffix match |
| AC-5: Tests pass | Pass | 678/678 passing, 7 new tests |

## Accomplishments

- Rewrote all 7 `TOOLS` descriptions in `server.py` with agent-actionable guidance: when to use, what it returns, which tool to chain to next
- `axon_query` now accepts optional `language` parameter and post-filters `SearchResult` list by language — agents on polyglot repos can narrow to one language
- `axon_context` parses `file/path.py:symbol_name` format for unambiguous lookup; when bare name matches multiple files, returns a numbered disambiguation list with retry instruction
- `_resolve_symbol` extended with `limit` param (default=1) so disambiguation can fetch up to 5 candidates without breaking existing callers

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/mcp/server.py` | Modified | Rewrote TOOLS descriptions; added language param to axon_query schema + dispatch |
| `src/axon/mcp/tools.py` | Modified | handle_query language filter; handle_context file:symbol + disambiguation; _parse_file_symbol helper; _resolve_symbol limit param |
| `src/axon/core/storage/base.py` | Modified | Added `language: str = ""` field to SearchResult |
| `src/axon/core/storage/kuzu_backend.py` | Modified | fts_search: include node.language in Cypher, populate SearchResult.language; vector_search: populate from node cache |
| `tests/mcp/test_tools.py` | Modified | 7 new tests: TestHandleQueryLanguageFilter (3) + TestHandleContextDisambiguation (4) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Language filter is post-search | Keeps hybrid_search simple; language already in SearchResult after storage changes | No hybrid_search API changes needed |
| Add SearchResult.language + kuzu changes | Without it the filter is a no-op on real data; plan boundary was set without knowing the gap | Minimal: one field + two SQL queries updated |
| Disambiguation uses suffix match on file_path | Handles relative vs absolute path differences | Agents can use short paths like `parsers/python.py:parse` |
| _resolve_symbol default limit=1 | Preserves backward compat for handle_impact which relies on limit=1 | No regressions |

## Deviations from Plan

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Scope additions | 1 | Essential — feature non-functional without it |
| Auto-fixed | 1 | Test mock needed exact_name_search configured |

**Total impact:** One necessary scope addition (SearchResult.language + kuzu_backend); one test fix.

### Scope Addition: SearchResult.language + kuzu_backend changes

- **Found during:** Task 2 (language filter implementation)
- **Issue:** `SearchResult` dataclass had no `language` field; filter on `r.language` would raise `AttributeError`
- **Fix:** Added `language: str = ""` to `SearchResult` in `base.py`; updated `fts_search` to include `node.language` in Cypher (col index 5) and `vector_search` to read from node cache
- **Files:** `src/axon/core/storage/base.py`, `src/axon/core/storage/kuzu_backend.py`
- **Verification:** 678/678 tests pass; language filter tests confirm correct filtering

### Auto-fixed: Test mock for exact_name_search

- **Found during:** Task 3 (test for disambiguation)
- **Issue:** `_resolve_symbol` checks `hasattr(storage, "exact_name_search")` first; MagicMock always has this attr and returns a truthy MagicMock, bypassing the `fts_search` mock
- **Fix:** Added `mock_storage.exact_name_search.return_value = []` in disambiguation tests so code falls through to `fts_search`

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| SearchResult lacks language field | Added field to base.py + populated in kuzu_backend (see deviations) |
| MagicMock exact_name_search always truthy | Configured return_value = [] in disambiguation tests |

## Next Phase Readiness

**Ready:**
- MCP tools have clear agent-oriented descriptions ready to reference in 03-04 documentation
- Language filter and file:symbol disambiguation are tested and working
- SearchResult.language now populated for all fts and vector search results

**Concerns:**
- `exact_name_search` and `fuzzy_search` in kuzu_backend still don't populate `language` (they're fallbacks; not called by hybrid_search in normal flow)
- Community membership is still not returned by `handle_context` (removed from description, deferred)

**Blockers:** None

---
*Phase: 03-workflow-integration, Plan: 03*
*Completed: 2026-02-26*
