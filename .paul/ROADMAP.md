# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

**v0.7 Quality & Security**
Status: 🚧 In Progress
Phases: 1 of 2 complete

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Sécurité & Robustesse | 1/1 | ✅ Complete | 2026-03-02 |
| 2 | Qualité, Parsers & Features | TBD | Planning | - |

### Phase 1: Sécurité & Robustesse ✅

Focus: Fermer les 3 vulnérabilités critiques + 6 bugs majeurs identifiés par l'audit (score 61→~75/100)
Plans: 1/1 complete

Items:
- Path traversal via `repo=` dans `_load_repo_storage` (tools.py:76) → `_sanitize_repo_slug()`
- Injection Cypher dans `handle_detect_changes` (tools.py:562) → paramètres nommés KuzuDB
- Race condition `_get_storage()` (server.py:72) → `asyncio.Lock` double-checked
- Socket Unix sans chmod (daemon/server.py) → `0o600` owner-only
- `_WRITE_KEYWORDS` incomplet (tools.py:599) → ajouter RENAME, ALTER, IMPORT
- Snippets tronqués à 200 chars (kuzu_search.py) → `_make_snippet()` 400 chars, coupure newline
- Callers illimités dans `handle_context` (tools.py:374) → cap 20, "... and N more"
- `asyncio.Queue()` unbounded (watcher.py:167) → `maxsize=100`, drop-oldest
- `remove_nodes_by_file` retourne 0 (kuzu_backend.py:153) → COUNT avant DELETE
- `meta.json` écrit avant pipeline (cli/main.py) → écriture atomique après succès
- N+1 queries dans `handle_detect_changes` (tools.py:558) → 1 requête `IN $fps`

### Phase 2: Qualité, Parsers & Features

Focus: axon read-symbol, byte offsets sql/yaml, qualité parsers, architecture, polish MCP (score ~75→81/100)
Plans: TBD (defined during /paul:plan)

Items:
- `axon read-symbol` MCP tool — O(1) via start_byte/end_byte, retour source exacte
- Byte offsets `sql_lang.py` + `yaml_lang.py` — regex span() pour start_byte/end_byte
- Limite taille fichier 512KB dans walker.py (OOM sur fichiers >1MB)
- Dead code test detection — patterns manquants `spec/`, `__tests__/`, `_spec.rb`
- TypeScript generics — extraire tous les params `<T1, T2>` pour USES_TYPE
- Wildcard imports Python `from x import *` → edge IMPORTS vers module
- `compute_repo_slug()` dans `core/paths.py` (slug dupliqué 3× dans cli/main.py)
- `axon_batch` partial failure summary → `[BATCH WARNING: N/M failed: indices [...]]`
- `AXON_LRU_SIZE` env var pour LRU maxsize (hardcodé à 5)
- Tool descriptions MCP — formats d'entrée attendus (ex. `git diff HEAD` pour axon_detect_changes)
- Socket buffer → `makefile("rb")` + `readline()` (remplacement recv 4096 bytes)

## Next Milestone

TBD après v0.7.

## Completed Milestones

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
*Last updated: 2026-03-02 — v0.7 Phase 1 complete (Sécurité & Robustesse)*
