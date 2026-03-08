---
phase: 02-mcp-tools-dx
plan: 02
subsystem: mcp
tags: [axon_lint, community-detection, igraph, modularity, code-quality]

requires:
  - phase: 02-mcp-tools-dx plan 01
    provides: axon_find_usages, improved MCP tool descriptions

provides:
  - axon_lint MCP tool (fan-out, god class, import cycle detection)
  - Real community cohesion values (intra-edge density + global modularity)

affects: 02-03 (axon_summarize may group by community), 02-04

tech-stack:
  added: []
  patterns: ["3-query lint scan via parameterized execute_raw", "igraph subgraph().ecount() for per-community density"]

key-files:
  created: []
  modified:
    - src/axon/mcp/tools.py
    - src/axon/mcp/server.py
    - src/axon/core/ingestion/community.py
    - tests/mcp/test_tools.py
    - tests/core/test_community.py

key-decisions:
  - "cohesion = intra_edges / total_edges (not modularity per se) — bounded [0,1], computed via igraph subgraph"
  - "global modularity also stored per community node under properties['modularity']"
  - "Import cycles limited to 2-cycles (a.file_path < b.file_path deduplication)"

patterns-established:
  - "Lint rules run via 3 independent execute_raw calls, results filtered in Python"
  - "CodeRelation rel_type filtering in Cypher (not separate edge labels)"

duration: 1 session
started: 2026-03-06T00:00:00Z
completed: 2026-03-06T20:00:00Z
---

# Phase 02 Plan 02: axon_lint + Community Cohesion Summary

**axon_lint MCP tool added (3 structural rules) and community.py cohesion placeholder replaced with real intra-edge density + global modularity.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | 1 session |
| Completed | 2026-03-06 |
| Tasks | 4 completed |
| Files modified | 5 |
| Tests before | 917 |
| Tests after | 924 (+7) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: axon_lint detects high-coupling (fan-out > 20) | Pass | 4 lint tests pass incl. high coupling |
| AC-2: axon_lint detects god classes (> 15 methods) | Pass | god class detection tested |
| AC-3: axon_lint detects 2-cycle import loops | Pass | import cycle detection tested |
| AC-4: axon_lint returns clean message when no issues | Pass | test_lint_clean passes |
| AC-5: Community cohesion is a real modularity value > 0.0 | Pass | test_community_cohesion_nonzero passes; bounds [0,1] verified |

## Accomplishments

- `handle_lint` added to `tools.py`: 3 parameterized Cypher queries (fan-out, god classes, 2-cycle imports), formatted markdown lint report, registered in `server.py` TOOLS list and `_dispatch_tool`
- `community.py` cohesion placeholder replaced: per-community intra-edge density (`intra_edges / total_edges`) via `igraph.subgraph().ecount()`; global modularity (`ig_graph.modularity(membership)`) also stored per node
- 7 new tests: 4 lint unit tests (`TestHandleLint`) + 3 community cohesion tests (`TestCommunityCohesion`)

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| Tasks 1–4 (lint + cohesion + tests) | `7ef1cd8` | feat | axon_lint + community cohesion fix, 7 new tests |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/mcp/tools.py` | Modified | Added `handle_lint` (+58 lines) |
| `src/axon/mcp/server.py` | Modified | Registered `axon_lint` in TOOLS + dispatch |
| `src/axon/core/ingestion/community.py` | Modified | Real cohesion + modularity computation (+26 lines) |
| `tests/mcp/test_tools.py` | Modified | 4 lint tests added |
| `tests/core/test_community.py` | Modified | 3 cohesion tests added |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| cohesion = intra_edges / total_edges | Bounded [0,1], no extra deps, intuitive | Future axon_summarize can rank communities by cohesion |
| Global modularity stored per community node | igraph.modularity() gives one value for full partition | Available for future ML features |
| 2-cycle import detection only | 3-cycles are O(n³), deferred to v1.0 | No regression; documented in SCOPE LIMITS |

## Deviations from Plan

None — plan executed exactly as specified. All 4 tasks followed plan descriptions precisely.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- `axon_lint` available in MCP for AI agents to detect structural anti-patterns
- Community cohesion values are meaningful (not 0.0) — `axon_summarize` grouping by community is now viable
- 924 tests passing, clean git state (`7ef1cd8`)

**Concerns:**
- Import cycle detection is 2-cycles only; 3+ cycle detection deferred to v1.0

**Blockers:**
- None

---
*Phase: 02-mcp-tools-dx, Plan: 02*
*Completed: 2026-03-06*
