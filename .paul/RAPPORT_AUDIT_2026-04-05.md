# Rapport d'Audit & Handoff (Reality-First Stabilization)

**Date :** 2026-04-05
**Branche :** main
**Snapshot de Référence :** 2026-04-01 (issu de `STATE.md`)

## 1. Contexte et Prise de Terrain
Le projet **Axon** est une infrastructure de "vérité structurelle" (Intelligence structurelle via graphe et indexation sémantique). 
- L'environnement officiel est garanti par `Nix / Devenv` (`devenv 2.0.4`).
- L'architecture est séparée en deux plans stricts :
  - **Plane A (Elixir)** : Visualisation, Cockpit read-only, Télémétrie.
  - **Plane B (Rust)** : Autorité de runtime, Ingestion, DataFusion/SQL, MCP.

## 2. Validation de l'Environnement de Vérité
Les commandes ont toutes été exécutées sous `devenv shell` pour garantir l'intégrité (reproductibilité Nix).

### Rust Core (Data Plane)
- Les tests unitaires et e2e s'exécutent avec succès. 
- **Résultat** : `210/210` tests lib passés, `47/47` tests bin passés. 
- **Preuve** : 100% de succès. Le socle Rust est stable.

### Elixir Dashboard (Visualization Plane)
- **Résultat** : `34/35` tests passés.
- **Erreur dominante identifiée** : 
  - `AxonDashboardWeb.StatusLiveTest` : Le test "ignores local backpressure telemetry because cockpit reads Rust runtime only" échoue sur une assertion `assert html =~ "HEALTHY"`. 
  - **Cause racine (isolée et prouvée)** : Fuite d'état globale dans ETS (`:axon_telemetry`). Le `host_state` hérite de la valeur `"watch"` (ou `"constrained"`) d'un test précédent, au lieu de reprendre la valeur par défaut (`"healthy"`), car `Axon.Watcher.Telemetry.reset!()` ne réinitialise pas toujours correctement le `runtime_snapshot` ou il y a un race-condition avec le processus asynchrone `BridgeClient`. Un test précédent (`ProgressTest`) a été réparé avec succès suite au passage à l'alias `f.graph_ready`.

## 3. Réconciliation Théorie / Réalité

| Composant | Statut | Remarques |
| :--- | :--- | :--- |
| **Environnement (Devenv)** | **Aligné** | Lockfile présente, outils standards (rustc 1.93, elixir 1.18). |
| **Rust Core (Runtime/Ingestion)** | **Aligné** | Budget dynamique et pipeline conformes à la doc `STATE.md`. 100% tests OK. |
| **Elixir Cockpit** | **Partiel** | Fuite d'état ETS mineure dans les tests (`StatusLiveTest`). |
| **Protocoles (MCP / SQL)** | **Aligné** | Interface de requêtage testée et documentée. |

## 4. Défauts Dominants et Priorisation
1. **[Priorité 1 - Stabilisation Tests]** Corriger l'isolation d'état ETS dans la suite de tests Elixir (`AxonDashboardWeb.StatusLiveTest`) pour garantir une reproductibilité à 100% sans faux-négatifs liés au shared-state.
2. **[Priorité 2 - Nettoyage]** Supprimer ou versionner les fichiers `SOLL_EXPORT_*` non-trackés détectés à la racine (`docs/vision/SOLL_EXPORT...`).

## 5. Handoff & Prochaines Étapes
L'infrastructure est dans un état très sain, fidèle aux exigences du manifeste `Nexus Lead Architect`. 
**Prochaine action logique :**
Appliquer le correctif de reset d'état ETS complet (y compris sur les processus enfants éventuels) pour le test Elixir restant, puis engager la Phase 3 de la roadmap (v1.0 Language Coverage) après sécurisation absolue de la CI.
