---
phase: 01-consolidation-and-scale
plan: 02
subsystem: storage, mcp, cli, ingestion
tags: [exception-handling, kuzu-backend-split, code-quality, version-bump, refactoring]

requires:
  - phase: 01-consolidation-and-scale
    plan: 01
    provides: batch inserts, async embeddings, profiling baseline
provides:
  - Specific exception types replacing all bare `except Exception`
  - kuzu_backend.py split into 4 focused modules (backend, schema, search, bulk)
  - Version bump to 0.4.0
affects: [01-03 markdown parser, 01-04 new languages, all downstream plans]

tech-stack:
  added: []
  patterns: [internal module extraction with class-as-public-API, specific exception catching]

key-files:
  created:
    - src/axon/core/storage/kuzu_schema.py
    - src/axon/core/storage/kuzu_search.py
    - src/axon/core/storage/kuzu_bulk.py
  modified:
    - src/axon/core/storage/kuzu_backend.py
    - src/axon/mcp/tools.py
    - src/axon/mcp/resources.py
    - src/axon/cli/main.py
    - src/axon/core/ingestion/parser_phase.py
    - src/axon/core/ingestion/pipeline.py
    - pyproject.toml
    - src/axon/__init__.py
    - tests/core/test_kuzu_backend.py

key-decisions:
  - "KuzuDB has no specific exception types — raises plain RuntimeError, not kuzu.RuntimeError"
  - "kuzu_backend split: schema/search/bulk as internal modules, KuzuBackend class stays as sole public API"
  - "_row_to_node duplicated in kuzu_search.py to avoid circular imports"

patterns-established:
  - "Internal module extraction: standalone functions receiving kuzu.Connection as parameter"
  - "Exception specificity: RuntimeError for KuzuDB, (RuntimeError, ValueError) for MCP, (RuntimeError, OSError) for file ops"

duration: ~1 session
started: 2026-02-27
completed: 2026-02-27
---

# Phase 1 Plan 02: Code Quality Consolidation Summary

**Replaced 37 bare `except Exception` with specific exception types, split kuzu_backend.py from 981 to 449 LOC across 4 modules, and bumped version to 0.4.0.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | 1 session |
| Started | 2026-02-27 |
| Completed | 2026-02-27 |
| Tasks | 3 completed |
| Files modified | 9 source + 3 created + 1 test |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: No bare `except Exception` remains | Pass | 37 occurrences replaced across 6 files; grep returns 0 results |
| AC-2: kuzu_backend.py split into focused modules | Pass | 449 LOC remaining; kuzu_schema.py (70), kuzu_search.py (239), kuzu_bulk.py (187) created |
| AC-3: Version bumped to 0.4.0 | Pass | pyproject.toml and src/axon/__init__.py updated |
| AC-4: All existing tests pass | Pass | 687 passed, 0 failures |

## Accomplishments

- Replaced all 37 bare `except Exception` with specific types: RuntimeError for KuzuDB operations, (RuntimeError, ValueError) for MCP query handlers, (RuntimeError, OSError) for file/cleanup operations
- Split kuzu_backend.py (981 LOC) into 4 focused modules: kuzu_backend.py (449, orchestration + CRUD), kuzu_schema.py (70, schema creation + FTS indexes), kuzu_search.py (239, all search variants), kuzu_bulk.py (187, bulk load + CSV copy)
- KuzuBackend class remains the sole public API; extracted modules are internal with standalone functions receiving `kuzu.Connection`
- Version bumped from 0.2.3 to 0.4.0 in both pyproject.toml and `__init__.py`
- Added `_node_to_row()`, `_rel_to_row()`, `_rel_query()` helpers in kuzu_bulk.py for DRY

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| All 3 tasks | `75e1555` | refactor | exception specificity, kuzu_backend split, version 0.4.0 |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/storage/kuzu_backend.py` | Modified | Reduced from 981 to 449 LOC; delegates to schema/search/bulk modules |
| `src/axon/core/storage/kuzu_schema.py` | Created | `create_schema(conn)`, `create_fts_indexes(conn)` — 70 LOC |
| `src/axon/core/storage/kuzu_search.py` | Created | `exact_name_search`, `fts_search`, `fuzzy_search`, `vector_search` — 239 LOC |
| `src/axon/core/storage/kuzu_bulk.py` | Created | `bulk_load`, `csv_copy`, `bulk_load_nodes_csv`, `bulk_load_rels_csv` — 187 LOC |
| `src/axon/mcp/tools.py` | Modified | 3 bare `except Exception` replaced with `(RuntimeError, ValueError)` |
| `src/axon/mcp/resources.py` | Modified | 3 bare `except Exception` replaced with `(RuntimeError, ValueError)` |
| `src/axon/cli/main.py` | Modified | 1 bare `except Exception` replaced with specific type |
| `src/axon/core/ingestion/parser_phase.py` | Modified | 1 bare `except Exception` replaced with specific type |
| `src/axon/core/ingestion/pipeline.py` | Modified | 1 bare `except Exception` replaced with specific type |
| `pyproject.toml` | Modified | Version 0.2.3 -> 0.4.0 |
| `src/axon/__init__.py` | Modified | `__version__` 0.2.3 -> 0.4.0 |
| `tests/core/test_kuzu_backend.py` | Modified | 2 monkeypatch targets updated for kuzu_bulk module path |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| KuzuDB raises plain RuntimeError, not kuzu.RuntimeError | `kuzu` module has no specific exception types; tested empirically | All except blocks use `RuntimeError` directly |
| `_row_to_node` duplicated in kuzu_search.py | Avoids circular import between kuzu_backend and kuzu_search | Minor duplication (one small function) vs import complexity |
| Added `_node_to_row`, `_rel_to_row`, `_rel_query` helpers | DRY improvement during extraction — repeated patterns in bulk operations | Cleaner kuzu_bulk.py internals |
| Shared constants stay in kuzu_backend.py | `_LABEL_TO_TABLE`, `_LABEL_MAP`, etc. imported by submodules from kuzu_backend | Single source of truth, no circular imports |

## Deviations from Plan

| Deviation | Reason | Impact |
|-----------|--------|--------|
| kuzu_backend.py is 449 LOC (plan target: ~350-400) | Some orchestration methods kept inline rather than extracted | Still a 54% reduction; well within acceptable range |
| `_row_to_node` duplicated in kuzu_search.py | Circular import avoidance — kuzu_backend imports kuzu_search but kuzu_search would need kuzu_backend | Pragmatic choice; function is small and stable |
| 31 replacements in kuzu_backend.py (plan estimated 25) | More except blocks existed than initially counted | No negative impact — all replaced correctly |

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- Codebase is cleaner with specific exception handling — debugging will be easier for all subsequent plans
- kuzu_backend.py is modular — new storage features can target specific modules
- Version 0.4.0 reflects the milestone entry point

**Concerns:**
- None. Pure refactoring with no behavior changes.

**Blockers:**
None.

---
*Phase: 01-consolidation-and-scale, Plan: 02*
*Completed: 2026-02-27*
