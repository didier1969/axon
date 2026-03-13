# Axon - Taint Analysis Engine (v1.0)

## Goal
Améliorer le moteur de sécurité pour passer d'une simple recherche de mots-clés à un véritable "Taint Analysis" (Source -> Sink) via des chemins Cypher (depth > 1), permettant également de détecter les backdoors sémantiques.

## Phases

### Phase 1: TDD - Taint Analysis Paths (Rouge) (COMPLETED)
- [x] Écrire un test dans `src/axon-core/src/graph.rs` (ou `mcp.rs`) simulant une chaîne d'appel (ex: `user_input` -> `run_task` -> `eval`) et s'assurer que `axon_audit` ou `get_security_score` le détecte.
- [x] Le test doit échouer car l'implémentation actuelle limite la détection à un appel direct (`-[:CALLS]->`).

### Phase 2: Implémentation Cypher (Vert) (COMPLETED)
- [x] Modifier la requête dans `get_security_score` (`src/axon-core/src/graph.rs`) pour utiliser des chemins de profondeur variable, par exemple `[:CALLS*1..4]`.
- [x] Faire passer le test.

### Phase 3: Enrichissement de la Réponse Audit (Refactor) (COMPLETED)
- [x] Modifier `axon_audit` dans `mcp.rs` pour qu'il retourne non seulement le score, mais aussi les **chemins critiques** détectés. On devra ajouter une méthode dans `graph.rs` comme `get_critical_paths` ou ajuster la réponse de `axon_audit`.

### Phase 4: Zéro Warning & Commit (COMPLETED)
- [x] Exécuter `cargo clippy -- -D warnings` et `cargo test`.
- [x] Mettre à jour `ROADMAP.md` et `progress.md`.