# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

**v0.5 Hardening**
Status: ✅ Complete
Phases: 2 of 2 complete

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Test Quality & Bug Fixes | 2/2 | ✅ Complete | 2026-02-28 |
| 2 | Parser & Performance | 2/2 | ✅ Complete | 2026-02-28 |

### Phase 1: Test Quality & Bug Fixes ✅

Focus: Fix test infrastructure issues and analytics pollution bugs
Plans: 2/2 complete

- ✅ events.jsonl isolated via autouse conftest fixture (plan 01-01)
- ✅ Async embeddings race fixed: future.result() inside patch block (plan 01-02)
- ✅ test_watcher.py: 102s → 28s via session-scoped pre-indexed template (plan 01-02)
- ✅ test_pipeline.py: 166s → 81s via session-scoped schema template (plan 01-02)
- ✅ Watcher aggressiveness hotfix: embeddings now on 60s interval not 30s (plan 01-02)

### Phase 2: Parser & Performance ✅

Focus: Close parser gaps and parallelize graph algorithms
Plans: 2/2 complete

- ✅ Elixir `use` → USES relationship (`RelType.USES`, 4 tests) — plan 02-01
- ✅ Community detection parallelized (WCC + ThreadPoolExecutor) — plan 02-02

## Next Milestone

TBD after v0.5 complete.

## Completed Milestones

<details>
<summary>v0.4 Consolidation & Scale — 2026-02-27 (1 phase, 4 plans)</summary>

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Consolidation & Scale | 4/4 | ✅ Complete | 2026-02-27 |

</details>

<details>
<summary>v0.3 Workflow Integration — 2026-02-27 (3 phases, 8 plans)</summary>

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Language Coverage | 1/1 | ✅ Complete | 2026-02-26 |
| 2 | Large Project Performance | 3/3 | ✅ Complete | 2026-02-26 |
| 3 | Workflow Integration | 4/4 | ✅ Complete | 2026-02-27 |

</details>

---
*Roadmap created: 2026-02-26*
*Last updated: 2026-02-28 — v0.5 Hardening milestone created*
