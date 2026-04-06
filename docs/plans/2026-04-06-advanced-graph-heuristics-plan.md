# Plan d'Implémentation : Heuristiques Avancées par Graphe (Advanced Graph Heuristics)

## 🎯 Objectif (Vision)
Transformer le moteur analytique d'Axon en un **Linter Architectural Global**. Passer de la simple détection de dette technique locale (God Objects, Code Mort) à l'identification formelle et mathématique de failles systémiques (Boucles infinies, Fuites de domaine, Risques de blocage NIF) via l'exploitation du Property Graph (DuckDB/SQLite).

## 🗺️ Phase 1 : Implémentation des Moteurs de Requêtes (Data Plane)
**Fichier cible :** `src/axon-core/src/graph_analytics.rs`

Nous devons étendre l'implémentation de `GraphStore` avec de nouvelles méthodes exécutant des requêtes SQL complexes (Common Table Expressions) sur les tables `CALLS`, `CONTAINS` et `Symbol`.

1.  **`get_circular_dependencies(&self, project: &str)`**
    *   *Mécanique :* Requête SQL récursive (CTE) sur la table `CALLS`.
    *   *Seuil de déclenchement :* Tout chemin cyclique détecté (A -> B -> C -> A).
    *   *Retour :* Liste des chaînes d'appels circulaires.

2.  **`get_unsafe_exposure(&self, project: &str)`**
    *   *Mécanique :* Pathfinding entre un symbole `is_public = true` et un symbole `is_unsafe = true` (ou `name = 'unwrap'`).
    *   *Seuil de déclenchement :* Existence d'un chemin d'appel non protégé.
    *   *Retour :* Graphes d'appels compromettants.

3.  **`get_domain_leakage(&self, project: &str, domain_path: &str, infra_path: &str)`**
    *   *Mécanique :* Jointure spatiale sur les chemins de fichiers. Une source dans `domain_path` ne doit avoir aucune arête `CALLS` vers une cible dans `infra_path`.
    *   *Seuil de déclenchement :* Violation de la Clean Architecture.

4.  **`get_nif_blocking_risks(&self, project: &str)`**
    *   *Mécanique :* Analyse des arêtes `CALLS_NIF` (Elixir -> Rust). Si le symbole Rust cible possède une profondeur d'appel (fan-out) supérieure à un seuil critique sans indication de traitement asynchrone, lever une alerte.
    *   *Seuil de déclenchement :* Risque d'effondrement du Scheduler Erlang/BEAM.

## 🧠 Phase 2 : Exposition via le Control Plane (MCP)
**Fichier cible :** `src/axon-core/src/mcp/tools_governance.rs`

Les résultats des heuristiques de la Phase 1 doivent être injectés dans la boucle de rétroaction du développeur et de l'IA.

1.  **Enrichissement de `axon_audit` :**
    *   Ajouter une section `### 🌪️ Anti-Patterns Architecturaux`.
    *   Exécuter les nouvelles fonctions d'analyse de graphe.
    *   Si des dépendances circulaires ou des fuites de domaine sont trouvées, le score global chute drastiquement et l'audit passe en statut `critical`.

2.  **Création de l'outil `axon_architectural_drift` :**
    *   Un outil dédié permettant à un LLM d'explorer spécifiquement les violations de frontières (ex: pourquoi le module A appelle le module B alors qu'il ne devrait pas).

## 🏛️ Phase 3 : Ancrage Intentionnel (SOLL)
**Outil :** `soll_manager`

Lier ces nouvelles capacités physiques aux intentions architecturales du projet.
1.  Créer une Décision (`DEC-AXO-020`) : "Validation par Graphe des Invariants Architecturaux".
2.  Lier cette décision aux Guidelines globales `GUI-PRO-011` (Évolutivité Humaine / Clean Architecture) et `GUI-PRO-008` (Résilience Mécanique) pour prouver que ces recommandations sont désormais physiquement enforcées.

## 🚀 Phase 4 : Déploiement et Application (CI/CD)
1.  Écriture des tests d'intégration dans `tests/maillon_tests.rs` en insérant du code source factice contenant une boucle infinie, puis vérification que l'audit Axon la détecte et échoue.
2.  Mise à jour du binaire en production (`cargo build --release`).
