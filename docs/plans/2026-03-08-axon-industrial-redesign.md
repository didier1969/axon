# Design Document : Axon Industrial Redesign (v1.2+)

**Date** : 2026-03-08
**Statut** : Approuvé
**Objectif** : Transformer Axon en un système d'intelligence de code 100% robuste, performant et automatisé.

## 1. Architecture Core : Le Scanner Rust (Pod A)
Remplacement des mécanismes de scan actuels (Elixir/Python) par un composant natif pour garantir une fiabilité absolue.

- **Moteur** : NIF Rust (via Rustler) utilisant la crate `ignore` (moteur de ripgrep).
- **Parallélisme** : Scan multi-threadé du système de fichiers.
- **Surveillance** : Intégration de la crate `notify` pour une surveillance "always-on" sans dépendances externes (inotify-tools).
- **Intégrité** : Calcul systématique du Hash SHA-256 pour chaque fichier afin de ne ré-indexer que les changements réels.

## 2. Unification du Pilotage (.axonignore)
Suppression de la dépendance aux `.gitignore` tiers pour le moteur d'intelligence. Utilisation d'un standard unique.

- **Hiérarchie de Cascade** :
    1. `/home/dstadel/projects/.axonignore` (Niveau Agence) : Filtre les projets entiers.
    2. `.../axon/.axonignore` (Niveau Moteur) : Filtre technique global.
    3. `.../PROJECT/.axonignore` (Niveau Projet) : Spécificités locales et négations (`!`).
- **Règle d'Or** : Les fichiers `.md` sont systématiquement inclus pour la compréhension stratégique.

## 3. Source de Vérité Unifiée (HydraDB)
Migration du statut de la flotte depuis les fichiers `status.json` vers la base de données centrale.

- **Table `axon_repos_metadata`** : Stocke le statut, le nombre de fichiers (total/synced), le pourcentage, et les datetimes (`last_scan_at`, `last_file_import_at`).
- **Performance** : Requête unique agrégée pour `axon fleet status` via HydraDB v1.0.0.

## 4. Intégration Système et Disponibilité
Transformation d'Axon en un service d'infrastructure.

- **Démarrage automatique** : Configuration comme service Windows/Systemd.
- **Commandes simplifiées** : `axon start`, `axon stop`, `axon restart`.
- **MCP Server** : Exposition native des outils Axon via le Model Context Protocol pour Gemini et Claude.
- **Documentation IA** : Mise à jour de `GEMINI.md` et `CLAUDE.md` avec le contrat MCP complet.

## 5. Performance et Sécurité
- **Ingestion Parallèle** : Capacité à indexer plusieurs dépôts simultanément.
- **Régulation de Charge** : Limitation automatique de l'usage CPU pour ne pas bloquer la machine hôte.
- **Protection OWASP** : Refus des fichiers binaires, limites de taille (10Mo), et protection contre les dénis de service (bombes de fichiers).

## 6. Stratégie de Validation
- **Tests Rust** : Validation de la logique de filtrage et sécurité mémoire.
- **Tests Elixir** : Validation de l'intégration NIF et du reporting HydraDB.
- **Tests E2E** : Validation de la chaîne complète du scan à la requête MCP.
