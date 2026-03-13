# Roadmap: axon

## 💎 v1.0 - The Intelligent Immune System (IN PROGRESS)
**Focus :** Intelligence de flux, Intégration HydraDB et Performance Native.

### 🛡️ Sécurité & Audit (The Taint Analysis Engine)
- [x] **Analyse de Propagation (Taint Analysis) :** Passer d'une recherche par mots-clés à un suivi réel de la donnée (`Source` -> `Sanitizer` -> `Sink`).
- [x] **Détection de Backdoors sémantiques :** Identifier les fonctions dont le nom cache la dangerosité réelle (ex: `run_task` qui appelle `eval`).
- [x] **Clustering Auto-Adaptatif :** Remplacer les seuils fixes par une analyse de la densité locale du graphe.
- [x] **Visualisation de Flux :** Exporter les chemins d'exposition critiques vers des diagrammes Mermaid/SVG.

### ⚡ Performance & Scalabilité (The Hydra Engine)
- [x] **Intégration HydraDB :** Déportation de la persistence vers RocksDB/Dolt (Elixir/Rust).
- [x] **Stratégie Lazy vs Eager :** Implémentation de la file d'attente de tâches de fond via OTP (supervision Elixir).
- [ ] **Embeddings Parallélisés :** Réduction radicale du temps d'indexation (Cible : < 10min pour 40k symboles).

### 🧠 Intelligence & UX
- [ ] **Audit Proactif :** Alerte automatique dès qu'un changement dégrade le score de sécurité.
- [ ] **Traçage Polyglotte :** Traversée automatique des frontières (ex: Elixir ↔ Rust NIFs).

---

## ✅ v1.0 - The Structural Copilot (COMPLETED 2026-03-07)
**Focus :** Amélioration de la précision de l'audit architectural et de la performance globale.

- [x] **Audit Clustering :** Réduction drastique du bruit dans les rapports d'audit (docs, tests).
- [x] **Centrality-based Ejection :** Isolation automatique des hubs critiques pour la sécurité.
- [x] **CLI DX :** Option `--verbose`, fix des boucles de récursion, commande `axon up`.
- [x] **Fallback Parsers :** Support universel des fichiers texte/inconnus.

---

## 🏗️ Historique des Milestones Complétées

<details>
<summary>v0.8 Graph Intelligence — 2026-03-07</summary>
Centralité PageRank, Hybrid Search, axon_path, axon_find_usages, axon_lint, axon_summarize.
958 tests passants.
</details>

<details>
<summary>v0.7 Quality & Security — 2026-03-04</summary>
Sécurisation Cypher, byte offsets précis, axon_read_symbol.
</details>

<details>
<summary>v0.6 Daemon & Centralisation — 2026-03-02</summary>
Daemon central avec cache LRU, stockage ~/.axon/repos/.
</details>

## 🏗️ Consolidation MCP v1.2 (COMPLETED)

**Objectif :** Réduire la charge cognitive de l'IA et optimiser l'économie du contexte en passant de 17 à 8 outils haute performance.

### 📦 Spécification des Outils Consolidés
1.  **`axon_query` :** Recherche hybride (texte + vecteur) et similarité sémantique.
2.  **`axon_inspect` :** Vue 360° d'un symbole (code source, appelants/appelés, statistiques).
3.  **`axon_audit` :** Vérification de conformité (Sécurité OWASP, Qualité, Anti-patterns).
4.  **`axon_impact` :** Analyse prédictive (Rayon d'impact et chemins critiques).
5.  **`axon_health` :** Rapport de santé global (Code mort, lacunes de tests, points d'entrée).
6.  **`axon_diff` :** Analyse sémantique des changements (Git Diff -> Symboles touchés).
7.  **`axon_batch` :** Orchestration d'appels multiples (Performance).
8.  **`axon_cypher` :** Interface de bas niveau pour requêtes HydraDB brutes.

### 🛠️ Protocole d'Exécution
- [x] **Phase 1 (Tests) :** Écrire les tests E2E pour les 8 nouvelles signatures.
- [x] **Phase 2 (Tronc) :** Refactoriser le serveur (Rust MCP) pour enregistrer la nouvelle liste.
- [x] **Phase 3 (Feuilles) :** Fusionner la logique (diff, batch, cypher, inspect, etc.).
- [x] **Phase 4 (Purge) :** Supprimer les anciens outils redondants.
- [x] **Phase 5 (Qualité) :** Validation 100% PASS et Zéro Warning.
