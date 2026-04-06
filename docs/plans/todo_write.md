# TodoWrite: Omniscience Federation Design

**Context:** Remplacer la découverte de projet "Zero-Config" par un enregistrement explicite (via MCP) stocké dans la base unifiée, et implémenter un polling asynchrone pour que le Démon lance les Watchers dynamiquement sur les nouveaux projets.

- [x] **Task 1: Mise à jour du Schéma SQL (ProjectCodeRegistry)**
  - Modify `init_schema` and `ensure_additive_soll_schema` in `src/axon-core/src/graph_bootstrap.rs` to add `project_path VARCHAR`.
  - Remove old logic from `src/axon-core/src/project_meta.rs` (to be done across tasks, but starting here).
  - Verify with `cargo check` and `cargo test`.
  - Commit.

- [x] **Task 2: Refonte de l'Outil MCP (`axon_init_project`)**
  - Modify `src/axon-core/src/mcp/catalog.rs` to require `project_path`.
  - Modify `axon_init_project` in `src/axon-core/src/mcp/tools_soll.rs` to take `project_path` and insert it.
  - Update tests in `src/axon-core/src/mcp/tests.rs` to include `project_path`.
  - Run tests and commit.

- [ ] **Task 3: Le Polling Réactif de l'Orchestrateur**
  - Delete static discovery from `main.rs`.
  - Add `spawn_federation_orchestrator` loop in `main_background.rs` polling `soll.ProjectCodeRegistry` and spawning `spawn_hot_delta_watcher` / `spawn_initial_scan` dynamically.
  - Purge dead code (`discover_project_identities`, etc.) from `project_meta.rs`.
  - Run tests, zero warnings, commit.
