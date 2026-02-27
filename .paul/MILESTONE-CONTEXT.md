# Milestone Context

**Generated:** 2026-02-27
**Status:** Ready for /paul:milestone

## Features to Build

- **Performance optimization**: Batch Cypher inserts, async embeddings, profile & fix the 3 slowest repos (machineflow 562s, flow_analyzer 515s, BookingSystem 474s)
- **Markdown parser upgrade**: Move from regex to tree-sitter, add frontmatter/table/task-list extraction, reach parity with other parsers
- **Code quality consolidation**: Fix bare `except Exception` in tools.py/resources.py, split kuzu_backend.py (928 LOC), bump version to 0.3.0 in pyproject.toml
- **Multi-repo intelligence**: Cross-repo queries via MCP, "which projects use this pattern?", multi-repo axon_query
- **New language parsers**: Add 4 languages to reach 10 total — Go, YAML/TOML, SQL, HTML/CSS (based on project ecosystem needs)

## Scope

**Suggested name:** v0.4 Consolidation & Scale
**Estimated phases:** 1 (single intensive phase)
**Focus:** Harden what exists, then extend — performance, quality, coverage, multi-repo

## Phase Mapping

| Phase | Focus | Features |
|-------|-------|----------|
| 1 | Consolidation & Scale | All 5 feature areas as sequential plans within a single phase |

Expected plan breakdown (within the single phase):
- Plan 01: Performance (batch inserts, async embeddings, profiling)
- Plan 02: Code quality (except handling, kuzu_backend split, version bump)
- Plan 03: Markdown parser upgrade (tree-sitter, frontmatter, tables)
- Plan 04: New language parsers (Go, YAML/TOML, SQL, HTML/CSS)
- Plan 05: Multi-repo intelligence (cross-repo MCP queries)

## Constraints

- Solo developer — keep each plan independently testable
- No breaking changes to existing MCP tool signatures
- Maintain 678+ test count (only add, never reduce)
- Each plan must pass full test suite before moving to next

## Additional Context

- 25 projects now indexed — real-world performance data available
- Indexing times range from 11s (small) to 562s (large) — even without embeddings
- kuzu_backend.py at 928 LOC is the largest module, prime refactor target
- Markdown parser at 108 LOC vs 500+ for other parsers — clear coverage gap
- pyproject.toml version stuck at 0.2.3, needs bump to 0.3.0 (then 0.4.0 at milestone end)

---

*This file is temporary. It will be deleted after /paul:milestone creates the milestone.*
