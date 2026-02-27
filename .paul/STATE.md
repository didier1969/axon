# Project State

## Project Reference

See: .paul/PROJECT.md (updated 2026-02-27 after v0.3 complete)

**Core value:** Developers and AI agents can instantly understand any codebase — files auto-indexed, agents query the DB to reduce token usage and improve quality.
**Current focus:** v0.4 Consolidation & Scale — Phase 1: Consolidation & Scale

## Current Position

Milestone: v0.4 Consolidation & Scale
Phase: 1 of 1 (Consolidation & Scale) — Executing
Plan: 01-02 complete, loop closed
Status: Ready for next PLAN (01-03)
Last activity: 2026-02-27 — Plan 01-02 UNIFY complete

Progress:
- Milestone: [███░░░░░░░] 33%
- Phase 1: [███░░░░░░░] 33% (2/6 plans complete)

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ✓        ✓     [Loop closed, ready for next PLAN]
```

## Accumulated Context

### Decisions
| Decision | Phase | Impact |
|----------|-------|--------|
| Markdown headings → NodeLabel.FUNCTION | v0.3 Phase 1 | Markdown searchable via existing graph queries |
| Elixir module → NodeLabel.CLASS | v0.3 Phase 1 | Consistent with OOP-centric graph model |
| Content hash (sha256) over mtime for incremental | v0.3 Phase 2 | Reliable across copies/moves |
| max_workers=None → ThreadPoolExecutor default | v0.3 Phase 2 | CPU-adaptive |
| storage_load is 98%+ of indexing time (not pipeline phases) | v0.4 Plan 01-01 | Future perf work must target KuzuDB bulk_load |
| Async embeddings via ThreadPoolExecutor (default: non-blocking) | v0.4 Plan 01-01 | Pipeline returns immediately, embeddings in background |
| KuzuDB has no specific exception types — raises plain RuntimeError | v0.4 Plan 01-02 | All except blocks use RuntimeError (not kuzu.RuntimeError) |
| kuzu_backend split: schema/search/bulk as internal modules, class stays public API | v0.4 Plan 01-02 | Shared constants stay in kuzu_backend.py, imported by submodules |

### Deferred Issues
| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| Watcher integration tests slow (~57s) | v0.3 Phase 1 | S | v0.4 quality plan |
| Community detection (19% cold-start) still sequential | v0.3 Phase 2 | M | v0.4 perf plan |

### Blockers/Concerns
None.

## Session Continuity

Last session: 2026-02-27
Stopped at: Plan 01-02 UNIFY complete
Next action: /paul:plan for 01-03 (Markdown parser upgrade)
Resume file: .paul/ROADMAP.md
Resume context:
- Plans 01-01 and 01-02 complete (2/6 in Phase 1)
- Codebase at v0.4.0, clean exception handling, modular storage
- 687 tests passing, no regressions
- Next plan: 01-03 Markdown parser upgrade (tree-sitter, frontmatter, tables)

---
*STATE.md — Updated after every significant action*
