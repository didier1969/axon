# Roadmap Axon v2 : Industrialisation

## Phase 1 : Consolidation du Noyau (Data Plane)
- [ ] **M1 :** Création du workspace Rust `axon-core`.
- [ ] **M2 :** Intégration de `tree-sitter` et portage des parseurs Python vers Rust (Python, Elixir, TS).
- [ ] **M3 :** Implémentation de l'ingestion `KuzuDB` directe.
- [ ] **M4 :** Serveur MCP natif fonctionnel.

## Phase 2 : Le Pont & Le Dashboard (Control Plane)
- [ ] **M5 :** Serveur UDS/MsgPack dans le Data Plane.
- [ ] **M6 :** Client Elixir pour la socket UDS.
- [ ] **M7 :** Premier Dashboard LiveView (Statut d'indexation).
- [ ] **M8 :** Visualisation de graphe intégrée.

## Phase 3 : Intelligence & Audit
- [ ] **M9 :** Migration du moteur d'Audit (OWASP) de Python vers Rust/KuzuDB.
- [ ] **M10 :** Support Multi-Repo natif dans KuzuDB.
- [ ] **M11 :** Optimisation des embeddings (FastEmbed Rust).

## Phase 4 : Déploiement & Validation
- [ ] **M12 :** Packaging en binaire unique (Release build).
- [ ] **M13 :** Tests E2E de performance (Objectif : 1M de lignes/minute).
- [ ] **M14 :** Documentation finale et tutoriels.
