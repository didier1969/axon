# Axon - Visualisation de Flux (Mermaid)

## Goal
Exporter les chemins d'exposition critiques (Taint Analysis) vers des diagrammes Mermaid afin d'offrir une visualisation claire des failles de sécurité.

## Phases

### Phase 1: TDD - Mermaid Generator (Rouge) (COMPLETED)
- [x] Écrire un test dans `src/axon-core/src/graph.rs` (ou dans un nouveau module `mermaid.rs`) vérifiant la génération d'un graphe orienté Mermaid (ex: `A --> B`) à partir de chemins JSON ou de tuples relationnels.
- [x] Le test doit échouer.

### Phase 2: Implémentation du Générateur (Vert) (COMPLETED)
- [x] Implémenter une fonction `generate_mermaid_flow` qui prend les chemins JSON de `get_security_audit` et génère la syntaxe `graph TD` de Mermaid.
- [x] Faire passer le test.

### Phase 3: Intégration MCP (Refactor) (COMPLETED)
- [x] Modifier l'outil `axon_audit` pour inclure le diagramme Mermaid dans le rapport Markdown retourné à l'agent IA.
- [x] Vérifier la bonne structure du Markdown.

### Phase 4: Zéro Warning & Commit (COMPLETED)
- [x] Lancer `cargo test` et `cargo clippy`.
- [x] Mettre à jour `ROADMAP.md` et `progress.md`.