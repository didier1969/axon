# Axon: Copilote Architectural

## 🧠 Vision & Usage
Axon est un moteur d'intelligence structurelle. Utilisez Axon comme une **Boussole** (compréhension globale) et Grep comme un **Scalpel** (recherche de texte brut).

### 🛠️ Axon Tool Routing (MANDATORY)
| Tâche | Outil |
|-------|-------|
| Trouver un symbole (fonction, classe, module) | `axon_query` |
| Analyser un fichier ou un module (Résumé) | `axon_summarize` |
| Comprendre les dépendances (Callers/Callees) | `axon_context` |
| Analyser l'impact d'un changement | `axon_impact` |
| Tracer un flux entre deux fonctions | `axon_path` |
| Détecter des anti-patterns (Cycles, God classes) | `axon_lint` |
| **Audit Architectural (Immune System)** | `axon audit` |

**IMPORTANT:** Tous les outils Axon EXIGENT le paramètre `repo` ou `project_code` selon la surface appelée. Le code projet canonique est la source de vérité.
- Utilisez `axon_list_repos` une fois par session pour trouver les noms.
- Exemple: `axon_query(repo="axon", query="...")`

## 🛠️ Build & Test Commands
- Install: `uv sync --all-extras`
- Test: `uv run pytest`
- Lint: `ruff check .`
- Daemon: `axon daemon start`, `axon daemon status`, `axon daemon stop`

## 🏗️ Architecture
- **Core:** `src/axon/core` (Graph, Ingestion, Storage)
- **Database:** KuzuDB (Graphe + Vecteur)
- **Interface:** MCP Server (`src/axon/mcp/server.py`) et CLI.
- **Daemon:** `src/axon/daemon/` (Cache LRU pour les backends KuzuDB)
