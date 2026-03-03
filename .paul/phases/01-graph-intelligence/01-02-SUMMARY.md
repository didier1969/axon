---
phase: 01-graph-intelligence
plan: 02
subsystem: ingestion
tags: [schema, pagerank, test_coverage, centrality, hybrid_search, igraph]

requires:
  - phase: 01-graph-intelligence/01-01
    provides: Parser completeness, USES_TYPE edges, Go/spec dead code patterns

provides:
  - GraphNode.tested (bool) — symbol called/imported from test file
  - GraphNode.centrality (float) — PageRank score on CALLS+IMPORTS graph
  - process_test_coverage ingestion phase
  - process_centrality ingestion phase
  - hybrid_search centrality boost

affects: 01-03 (Intelligence Layer), axon_query ranking, axon_dead_code coverage signal

tech-stack:
  added: []
  patterns:
    - "len(row) guard for backward-compatible schema evolution"
    - "isinstance(float) guard in hybrid boost to survive mock storages"

key-files:
  modified:
    - src/axon/core/graph/model.py
    - src/axon/core/storage/kuzu_backend.py
    - src/axon/core/storage/kuzu_search.py
    - src/axon/core/search/hybrid.py
    - src/axon/core/ingestion/pipeline.py
  created:
    - src/axon/core/ingestion/test_coverage.py
    - src/axon/core/ingestion/centrality.py
    - tests/core/test_test_coverage.py
    - tests/core/test_centrality.py

key-decisions:
  - "isinstance(centrality, float) guard in hybrid boost — MagicMock would otherwise corrupt RRF scores in existing tests"
  - "kuzu_search._row_to_node fixed: was using 12-col offsets against 14-col schema (start_byte was wrongly mapped to content)"
  - "test_coverage placed before dead_code, centrality placed after communities — both use igraph"

patterns-established:
  - "len(row) > N guards for backward-compat column additions (consistent with v0.6 byte-offset pattern)"

duration: ~45min
started: 2026-03-05T00:00:00Z
completed: 2026-03-05T00:00:00Z
---

# Phase 01 Plan 02: Node Enrichment Summary

**Added tested+centrality to GraphNode and KuzuDB schema; PageRank and test-coverage ingestion phases; centrality boost in hybrid search — 902 tests passing (+11).**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~45 min |
| Completed | 2026-03-05 |
| Tasks | 3 completed |
| Files modified | 5 |
| Files created | 4 |
| Tests before | 891 |
| Tests after | 902 (+11) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Schema extension | Pass | GraphNode.tested + centrality; KuzuDB 16-col with len(row) guards |
| AC-2: Test coverage marking | Pass | CALLS + IMPORTS from test files → tested=True; 6 tests pass |
| AC-3: PageRank centrality | Pass | igraph pagerank on CALLS+IMPORTS; C >= B >= 0 ordering verified |
| AC-4: Centrality in hybrid search | Pass | `score * (1 + centrality)` boost with isinstance(float) guard |

## Accomplishments

- `GraphNode` extended with `tested: bool = False` and `centrality: float = 0.0`
- KuzuDB schema DDL updated; `_node_to_row()`, `_INSERT_NODE_CYPHER`, `_insert_node()`, `_row_to_node()` all updated with `len(row) > 14/15` guards
- `kuzu_search._row_to_node` fixed: previously used 12-col offsets against 14-col schema (content was reading `start_byte`); now properly handles all three schema versions
- `process_test_coverage`: marks symbols via CALLS (direct) and IMPORTS (file-level) from test files
- `process_centrality`: igraph PageRank on CALLS+IMPORTS graph across Function/Method/Class/File/Interface nodes
- `hybrid_search`: centrality boost with `isinstance(centrality, float)` guard (safe against MagicMock in tests)
- Pipeline wired: centrality after communities, test_coverage before dead_code

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/graph/model.py` | Modified | tested + centrality fields on GraphNode |
| `src/axon/core/storage/kuzu_backend.py` | Modified | Schema DDL + row serialization + _row_to_node for 16-col |
| `src/axon/core/storage/kuzu_search.py` | Modified | Fixed _row_to_node for 14-col + added 16-col support |
| `src/axon/core/search/hybrid.py` | Modified | Centrality boost with isinstance guard |
| `src/axon/core/ingestion/pipeline.py` | Modified | Added two new phase calls + PhaseTimings fields |
| `src/axon/core/ingestion/test_coverage.py` | Created | process_test_coverage phase |
| `src/axon/core/ingestion/centrality.py` | Created | process_centrality phase (PageRank) |
| `tests/core/test_test_coverage.py` | Created | 6 tests for test coverage marking |
| `tests/core/test_centrality.py` | Created | 5 tests for PageRank centrality |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| isinstance(float) guard in hybrid boost | MagicMock.centrality is truthy and not a float — without guard, existing hybrid tests would fail | Safe boost for production, no-op for mock storage |
| Fixed kuzu_search._row_to_node | Pre-existing bug: 12-col offsets vs 14-col schema corrupted content in vector search results | Correctness fix, no test regressions |
| test_coverage before dead_code | test coverage data should be available to dead_code (future: tested+dead = refactor candidate) | Better signal ordering |

## Deviations from Plan

| Type | Count | Impact |
|------|-------|--------|
| Auto-fixed | 1 | kuzu_search._row_to_node 12-col bug fixed in scope |
| Scope additions | 0 | — |
| Deferred | 0 | — |

**Total impact:** One pre-existing bug fixed as a necessary precondition for correct column parsing.

## Next Phase Readiness

**Ready:**
- tested + centrality available on all GraphNode objects after pipeline run
- Foundation for Plan 01-03: add centrality/tested to axon_query output, find_similar, query expansion

**Concerns:**
- centrality boost in hybrid_search makes N=`limit*3` `get_node` calls (no cache); acceptable for now, optimize in v0.9 if needed

**Blockers:**
- None

---
*Phase: 01-graph-intelligence, Plan: 02*
*Completed: 2026-03-05*
