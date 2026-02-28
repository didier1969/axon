# SUMMARY — Plan 02-02: Community Detection Parallelization

**Phase:** 02-parser-and-performance
**Plan:** 02
**Status:** COMPLETE
**Completed:** 2026-02-28

---

## What Was Done

Refactored `process_communities()` to split the call graph into weakly connected
components (WCCs) and run the Leiden algorithm on each component independently
via `ThreadPoolExecutor`, instead of running a single Leiden pass on the full graph.

### Files Modified

| File | Change |
|------|--------|
| `src/axon/core/ingestion/community.py` | Added `_partition_component()` helper + ThreadPoolExecutor-based `process_communities()` |
| `tests/core/test_community.py` | Added `TestProcessCommunitiesMultiComponent` (2 tests) |

### Key Implementation Details

**New helper `_partition_component(ig_graph, member_vertex_ids)`:**
- Takes a subgraph defined by original vertex IDs
- Returns empty list if component < 3 nodes (no Leiden needed)
- Creates an igraph subgraph via `ig_graph.subgraph(member_vertex_ids)`
- Runs Leiden, maps sub-indices back to original indices

**Refactored `process_communities()`:**
- Finds WCCs: `ig_graph.as_undirected().components(mode="WEAK")`
- Dispatches each component ≥ 3 nodes to `ThreadPoolExecutor`
- Collects all member groups and builds community nodes as before
- `cohesion` property set to `0.0` (placeholder — per-component modularity deferred)

---

## Test Results

| Test run | Result |
|----------|--------|
| `test_community.py` (all) | 11 passed (9 existing + 2 new) |
| Full suite `tests/` | **774 passed, 0 failed** |

---

## Decisions Made

- `ThreadPoolExecutor()` with default max_workers — let stdlib pick `min(32, cpu_count+4)`
- `cohesion: 0.0` placeholder acceptable — per-component modularity would require
  `_partition_component` to return `(groups, modularity)` tuple (deferred)
- `export_to_igraph()` and `generate_label()` unchanged — stable interfaces

---

## Acceptance Criteria

- [x] AC-1: Single large component → communities detected as before
- [x] AC-2: Multi-component graph → each component processed independently
- [x] AC-3: Small components (< 3 nodes) skipped without error
- [x] AC-4: Empty graph returns 0 (unchanged)
- [x] AC-5: All existing tests pass (774 total)
