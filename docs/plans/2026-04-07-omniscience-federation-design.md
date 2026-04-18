# Démon Central (Omniscience) : Fédération et Enregistrement Explicite Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remplacer la découverte de projet "Zero-Config" par un enregistrement explicite (via MCP) stocké dans la base unifiée, et implémenter un polling asynchrone pour que le Démon lance les Watchers dynamiquement sur les nouveaux projets.

**Architecture:** 
1. **MCP Control Plane:** L'outil `axon_init_project` prend un `project_path` absolu, dérive ou valide un code canonique (3 caractères majuscules), et insère `(project_code, project_name, project_path)` dans `soll.ProjectCodeRegistry`.
2. **Data Plane:** La table `soll.ProjectCodeRegistry` est modifiée pour inclure la colonne `project_path VARCHAR`.
3. **Background Orchestrator:** Une boucle asynchrone interroge `ProjectCodeRegistry` toutes les secondes. Elle compare avec un état en mémoire (`HashSet`). Pour tout nouveau projet, elle déclenche `spawn_hot_delta_watcher` et `spawn_initial_scan` sur le `project_path` absolu. L'ancien mécanisme de découverte récursive par système de fichiers est supprimé.

**Tech Stack:** Rust, DuckDB, Tokio.

---

### Task 1: Mise à jour du Schéma SQL (ProjectCodeRegistry)

**Files:**
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/project_meta.rs` (suppression de l'ancienne logique)
- Test: `cargo test --manifest-path src/axon-core/Cargo.toml`

**Step 1: Modifier `init_schema` et `ensure_additive_soll_schema`**
Ajouter la colonne `project_path` à la création de la table.
```rust
// Dans ensure_additive_soll_schema :
self.execute("ALTER TABLE soll.ProjectCodeRegistry ADD COLUMN IF NOT EXISTS project_path VARCHAR")?;
```

**Step 2: Run test to verify it passes (Zéro Warning)**
Run: `cargo check --manifest-path src/axon-core/Cargo.toml`
Expected: Zéro Warning.

**Step 3: Commit**
```bash
git add src/axon-core/src/graph_bootstrap.rs
git commit -m "feat(core): add project_path column to ProjectCodeRegistry"
```

---

### Task 2: Refonte de l'Outil MCP (`axon_init_project`)

**Files:**
- Modify: `src/axon-core/src/mcp/tools_soll.rs`
- Modify: `src/axon-core/src/mcp/catalog.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Mettre à jour le catalogue MCP**
Ajouter le paramètre `project_path` requis dans `catalog.rs`.

**Step 2: Modifier `axon_init_project`**
La fonction doit générer le slug (auto-génération type Approche 1) et l'insérer avec le `project_path`.
```rust
let project_path = args.get("project_path")?.as_str()?;
// Logique d'insertion avec le path...
```

**Step 3: Run test to verify it fails**
Run: `cargo test test_axon_init_project_returns_global_guidelines --manifest-path src/axon-core/Cargo.toml`
Expected: Échec car l'argument `project_path` manque dans le test existant.

**Step 4: Corriger les tests MCP**
Mettre à jour les payloads JSON dans `mcp/tests.rs` pour inclure `project_path: "/tmp/fake"`.

**Step 5: Run tests and Commit**
Run: `cargo test --manifest-path src/axon-core/Cargo.toml`
```bash
git add src/axon-core/src/mcp/tools_soll.rs src/axon-core/src/mcp/catalog.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat(mcp): explicit project registration with absolute path via axon_init_project"
```

---

### Task 3: Le Polling Réactif de l'Orchestrateur

**Files:**
- Modify: `src/axon-core/src/main.rs`
- Modify: `src/axon-core/src/main_background.rs`

**Step 1: Supprimer la découverte statique**
Dans `main.rs`, supprimer `axon_core::project_meta::discover_project_identities()` et la boucle de lancement des watchers statiques.

**Step 2: Créer la boucle asynchrone (Polling)**
Dans `main_background.rs`, créer `pub fn spawn_federation_orchestrator(store: Arc<GraphStore>, ...)`.
Cette boucle fait un `SELECT project_code, project_path FROM soll.ProjectCodeRegistry`, compare avec un `HashSet` local, et lance les `spawn_hot_delta_watcher` pour les nouveaux.

**Step 3: Brancher le Polling**
Dans `main.rs`, appeler `main_background::spawn_federation_orchestrator(...)` à la place de l'ancienne logique.

**Step 4: Supprimer le code mort**
Purger `discover_project_identities` et `candidate_directories` de `project_meta.rs` (Zéro Warning).

**Step 5: Run tests and Commit**
Run: `cargo check && cargo test`
```bash
git add src/axon-core/src/main.rs src/axon-core/src/main_background.rs src/axon-core/src/project_meta.rs
git commit -m "feat(core): reactive project orchestration via SOLL registry polling"
```
