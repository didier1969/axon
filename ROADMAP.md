# Roadmap: axon

## 💎 v1.0 - The Intelligent Immune System (IN PROGRESS)
**Focus :** Intelligence de flux, Intégration HydraDB et Performance Native.

### 🛡️ Sécurité & Audit (The Taint Analysis Engine)
- [ ] **Analyse de Propagation (Taint Analysis) :** Passer d'une recherche par mots-clés à un suivi réel de la donnée (`Source` -> `Sanitizer` -> `Sink`).
- [ ] **Détection de Backdoors sémantiques :** Identifier les fonctions dont le nom cache la dangerosité réelle (ex: `run_task` qui appelle `eval`).
- [ ] **Clustering Auto-Adaptatif :** Remplacer les seuils fixes par une analyse de la densité locale du graphe.
- [ ] **Visualisation de Flux :** Exporter les chemins d'exposition critiques vers des diagrammes Mermaid/SVG.

### ⚡ Performance & Scalabilité (The Hydra Engine)
- [ ] **Intégration HydraDB :** Déportation de la persistence vers RocksDB/Dolt (Elixir/Rust).
- [ ] **Stratégie Lazy vs Eager :** Implémentation de la file d'attente de tâches de fond via OTP (supervision Elixir).
- [ ] **Embeddings Parallélisés :** Réduction radicale du temps d'indexation (Cible : < 10min pour 40k symboles).

### 🧠 Intelligence & UX
- [ ] **Audit Proactif :** Alerte automatique dès qu'un changement dégrade le score de sécurité.
- [ ] **Traçage Polyglotte :** Traversée automatique des frontières (ex: Elixir ↔ Rust NIFs).

---

## ✅ v0.9 - The Structural Copilot (COMPLETED 2026-03-07)
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
