# Axon - v2.1 Super-Weapons (Advanced MCP Tools)

## Goal
Enrichir le serveur MCP Axon avec 5 nouveaux outils "super-armes" qui permettent au LLM de comprendre, auditer, et simuler l'évolution d'une base de code de manière macroscopique et microscopique, et intégrer des vérifications croisées dans les outils existants (comme l'audit).

## Phases

### Phase 1: Tool Registry & Router
- [ ] Ajouter les définitions des 5 nouveaux outils dans `tools/list` (`src/axon-core/src/mcp.rs`):
  - `axon_semantic_clones`
  - `axon_architectural_drift`
  - `axon_bidi_trace`
  - `axon_api_break_check`
  - `axon_simulate_mutation`
- [ ] Ajouter le routing dans `handle_call_tool`.
- [ ] S'assurer que les définitions d'entrée (inputSchema) sont claires.

### Phase 2: Implementation of Tools (Part 1 - Analysis)
- [ ] Implémenter `axon_semantic_clones`: Rechercher des symboles similaires (placeholder Cypher pour l'instant si les embeddings SQL ne sont pas prêts, ou requête par tag).
- [ ] Implémenter `axon_architectural_drift`: Requête Cypher détectant un chemin direct (CALLS ou IMPORTS) entre des couches non autorisées (ex: UI -> DB).
- [ ] Implémenter `axon_bidi_trace`: Tracer les appelants vers le haut (Entry Points) et les appelés vers le bas à partir d'un symbole (ex: pour du débuggage d'exception).

### Phase 3: Implementation of Tools (Part 2 - Simulation & Contracts)
- [ ] Implémenter `axon_api_break_check`: Pour un diff donné (ou symbole), vérifier s'il est `EXPORTED`/`PUBLIC` et lister les composants qui l'appellent pour alerter d'un breaking change.
- [ ] Implémenter `axon_simulate_mutation`: Simuler l'impact d'un changement en calculant la taille du sous-graphe impacté.
- [ ] Mettre à jour `axon_audit` pour inclure le résultat de `axon_api_break_check` ou du moins la détection de dérive architecturale (`axon_architectural_drift`) dans son rapport macro.

### Phase 4: Validation & Zero Warnings
- [ ] Ajouter des tests unitaires pour chaque nouvel outil dans `mcp.rs`.
- [ ] Lancer `cargo check` et `cargo test`.
- [ ] Mettre à jour le fichier `progress.md`.
