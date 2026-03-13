# Axon - Consolidation MCP v1.2 Plan

## Goal
Réduire la charge cognitive de l'IA et optimiser l'économie du contexte en passant à 8 outils haute performance.

## Phases

### Phase 1: Tests E2E (Signatures)
- [ ] Écrire/adapter les tests unitaires et E2E dans `src/axon-core/src/mcp.rs` pour valider les 8 nouvelles signatures d'outils.

### Phase 2: Tronc (Refactorisation du Serveur)
- [ ] Mettre à jour `tools/list` dans `mcp.rs` pour enregistrer exactement les 8 outils consolidés :
  1. `axon_query`
  2. `axon_inspect`
  3. `axon_audit`
  4. `axon_impact`
  5. `axon_health`
  6. `axon_diff`
  7. `axon_batch`
  8. `axon_cypher`

### Phase 3: Feuilles (Fusion de la logique)
- [ ] Implémenter les handlers pour `axon_diff`, `axon_batch`, et adapter `axon_cypher` (qui remplace l'ancienne implémentation brute de `axon_query`).
- [ ] Mettre à jour les implémentations existantes (`axon_query`, `axon_inspect`, etc.) pour correspondre aux spécifications de la ROADMAP.

### Phase 4: Purge & Qualité
- [ ] Supprimer les anciens outils (comme `axon_list_repos` s'il est intégré ailleurs ou retiré de la liste).
- [ ] Valider 100% PASS et Zéro Warning avec `cargo test` et `cargo clippy`.