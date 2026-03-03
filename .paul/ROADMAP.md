# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

**v0.8 Graph Intelligence & Search Quality**
Status: 🚧 In Progress
Phases: 0 of 2 complete

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Graph Intelligence | TBD | Not started | - |
| 2 | MCP Tools & DX | TBD | Not started | - |

### Phase 1: Graph Intelligence

Focus: Boucher les trous dans le graphe + recherche hybride inspirée de qmd (fondations)
Plans: TBD (defined during /paul:plan)

Items:
- TypeScript generics → USES_TYPE edges (`Array<T>`, `Promise<User>`, etc.)
- Python wildcard imports `from x import *` → IMPORTS edges
- Test coverage integration — marquer `tested: bool` par symbole via croisement avec fichiers test
- Dead code: patterns de test manquants (`spec/`, `__tests__/`, `_spec.rb`, `*_test.go`)
- Private/public API surface — `exported: bool` sur les nœuds (Python `__all__`, TS `export`, Rust `pub`)
- Hybrid search: BM25 (SQLite FTS5 sur noms de symboles) + vecteurs + Reciprocal Rank Fusion
- Query expansion via LLM (2 variations sémantiques avant recherche)
- Symbol centrality scoring (PageRank sur le graphe de relations) → ranker par importance réelle
- Code-aware chunking pour les embeddings (couper aux frontières fonction/classe)
- `axon_find_similar` — trouver les symboles sémantiquement proches (détection de doublons)

### Phase 2: MCP Tools & DX

Focus: Nouveaux outils MCP pour agents AI + améliorations architecture et DX
Plans: TBD (defined during /paul:plan)

Items:
- `axon_find_usages` — tous les call-sites d'un symbole dans le repo (exhaustif)
- `axon_summarize` — résumé LLM-ready d'un fichier/module/classe
- `axon_lint` — règles structurelles : couplage élevé, god classes, cycles IMPORTS
- Community detection cohesion metric (remplace placeholder `cohesion: 0.0` de v0.5)
- `axon_diff` — comparer un symbole entre commits (`axon_diff Symbol --from HEAD~5`)
- `axon_refactor_hints` — suggère candidats à l'extraction, détecte high coupling/fan-in
- Multi-repo dependency edges — `DEPENDS_ON` entre repos via `pyproject.toml`/`package.json`/`go.mod`
- MCP tool descriptions — documenter les formats d'entrée attendus pour chaque outil
- `axon analyze --progress` — barre de progression pendant l'indexing
- Streaming batch responses — `axon_batch` retourne les résultats au fil de l'eau

## Planned Milestones

### v0.9: Language Coverage & Visualization

Focus: Parsers pour les grandes familles manquantes + export graph visuel via Memgraph
Phases: TBD

Items:
- Parser Java (tree-sitter-java) — enterprise/Android
- Parser C# (tree-sitter-c-sharp) — .NET/Unity
- Parser Ruby (tree-sitter-ruby) — Rails
- Graph visualization via Memgraph (affichage interactif, remplace idée Mermaid)
- Parser Kotlin (tree-sitter-kotlin) — JVM moderne
- Parser PHP (tree-sitter-php) — Laravel/WordPress/legacy
- Parser C++ (tree-sitter-cpp) — systèmes, jeux, infra

### v0.10: Architecture Avancée & Observabilité

Focus: Portabilité cross-machine, tracking de renommage, LSP, observabilité
Phases: TBD

Items:
- `axon_refactor_hints` amélioré (si reporté depuis v0.8)
- Symbol rename/move tracking — détecter via `git log --follow`, edge `RENAMED_FROM`
- LSP integration — exposer axon via Language Server Protocol (VS Code, Neovim)
- Watch mode real-time stats — dashboard ASCII live (events/s, queue depth, last indexed)
- Changelog generation — diff de graphes entre tags git
- Content-addressable storage (hash-based dedup, qmd-inspired)
- Virtual URI scheme `axon://repo-slug/file.py:42` (portabilité cross-machine)

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
