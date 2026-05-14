# Axon Phase de Consolidation : Qualité Sémantique et Télémétrie

## 1. Sémantique de Sécurité Universelle (Taint Analysis)
La requête de Taint Analysis actuelle est codée en dur avec des fonctions Python (`eval`, `exec`, `system`, `pickle`).
Nous devons remplacer cela par un champ dynamique `is_unsafe` géré au niveau du parsing (déjà supporté par la structure `Symbol`) et étendre la requête de score pour cibler les langages globaux :
- **Elixir :** `:os.cmd`, `Code.eval_string`, `Ecto.Adapters.SQL.query!`
- **Rust :** Blocs `unsafe {}` (déjà extraits, mais mal liés), `std::process::Command`
- **Python :** `eval`, `exec`, `os.system`, `subprocess.Popen`
- **JS/TS :** `eval()`, `dangerouslySetInnerHTML`, `exec()`

## 2. Sémantique de Couverture de Tests (Test Coverage)
Au lieu d'attendre que chaque parseur (Tree-Sitter) essaie de deviner si un symbole est "testé", nous allons utiliser **la puissance du Graphe KuzuDB**.
Nous implémenterons une requête de post-traitement en Rust (exécutée périodiquement ou après un scan complet) :
```cypher
MATCH (test_file:File)-[:CONTAINS]->(test_func:Symbol)-[:CALLS]->(prod_func:Symbol)
WHERE test_file.path CONTAINS 'test' OR test_func.name STARTS WITH 'test_'
SET prod_func.tested = true
```
Cela activera la couverture de test *automatiquement pour tous les langages*, tant que le parseur sait lier les `CALLS` et identifier les fichiers de test.

## 3. Remontée Globale de l'État d'Indexation
L'outil LiveView et MCP a besoin du panorama complet. L'outil `axon_health` et l'envoi de statistiques LiveView devront remonter :
- `total_files` : Le total de fichiers capacitaires du projet (statut `pending` + `indexed` + `failed`).
- `indexed_files` : Ceux insérés dans le graphe (statut `indexed`).
- `failed_files` : Les erreurs WASM/Syntaxe (statut `failed`).

## Étapes de réalisation
1. **Rust (`src/axon-core/src/graph.rs`) :** 
   - Modifier `get_security_audit` pour inclure les sinks Elixir/JS/Rust.
   - Ajouter une méthode `update_test_coverage_flags` qui exécute la mutation de graphe Cypher.
2. **Rust (`src/axon-core/src/mcp.rs`) :** 
   - Modifier `axon_health` pour qu'il interroge SQLite et retourne `(Total, Indexed, Failed)` en plus du score.
3. **Elixir (`src/dashboard/lib/axon_nexus/axon/watcher/server.ex` / `stats_cache.ex`) :**
   - S'assurer que le LiveView affiche bien cette trinité (Total, Indexed, Failed) dans les cartes de projets.
