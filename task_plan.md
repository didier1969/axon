# Axon - Clustering Auto-Adaptatif

## Goal
Remplacer les seuils d'audit de sécurité et de santé fixes par une analyse dynamique basée sur la densité locale du graphe. Actuellement, une fonction est considérée critique ou dangereuse via des règles fixes. L'objectif est d'utiliser des métriques de centralité (comme un sous-graphe ou in-degree massif) pour ajuster les scores en fonction du contexte architectural du projet.

## Phases

### Phase 1: TDD - Métriques de Densité (Rouge) (COMPLETED)
- [x] Écrire un test dans `src/axon-core/src/graph.rs` (ou `mcp.rs`) qui génère un graphe avec un noeud hautement connecté (hub) et un noeud isolé.
- [x] Le test doit s'attendre à ce qu'une nouvelle méthode `get_graph_density` ou que la pénalité de sécurité prenne en compte le degré du noeud.

### Phase 2: Implémentation Cypher (Vert) (COMPLETED)
- [x] Ajouter une requête Cypher calculant la centralité de degré (in-degree et out-degree) pour identifier les "God Objects" ou hubs (Clustering).
- [x] Mettre à jour `get_security_audit` ou ajouter `get_architecture_metrics` dans `graph.rs`.

### Phase 3: Intégration MCP (Refactor) (COMPLETED)
- [x] Exposer cette métrique de densité ou ces "God Objects" via `axon_health` ou `axon_audit`.
- [x] Refactoriser pour rendre le code propre.

### Phase 4: Zéro Warning & Commit (COMPLETED)
- [x] Lancer `cargo test` et `cargo clippy`.
- [x] Valider 100% de succès.