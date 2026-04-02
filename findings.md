# Findings

## 2026-04-01 - Reprise

### 1. Environnement réel vs shell courant
- Le shell courant n'est pas l'environnement officiel du projet.
- Hors `devenv shell`, les toolchains actives proviennent de `mise` et du système (`cargo`, `rustc`, `mix`, `elixir`, `uv`), avec plusieurs variables critiques absentes.
- Dans `devenv shell`, les variables clés et les toolchains Rust/Elixir/Python sont cohérentes avec le contrat du projet.

### 2. Git reality
- La branche active est `feat/rust-first-control-plane`.
- Le worktree est massivement sale, mais `git diff --stat` montre surtout des artefacts `.devenv` et des binaires générés.
- Aucun changement n'est staged au moment de la reprise.

### 3. Divergence documentation / réalité
- `README.md` décrit un workflow Rust-first via DuckDB + dashboard Phoenix comme surface opérateur.
- `docs/working-notes/reality-first-stabilization-handoff.md` affirme que le "Final Gate" de stabilisation est passé et active de nouveaux objectifs `A/B/C`.
- `task_plan.md`, `progress.md` et `STATE.md` précédents racontent une stabilisation largement terminée, sans preuve exécutable récente attachée à cette reprise.
- Conclusion provisoire: le récit documentaire est plus avancé que la preuve terrain actuellement revalidée.

### 4. Risque dominant de reprise
- Le risque principal n'est pas encore un bug métier identifié.
- Le risque dominant est la confiance excessive dans un récit de stabilité qui n'a pas encore été revalidé dans l'environnement officiel au moment de cette reprise.

### 5. Validation exécutable réelle
- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` est vert.
- Résultat Rust: `127` tests exécutés, `127` passés.
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` est vert.
- Résultat dashboard: `35` tests exécutés, `35` passés.
- `bash scripts/start-v2.sh` monte correctement les surfaces canoniques:
  - dashboard prêt
  - SQL prêt
  - MCP prêt
- Probes directes post-démarrage:
  - `/sql` retourne les tables attendues (`File`, `Symbol`, `RuntimeMetadata`, `Chunk`, `GraphProjection`, etc.)
  - `/mcp` retourne une liste d'outils cohérente avec la couche DX/gouvernance/SOLL

### 6. Autorité résiduelle Elixir encore présente
- La dette de migration documentée dans le handoff existe toujours dans le code.
- Les modules suivants sont encore présents et référencés:
  - `Axon.Watcher.Server`
  - `Axon.Watcher.Staging`
  - `Axon.Watcher.IndexingWorker`
  - `Axon.Watcher.BatchDispatch`
  - `Axon.Watcher.PoolFacade`
  - `Axon.Watcher.PoolEventHandler`
  - `Axon.BackpressureController`
  - `Axon.Watcher.TrafficGuardian`
- Conclusion: la narration "Rust-first" est exécutable et crédible, mais la désautorisation complète d'Elixir n'est pas terminée.

### 7. Objectifs `A/B/C` du handoff
- Observation directe:
  - `A` existe partiellement et de manière crédible via les outils MCP de retrieval/DX (`axon_query`, `axon_inspect`, `axon_fs_read`, `axon_bidi_trace`) et leurs tests.
  - `B` existe partiellement via les outils/primitives de garde-fou (`axon_impact`, `axon_diff`, `axon_audit`, `axon_health`, `axon_api_break_check`, `axon_simulate_mutation`).
  - `C` existe partiellement via `SOLL` (`axon_soll_manager`, `axon_export_soll`, `axon_restore_soll`, `axon_validate_soll`) et les tests de continuité.
- Inférence: les objectifs `A/B/C` ne sont pas absents, mais ils cohabitent encore avec une dette structurelle de migration du plan d'exécution.

### 8. Nettoyage documentaire exécuté
- Les snapshots `SOLL_EXPORT_*.md` mal placés ont été déplacés de `src/axon-core/docs/vision/` vers `docs/archive/soll-exports/`.
- Le runtime Rust résout désormais le chemin canonique des exports `SOLL` vers `docs/vision/` au niveau racine du dépôt, indépendamment du `cwd`.
- Les documents obsolètes mais historiquement utiles ont été déplacés vers `docs/archive/`:
  - anciennes docs `v1.0`
  - anciennes docs `v2`
  - anciens documents racine non canoniques
- Les docs canoniques ont été réalignées pour réduire l'ambiguïté de reprise:
  - `README.md`
  - `docs/getting-started.md`
  - `STATE.md`
  - `ROADMAP.md`
  - `docs/working-notes/reality-first-stabilization-handoff.md`

### 9. Point de vérité documentaire après nettoyage
- Un nouveau repreneur doit désormais pouvoir distinguer plus facilement:
  - la documentation canonique courante
  - l'archive historique
  - les snapshots générés
- Les références historiques à `KuzuDB` sont désormais reléguées à l'historique ou explicitement qualifiées comme telles.
- Le backend nominal courant est documenté comme **Canard DB** (`DuckDB`).

## Legacy Context Preserved
- Une ancienne note de findings affirmait une architecture "Zero-Sleep / MVCC / Zero-SELECT".
- Cette direction reste utile comme contexte historique, mais elle ne doit pas être traitée comme preuve de stabilité actuelle sans revalidation exécutable.

## 2026-04-02 - Ingress Guard

### 1. Le problème dominant n'est pas l'absence de structure, mais le churn de `File.status`
- Pour le projet `axon`, la matière structurelle existe déjà sur une grande partie des fichiers actuels.
- Le symptôme dominant est une remise en `pending` de fichiers déjà matérialisés, probablement par le chemin ordinaire scanner/upsert.
- Le backlog visible est donc contaminé par des retraitements et des reliquats historiques.

### 2. Le bon levier n'est pas une deuxième vérité, mais un filtre amont dérivé
- Le composant retenu s'appelle `FileIngressGuard`, pas `MemoryIndex`.
- Son rôle est de filtrer l'ingress avant écriture DB pour éviter les `upsert` inutiles.
- DuckDB reste la seule vérité canonique pour `status`, `priority`, claims et scheduling.

### 3. Le MVP doit rester étroit
- Signal d'entrée du guard: `path + mtime + size`.
- Pas de hash fichier dans le MVP.
- Pas de priorité canonique en mémoire.
- Pas de favoritisme canonique du repo courant.
- Le guard doit `fail-open` et ne jamais bloquer l'ingestion globale si son état est absent ou divergent.

### 4. Les invariants non négociables sont maintenant clairs
- aucune transition `pending -> indexing` ne vit seulement en mémoire
- aucune mise à jour du guard avant commit DB réussi
- toute divergence cache/DB se résout au profit de la DB
- recovery startup et invalidations globales doivent reconstruire le guard

### 5. Deux artefacts canoniques ont été créés
- `docs/plans/2026-04-02-file-ingress-guard-design.md`
- `docs/plans/2026-04-02-file-ingress-guard-implementation-plan.md`

### 6. La première revue experte a resserré le contrat
- le rollout doit avoir un `kill switch` explicite
- le guard doit apprendre depuis la ligne `File` réellement commitée, jamais depuis l’intention scanner/watcher
- le MVP doit réduire son shadow state au strict minimum
- le cas `indexing + changement de metadata` doit rester explicitement compatible avec `needs_reindex`
- la règle de rebuild/invalidation du guard doit être claire par rapport au bootstrap DB

### 7. Le FileIngressGuard est maintenant implémenté et validé
- nouveau module: `src/axon-core/src/file_ingress_guard.rs`
- hydratation au boot depuis `File` après `GraphStore::new()`
- intégration runtime:
  - watcher hot via variantes `stage_hot_delta_with_guard` / `stage_hot_deltas_with_guard`
  - scanner via `scan_with_guard` / `scan_subtree_with_guard`
- mise à jour du guard à partir des lignes `File` réellement relues après commit, pas depuis l’intention d’écriture
- `kill switch` opérationnel via `AXON_ENABLE_FILE_INGRESS_GUARD`
- télémétrie minimale ajoutée:
  - `guard_hits`
  - `guard_misses`
  - `guard_bypassed_total`
  - `guard_hydrated_entries`
  - `guard_hydration_duration_ms`

### 8. Un défaut de test réel a été trouvé pendant la validation complète
- le test du kill switch manipulait l’environnement global et contaminait les autres tests du guard en exécution parallèle
- correction appliquée:
  - verrou statique partagé autour des tests du guard
- conclusion:
  - le défaut était dans la suite de tests, pas dans le runtime
