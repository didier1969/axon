# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

**v0.6 Daemon & Centralisation**
Status: 🚧 In Progress
Phases: 2 of 3 complete

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Centralisation du stockage | 1/1 | ✅ Complete | 2026-03-02 |
| 2 | Daemon central | 3/3 | ✅ Complete | 2026-03-02 |
| 3 | Watch & filtrage | TBD | Not started | - |

### Phase 1: Centralisation du stockage — ✅ Complete

Focus: Migrer toutes les KuzuDB vers `~/.axon/repos/{name}/kuzu`. Migration automatique des DBs locales existantes. Plus de `.axon/` éparpillé dans chaque projet.
Plans: 1/1 complete — see `phases/01-centralisation-stockage/01-01-SUMMARY.md`

### Phase 2: Daemon central — ✅ Complete

Focus: `axon daemon start/stop/status`. Unix socket `~/.axon/daemon.sock`. LRU cache DBs (max 5). Modèle d'embeddings partagé. MCP devient proxy léger (~10 MB). axon_batch pour N appels sur 1 socket.
Plans: 3/3 complete — see `phases/02-daemon-central/`

### Phase 3: Watch & filtrage

Focus: Watcher séquentiel avec queue prioritaire. Debounce configurable. Filtrage `.paul/`, `_build/`, `target/` par défaut. Byte-offset caching pour récupération O(1) des symboles.
Plans: TBD (définis lors de /paul:plan)

## Next Milestone

Run /paul:discuss-milestone ou /paul:milestone pour définir.

## Completed Milestones

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
*Last updated: 2026-03-02 — Phase 2 (Daemon central) complete*
