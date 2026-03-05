# Cahier des Charges : Intégration HydraDB pour Axon

**Version :** 1.1.0
**Date :** 2026-03-05
**Architecture cible :** Nix-Native (Mix/OTP/ErlPort)
**Statut :** Stratégique / Validé

---

## 1. Vision Architecturelle : "The Hermetic Native Stack"

Axon devient une application OTP supervisée par HydraDB. L'environnement est géré hermétiquement par **Nix**.

## 2. Répartition des Responsabilités (Solve vs Wait)

| Couche | Responsabilité Axon (Python) | Responsabilité HydraDB (Elixir/Rust) |
|--------|------------------------------|--------------------------------------|
| **Parsing AST** | **Prend en charge** (Tree-sitter) | Ne fait rien |
| **Expansion Macros** | Fournit les hooks (`cargo expand`) | **Prend en charge** l'exécution (Mix/OTP) |
| **Résolution Alias** | **Prend en charge** (Scope local) | Fournit le Datalog pour la résolution **Globale** |
| **Persistence** | Calcule les hashes | **Prend en charge** (RocksDB + Dolt) |
| **Versionnage** | Fournit les métadonnées | **Prend en charge** (Dolt Branch/Merge/Diff) |

## 3. Garanties & Validation (The Compliance Suite)

Pour assurer la qualité de l'interface, les deux équipes s'engagent sur une suite de tests de conformité.

### A. Garanties Axon (Le "Provider")
Axon fournit une suite de tests unitaires (via `pytest`) garantissant :
*   **Fidélité AST** : L'extraction des symboles correspond exactement à la structure Tree-sitter.
*   **Conformité Arrow** : Les flux de données poussés vers HydraDB respectent le schéma de colonnes convenu.
*   **Délai de Parsing** : Le parsing moyen d'un fichier source ne doit pas dépasser **10ms** (sur CPU moderne Nix-isolé).

### B. Validation HydraDB (Le "Consumer")
HydraDB doit fournir et valider une suite de tests d'intégration garantissant :
1.  **Test d'Ingestion** : Capacité à ingérer 100 000 symboles en moins de **5 secondes** via ErlPort sans bloquer le scheduler BEAM.
2.  **Test de Résolution Datalog** : Une requête récursive de résolution d'alias complexe doit répondre en moins de **50ms** pour une base de 1 million de symboles.
3.  **Test d'Atomaticité Dolt** : Un commit structurel via Dolt doit être atomique (tout ou rien) et résister à un crash brutal de la VM Erlang.
4.  **Test de Supervision** : Un crash simulé du worker Python doit être détecté et géré (restart) par Elixir en moins de **100ms**.

## 4. Le Contrat d'Interface (Le "Pont")

*   **Ingestion** : Batching par paquets de 1000 symboles via ErlPort.
*   **Requêtes** : Localhost TCP MsgPack ou Distributed Erlang.

## 5. Cas d'Usage Résolus

*   **Zéro Docker Desktop** : Fluidité via Nix.
*   **Précision Elixir** : Résolution d'alias via Datalog récursif ultra-performant.
*   **Traçage Polyglotte** : Traversée NIF native.

---
*Le non-respect de la Compliance Suite (Section 3) invalide l'intégration.*
