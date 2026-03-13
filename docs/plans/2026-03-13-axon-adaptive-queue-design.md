# Axon - Adaptive Priority Queue Design (Lazy vs Eager)

## Context
Actuellement, Axon indexe de manière strictement "Eager" au démarrage, saturant le CPU pour traiter l'intégralité des fichiers. Le calcul des embeddings et du graphe AST est lourd, ce qui pénalise la réactivité initiale si l'agent IA a immédiatement besoin du contexte.

## Objectif
Mettre en place une stratégie progressive et auto-adaptative (Lazy + Background) tirant parti de la supervision OTP d'Elixir et d'Oban.

## Architecture & Approche

### 1. File d'attente à double priorité (Hot vs Cold)
La configuration d'Oban est scindée en deux priorités :
- **Priority 1 (Hot Path) :** Réagit instantanément aux événements de modification (`file_event`) ou aux accès ciblés. La tâche est traitée immédiatement.
- **Priority 2 (Cold Path) :** Tâches d'indexation de masse en arrière-plan. Elles ne s'exécutent que lorsque le Hot Path est vide.

### 2. Groupement par Proximité Architecturale (Dossiers Actifs)
Quand un fichier est modifié (ex: `src/api/auth.ex`), ce n'est pas uniquement le fichier qui est promu dans le Hot Path, mais également le reste du dossier. Ce clustering augmente la probabilité de résoudre correctement les dépendances adjacentes (relations Cypher).

### 3. Gestion du Démarrage (Boot Sequence)
1. Le démon scanne l'arbre de fichiers.
2. Tout est poussé dans la file d'attente "Cold Path" (priorité 2).
3. Le système est déclaré "prêt" presque instantanément (non-bloquant).
4. Les workers consomment le Cold Path silencieusement avec une allocation CPU maîtrisée.

## Modifications Prévues
1. **`src/watcher/config/config.exs` :** Configurer Oban pour supporter les files avec priorité (`[hot: 2, default: 1]`).
2. **`src/watcher/lib/axon/watcher/server.ex` :** 
   - `handle_info(:initial_scan)` : Injecter via Oban avec `priority: 2` (Background).
   - `handle_info({:file_event})` / `dispatch_batch` : Injecter les modifications ciblées avec `priority: 1` (Hot path).
3. **`ROADMAP.md` :** Marquer le point comme complété.