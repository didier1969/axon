# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

**v0.9: Language Coverage**
Status: 🔵 Planning
Phases: TBD

## Completed Milestones (Recent)

<details>
<summary>v0.8 Graph Intelligence & Search Quality — 2026-03-07 (2 phases, 7 plans)</summary>

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Graph Intelligence | 3/3 | ✅ Complete | 2026-03-05 |
| 2 | MCP Tools & DX | 4/4 | ✅ Complete | 2026-03-07 |

Phase 1: TypeScript USES_TYPE generics, test coverage `tested: bool`, PageRank centrality + hybrid search boost, code-aware embeddings, `axon_find_similar`, attribute surfacing (`exported`/`untested` tags), `AXON_QUERY_EXPAND`. 913 tests.

Phase 2: `axon_find_usages` (exhaustive call-sites), MCP tool descriptions, `axon_lint` structural rules, community cohesion real intra-edge density, `axon_summarize` LLM-ready summaries, multi-repo DEPENDS_ON edges (manifest parsing), `axon analyze --progress`. 936 tests. Deferred to v0.9: `axon_diff`, streaming batch.

</details>

## Planned Milestones

### v0.9: Language Coverage

Focus: Parsers pour les grandes familles manquantes
Phases: TBD

Items:
- Parser Java (tree-sitter-java) — enterprise/Android
- Parser C# (tree-sitter-c-sharp) — .NET/Unity
- Parser Ruby (tree-sitter-ruby) — Rails
- Parser Kotlin (tree-sitter-kotlin) — JVM moderne
- Parser PHP (tree-sitter-php) — Laravel/WordPress/legacy
- Parser C++ (tree-sitter-cpp) — systèmes, jeux, infra
- `axon_diff` — comparer un symbole entre commits (deferred from v0.8)
- Streaming batch responses — `axon_batch` retourne les résultats au fil de l'eau (deferred from v0.8)

### v0.10: Architecture Avancée & Observabilité

Focus: Portabilité cross-machine, visualisation graph, tracking de renommage, LSP, observabilité
Phases: TBD

Items:
- `axon_refactor_hints` — suggère candidats à l'extraction, détecte high coupling/fan-in
- Graph visualization via Memgraph (affichage interactif — architecture TBD)
- Symbol rename/move tracking — détecter via `git log --follow`, edge `RENAMED_FROM`
- LSP integration — exposer axon via Language Server Protocol (VS Code, Neovim)
- Watch mode real-time stats — dashboard ASCII live (events/s, queue depth, last indexed)
- Changelog generation — diff de graphes entre tags git
- Content-addressable storage (hash-based dedup, qmd-inspired)
- Virtual URI scheme `axon://repo-slug/file.py:42` (portabilité cross-machine)

## Completed Milestones

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
*Last updated: 2026-03-07 — v0.8 complete (2 phases, 7 plans, 936 tests); v0.9 Language Coverage next*
