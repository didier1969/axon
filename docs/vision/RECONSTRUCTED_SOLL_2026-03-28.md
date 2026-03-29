# Axon Intelligence Framework - SOLL Structure (Technical Audit)
Date: 2026-03-28
Author: Nexus Lead Architect

## 1. VISION (Spécifications Macro)
**Titre :** Système de Vérité Structurelle (The Lattice)
**Description :** Infrastructure de cartographie AST multi-projets. Axon fournit une interface de requêtage sémantique sur l'intégralité du patrimoine logiciel local.
**Objectif :** Éliminer les erreurs de contexte des agents IA en substituant la lecture de fichiers bruts par des requêtes de graphe certifiées par le protocole Witness.

---

## 2. PILIERS TECHNIQUES (Contraintes Physiques)

### PIL-AXO-001 : Ingestion Ghost (Background Process)
*   **Contrainte :** Consommation CPU plafonnée à 5% hors phases de scan initial.
*   **Mécanique :** Synchronisation base/disque avec latence < 100ms.

### PIL-AXO-002 : Fédération de Graphe (Cross-Repo)
*   **Contrainte :** Unification des schémas entre dépôts Git distincts.
*   **Mécanique :** Jointures DuckDB inter-fichiers via relations HAS_SUBPROJECT.

### PIL-AXO-003 : Protocole Witness (Certification de Rendu)
*   **Contrainte :** Validation physique de l'état du DOM via LiveView.Witness.
*   **Mécanique :** Boucle de rétroaction L1/L2/L3 avec signature Witness.Token.

### PIL-AXO-004 : Résilience Zero-Sleep
*   **Contrainte :** Disponibilité du serveur MCP < 100ms.
*   **Mécanique :** Watchdog RSS (14GB limit) avec auto-recyclage de processus.

### PIL-AXO-005 : Architecture Multi-DB (SOLL/IST) (Isolation)
*   **Contrainte :** Séparation physique de la couche intentionnelle.
*   **Mécanique :** DuckDB Multi-DB : soll.db (Read-Only IA) vs ist.db (Write Forge).

---

## 3. EXIGENCES OPÉRATIONNELLES (Meso)

### REQ-AXO-001 : Allocation 1:1 Agent/Worker
*   **Description :** 1 thread Rust par cœur CPU physique piloté par un Agent Elixir.
*   **Justification :** Élimination de la congestion I/O sur les machines multicœurs.

### REQ-AXO-002 : Traçabilité MBSE
*   **Description :** Lien physique bidirectionnel entre Requirement et Symbol (Code).
*   **Justification :** Vérification de la complétude fonctionnelle via le graphe.

### REQ-AXO-003 : Gestion des Fichiers Titan
*   **Description :** Bypass des embeddings pour les fichiers > 512KB.
*   **Justification :** Prévention des erreurs OOM lors de l'inférence vectorielle ONNX.

### REQ-AXO-004 : Orchestration Asynchrone (Polling)
*   **Description :** Pattern de gestion des requêtes SQL/PGQ lourdes par ticket ID.
*   **Justification :** Empêcher les Timeouts TCP du serveur MCP.

---

## 4. CONCEPTS D'IMPLÉMENTATION (Micro)

### CPT-AXO-001 : CTE Récursives USING KEY
*   **How :** Parcours de graphe optimisé via DuckDB SQL natif (Hash Map itératif).
*   **Rationale :** Performance O(log n) sur les traversées de dépendances.

### CPT-AXO-002 : Registre Souverain (Registry)
*   **How :** Table soll.Registry centralisant l'incrémentation des IDs.
*   **Rationale :** Garantie d'unicité et formatage DNA (REQ-AXO-001) forcé par le serveur.

### CPT-AXO-003 : Nexus Seal (Oracle OOB)
*   **How :** Signature des diagnostics système hors du flux de données principal.
*   **Rationale :** Intégrité des rapports d'audit fournis aux agents tiers.
