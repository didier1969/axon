---
phase: 03-watch-filtrage
plan: 03
subsystem: database
tags: [tree-sitter, kuzu, byte-offsets, schema, parsers, python, typescript, elixir, rust, go, css, html, markdown]

# Dependency graph
requires:
  - phase: 03-watch-filtrage/03-02
    provides: asyncio.Queue producer/consumer watcher; SymbolInfo/GraphNode in use
  - phase: 02-daemon-central
    provides: KuzuDB storage backend with bulk_load/CSV paths

provides:
  - SymbolInfo.start_byte / .end_byte fields (default 0)
  - GraphNode.start_byte / .end_byte fields (default 0)
  - KuzuDB schema with start_byte INT64, end_byte INT64 columns
  - 8 tree-sitter parsers propagating node.start_byte/end_byte to SymbolInfo
  - parser_phase.py propagating byte offsets from SymbolInfo to GraphNode

affects:
  - future content retrieval tools (axon read-symbol, v0.7)
  - any phase that queries GraphNode or reads schema columns

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Byte offset caching: tree-sitter nodes expose start_byte/end_byte; stored verbatim in graph schema for O(1) content retrieval"
    - "Backward-compat row guard: _row_to_node uses len(row) > 6 to detect 12-col (old) vs 14-col (new) schema"

key-files:
  created: []
  modified:
    - src/axon/core/parsers/base.py
    - src/axon/core/graph/model.py
    - src/axon/core/storage/kuzu_backend.py
    - src/axon/core/ingestion/parser_phase.py
    - src/axon/core/parsers/python_lang.py
    - src/axon/core/parsers/typescript.py
    - src/axon/core/parsers/elixir_lang.py
    - src/axon/core/parsers/rust_lang.py
    - src/axon/core/parsers/go_lang.py
    - src/axon/core/parsers/css_lang.py
    - src/axon/core/parsers/html_lang.py
    - src/axon/core/parsers/markdown.py
    - tests/core/test_parser_python.py

key-decisions:
  - "Byte offsets stored as INT64 in KuzuDB; no migration script — users re-run axon analyze to get new schema"
  - "Old 12-col schemas remain readable via len(row) > 6 guard in _row_to_node (start_byte/end_byte default to 0)"
  - "markdown.py sections carry start_byte from the atx_heading node; end_byte from the next heading's start_byte"
  - "sql_lang.py and yaml_lang.py left at default 0 (regex-based, no tree-sitter node available)"

patterns-established:
  - "Parser byte offset pattern: node.start_byte, node.end_byte always available on tree-sitter Node; pass directly to SymbolInfo"

# Metrics
duration: 29min
completed: 2026-03-02
---

# Phase 3 Plan 03: Byte-Offset Caching (start_byte/end_byte in schema) Summary

**start_byte/end_byte added to SymbolInfo, GraphNode, and KuzuDB schema; all 8 tree-sitter parsers now emit exact source byte offsets enabling O(1) content retrieval**

## Performance

- **Duration:** 29 min
- **Started:** 2026-03-02T19:21:10Z
- **Completed:** 2026-03-02T19:50:13Z
- **Tasks:** 2/2
- **Files modified:** 13

## Accomplishments

- Data model updated: SymbolInfo and GraphNode carry start_byte/end_byte (default 0) without breaking existing constructors
- KuzuDB schema updated: two new INT64 columns; backward-compat guard in _row_to_node handles both 12-col and 14-col schemas
- 8 tree-sitter parsers (Python, TypeScript, Elixir, Rust, Go, CSS, HTML, Markdown) propagate node.start_byte/end_byte
- parser_phase.py threads byte offsets from SymbolInfo into GraphNode at ingestion time
- 1 new test: TestByteOffsets.test_function_byte_offsets — verifies content.encode()[start:end].decode() == func.content
- Full suite: 824 tests, 0 failures

## Task Commits

Each task was committed atomically:

1. **Task 1: Data models + schema + ingestion + Python parser + test** - `71c53dd` (feat)
2. **Task 2: 8 tree-sitter parsers propagate byte offsets** - `b439476` (feat)

**Plan metadata:** (docs commit — see final commit below)

## Files Created/Modified

- `src/axon/core/parsers/base.py` - SymbolInfo gains start_byte/end_byte after end_line; content/decorators now have defaults
- `src/axon/core/graph/model.py` - GraphNode gains start_byte/end_byte after end_line
- `src/axon/core/storage/kuzu_backend.py` - _NODE_PROPERTIES, _node_to_row, _INSERT_NODE_CYPHER, _insert_node, _row_to_node all updated
- `src/axon/core/ingestion/parser_phase.py` - GraphNode constructor passes start_byte/end_byte from symbol
- `src/axon/core/parsers/python_lang.py` - _extract_function and _extract_class emit node.start_byte/end_byte
- `src/axon/core/parsers/typescript.py` - 5 SymbolInfo call sites updated
- `src/axon/core/parsers/elixir_lang.py` - 4 SymbolInfo call sites updated
- `src/axon/core/parsers/rust_lang.py` - 7 SymbolInfo call sites updated
- `src/axon/core/parsers/go_lang.py` - 3 SymbolInfo call sites updated
- `src/axon/core/parsers/css_lang.py` - 2 SymbolInfo call sites updated
- `src/axon/core/parsers/html_lang.py` - 1 SymbolInfo call site updated
- `src/axon/core/parsers/markdown.py` - _collect_headings passes node.start_byte; _extract_sections uses them
- `tests/core/test_parser_python.py` - TestByteOffsets class added

## Decisions Made

- Old 12-column KuzuDB schemas remain readable — `len(row) > 6` guard in `_row_to_node` returns 0 for byte offsets when columns absent
- No schema migration script: users must re-run `axon analyze` to get start_byte/end_byte populated
- `sql_lang.py` and `yaml_lang.py` left unchanged (regex-based, no tree-sitter node)
- markdown sections use heading node's `start_byte` as section start; next heading's `start_byte - 1` as end (content assembled from lines)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed python_lang.py byte offset emission in Task 1 (not Task 2)**

- **Found during:** Task 1 (new test test_function_byte_offsets failed with 0/0)
- **Issue:** The test in Task 1 exercises the Python parser; python_lang.py was listed under Task 2 but Task 1's verification required it
- **Fix:** Updated python_lang.py `_extract_function` and `_extract_class` as part of Task 1 execution to make the test pass
- **Files modified:** src/axon/core/parsers/python_lang.py
- **Verification:** test_function_byte_offsets passes (45/45 tests in test_parser_python.py)
- **Committed in:** 71c53dd (Task 1 commit)

**2. [Rule 1 - Bug] Fixed 16 pre-existing ruff errors across parsers/graph/storage**

- **Found during:** Task 1 and Task 2 verification (ruff reported errors)
- **Issue:** Pre-existing E501 (line too long), I001 (import sort), F841 (unused variables), F401 (unused imports) scattered across multiple files
- **Fix:** Reformatted long lines, removed unused variable assignments, cleaned unused imports; used ruff --fix for auto-fixable cases
- **Files modified:** base.py, kuzu_backend.py, elixir_lang.py, rust_lang.py, markdown.py, graph.py, storage/base.py, kuzu_search.py
- **Verification:** `uv run ruff check src/axon/core/parsers/ src/axon/core/graph/ src/axon/core/storage/` → All checks passed
- **Committed in:** 71c53dd and b439476

---

**Total deviations:** 2 auto-fixed (1 ordering deviation, 1 pre-existing lint cleanup)
**Impact on plan:** No scope creep; both deviations necessary for test correctness and the plan's 0-errors lint requirement.

## Issues Encountered

- `git stash` was used accidentally during pre-existing lint investigation, causing a temporary revert of all changes. Recovered via `git stash pop` without data loss.

## User Setup Required

None - no external service configuration required. Users must re-run `axon analyze` to populate start_byte/end_byte in existing KuzuDB databases.

## Next Phase Readiness

- Phase 3 complete: filters + debounce (03-01), asyncio.Queue watcher (03-02), byte-offset caching (03-03)
- v0.6 milestone complete — ready for `/paul:complete-milestone` → v0.7
- Future: `axon read-symbol` tool can use start_byte/end_byte for O(1) content retrieval (v0.7)

---
*Phase: 03-watch-filtrage*
*Completed: 2026-03-02*

## Self-Check: PASSED
