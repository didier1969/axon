# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

TBD — v0.8 planning not yet started.

## Completed Milestones

<details>
<summary>v0.7 Quality & Security — 2026-03-04 (2 phases, 5 plans)</summary>

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Sécurité & Robustesse | 1/1 | ✅ Complete | 2026-03-02 |
| 2 | Qualité, Parsers & Features | 4/4 | ✅ Complete | 2026-03-04 |

Phase 1: 11 security + robustness fixes (path traversal, Cypher injection, race conditions, snippet quality, bounded queues). Audit score 61→~75/100.

Phase 2: axon_read_symbol O(1) MCP tool, sql/yaml byte offsets, parser quality test coverage, walker 512KB OOM guard, compute_repo_slug() helper, readline() socket fix, BATCH WARNING, AXON_LRU_SIZE. Audit score ~75→~81/100. 884 tests passing.

</details>

<details>
<summary>v0.6 Daemon & Centralisation — 2026-03-02 (3 phases, 7 plans)</summary>

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Centralisation du stockage | 1/1 | ✅ Complete | 2026-03-02 |
| 2 | Daemon central | 3/3 | ✅ Complete | 2026-03-02 |
| 3 | Watch & filtrage | 3/3 | ✅ Complete | 2026-03-02 |

</details>

<details>
<summary>v0.5 Hardening — 2026-02-28 (2 phases, 4 plans)</summary>

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Test Quality & Bug Fixes | 2/2 | ✅ Complete | 2026-02-28 |
| 2 | Parser & Performance | 2/2 | ✅ Complete | 2026-02-28 |

</details>

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
*Last updated: 2026-03-04 — v0.7 complete (Quality & Security: 2 phases, 5 plans, 884 tests)*
