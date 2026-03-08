---
phase: 02-mcp-tools-dx
plan: 04
subsystem: ingestion, cli
tags: [cross-repo, depends-on, dependency-graph, progress, pipeline, manifest]

# Dependency graph
requires:
  - phase: 02-01-02-03
    provides: MCP tools (axon_find_usages, axon_lint, axon_summarize); RelType enum established
provides:
  - cross-repo DEPENDS_ON edges (pyproject.toml, package.json, go.mod)
  - axon analyze --progress flag (stderr phase output)
  - RelType.DEPENDS_ON in model.py
  - PhaseTimings.cross_repo + PipelineResult.cross_repo_deps fields
affects: v1.0 (Java/C# parsers will add DEPENDS_ON for Maven/NuGet), MCP axon_context (DEPENDS_ON visible in callee graph)

# Tech tracking
tech-stack:
  added: []
  patterns: [stdlib tomllib with tomli fallback, phase-timing pattern in pipeline, env-var + CLI flag dual activation]

key-files:
  created: [src/axon/core/ingestion/cross_repo.py, tests/core/test_cross_repo.py]
  modified: [src/axon/core/graph/model.py, src/axon/core/ingestion/pipeline.py, src/axon/cli/main.py, tests/core/test_graph_model.py]

key-decisions:
  - "Exact slug-name matching only: dep name must match ~/.axon/repos/ directory name exactly"
  - "Placeholder File nodes for dep repos: file_path='dep:{name}' — not full repo roots"
  - "go.mod: last path segment only (github.com/gin-gonic/gin → gin) — intentionally imprecise"
  - "Cross-repo phase omitted from reindex_files() — full re-index only"

patterns-established:
  - "PhaseTimings + PipelineResult new fields pattern: dataclass field + timing block in run_pipeline"
  - "Env-var dual activation: show_progress = flag or bool(os.getenv('AXON_ANALYZE_PROGRESS'))"

# Metrics
duration: 1 session
started: 2026-03-07T00:00:00Z
completed: 2026-03-07T00:00:00Z
---

# Phase 02 Plan 04: Multi-repo DEPENDS_ON edges + analyze --progress Summary

**Multi-repo DEPENDS_ON edges via manifest parsing (pyproject.toml/package.json/go.mod) + `axon analyze --progress` stderr output for CI/scripting contexts.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | 1 session |
| Completed | 2026-03-07 |
| Tasks | 4 completed |
| Files modified | 6 |
| Tests after | 936 passing |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: pyproject.toml deps → DEPENDS_ON edges | Pass | test_process_cross_repo_deps_registered_match covers this |
| AC-2: package.json deps → DEPENDS_ON edges | Pass | test_parse_package_json_extracts_deps verifies parsing |
| AC-3: Unregistered deps silently skipped | Pass | test_process_cross_repo_deps_no_match: returns 0, no edge |
| AC-4: --progress prints to stderr | Pass | `_show_progress = show_progress or bool(os.getenv("AXON_ANALYZE_PROGRESS"))` |
| AC-5: Progress callback called per phase | Pass | Cross-repo phase calls report() like all other phases |

## Accomplishments

- `cross_repo.py` created: three manifest parsers (`_parse_pyproject_toml`, `_parse_package_json`, `_parse_go_mod`) + `process_cross_repo_deps()` linking repos via DEPENDS_ON edges
- Cross-repo phase integrated into `run_pipeline()` (after coupling, before `storage_load`); PhaseTimings and PipelineResult updated with new fields
- `axon analyze --progress` flag + `AXON_ANALYZE_PROGRESS` env var prints `[phase] done` to stderr per completed phase — no new dependencies, no stdout interference

## Task Commits

All tasks committed in single atomic commit:

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| Tasks 1-4 (combined) | `2a78f69` | feat | multi-repo DEPENDS_ON edges + axon analyze --progress |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/ingestion/cross_repo.py` | Created | Manifest parsers + process_cross_repo_deps() |
| `src/axon/core/graph/model.py` | Modified | RelType.DEPENDS_ON = "depends_on" added |
| `src/axon/core/ingestion/pipeline.py` | Modified | Cross-repo phase + PhaseTimings.cross_repo + PipelineResult.cross_repo_deps |
| `src/axon/cli/main.py` | Modified | --progress flag + AXON_ANALYZE_PROGRESS env var |
| `tests/core/test_cross_repo.py` | Created | 7 tests for parsers + edge creation + no-match cases |
| `tests/core/test_graph_model.py` | Modified | DEPENDS_ON added to EXPECTED RelType list (regression guard) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Exact slug-name matching | No package registry lookups; simple and predictable | Only works when dep name == axon repo dir name (e.g. both named "requests") |
| Placeholder File nodes (file_path="dep:{name}") | Anchor for edge without full repo context | Queryable via axon_cypher; no symbol children |
| go.mod last path segment only | github.com/gin-gonic/gin → "gin"; imprecise but avoids full module path comparison | May miss matches for multi-segment Go module names |
| Cross-repo omitted from reindex_files() | DEPENDS_ON edges stable across incremental re-index | Must run full `axon analyze` to update cross-repo edges |

## Deviations from Plan

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Scope additions | 1 | Positive: regression test added |
| Count difference | 1 | Positive: 7 tests vs 6 planned |

**Total impact:** One unplanned file modified (test_graph_model.py); one extra test created. No scope creep.

### Scope Additions

**1. test_graph_model.py: DEPENDS_ON added to EXPECTED list**
- **Found during:** Task 1 (model.py modification)
- **Issue:** Adding DEPENDS_ON to RelType without updating the existing regression test in test_graph_model.py would cause a count assertion failure
- **Fix:** Added DEPENDS_ON to the EXPECTED set in test_graph_model.py
- **Files:** `tests/core/test_graph_model.py`
- **Verification:** Test passes as part of 936 test suite

**2. 7 tests created (plan said 6)**
- plan's "done" criteria counted 6 tests; implementation added a 7th (`test_parse_go_mod_extracts_deps`)

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| None | Plan executed cleanly |

## Next Phase Readiness

**Ready:**
- Phase 2 complete (4/4 plans with SUMMARYs)
- 936 tests passing, clean working tree
- All MCP tool DX improvements shipped: axon_find_usages, MCP descriptions, axon_lint, community cohesion, axon_summarize, DEPENDS_ON edges, --progress flag
- v0.8 milestone ready to close

**Concerns:**
- None

**Blockers:**
- None

---
*Phase: 02-mcp-tools-dx, Plan: 04*
*Completed: 2026-03-07*
