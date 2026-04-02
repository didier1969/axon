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

### 9. La prochaine tranche mémoire doit commencer par distinguer nature du RSS et leviers réels
- Axon ne fait actuellement ni `malloc_trim`, ni réglage explicite `DuckDB memory_limit`, ni `temp_directory`, ni instrumentation `RssAnon/RssFile`.
- DuckDB documente que `memory_limit` ne borne que le `buffer manager`; le RSS réel peut donc dépasser nettement cette limite.
- `CHECKPOINT` aide surtout le WAL et la persistance disque, pas une baisse garantie du RSS.
- La bonne première mesure n'est pas de changer l'allocateur, mais de distinguer:
  - `RssAnon`
  - `RssFile`
  - working set DuckDB via `duckdb_memory()`
  - spill via `duckdb_temporary_files()`
- Si le pic est surtout `RssAnon`, `malloc_trim` ou un allocateur plus agressif redeviennent de vrais candidats.
- Si le pic est surtout `RssFile`, il faut viser working set/cache et pas l'allocateur.

### 10. La tranche d’observabilité mémoire est maintenant en place
- `RuntimeTelemetry` expose désormais:
  - `rss_bytes`
  - `rss_anon_bytes`
  - `rss_file_bytes`
  - `rss_shmem_bytes`
  - `db_file_bytes`
  - `db_wal_bytes`
  - `db_total_bytes`
  - `duckdb_memory_bytes`
  - `duckdb_temporary_bytes`
- `axon_debug` n’affiche plus seulement le volume du graphe; il agrège maintenant:
  - volume graphe
  - backlog réel
  - mémoire runtime détaillée
  - stockage DuckDB
  - mémoire DuckDB agrégée

### 11. La causalité `pending` a une première vérité persistée
- nouvelle colonne canonique `File.status_reason`
- causes explicitement persistées sur plusieurs chemins critiques:
  - `metadata_changed_scan`
  - `metadata_changed_hot_delta`
  - `recovered_interrupted_indexing`
  - `needs_reindex_while_indexing`
  - `soft_invalidated`
  - `manual_or_system_requeue`
  - `oversized_for_current_budget`
- conclusion:
  - le problème `pending` n’est pas totalement fermé
  - mais la base donne maintenant une première explication persistée du churn au lieu d’un simple statut brut

### 12. Les vues MCP opératoires annoncent maintenant le niveau de vérité du scope projet
- `axon_debug` expose désormais les causes dominantes du backlog global à partir de `File.status_reason`
- les outils MCP scope-projet (`axon_query`, `axon_inspect`, `axon_impact`, `axon_audit`, `axon_health`) annoncent la complétude visible du projet demandé:
  - fichiers terminés / fichiers connus
  - backlog visible
  - répartition `pending` / `indexing`
  - causes backlog dominantes
- `axon_audit` et `axon_health` utilisent maintenant `project_slug` comme frontière de scope au lieu d’un `path LIKE` ambigu

### 13. Les transitions de scheduling critiques portent maintenant une cause explicite
- `fetch_pending_batch` et `claim_pending_paths` posent maintenant `status_reason = 'claimed_for_indexing'`
- `mark_pending_files_deferred` pose maintenant `status_reason = 'deferred_by_scheduler'`
- conclusion:
  - on sait maintenant distinguer un backlog simplement en attente d'execution d'un backlog volontairement differe
  - la causalité `pending/indexing` reste incomplète, mais le scheduler n'est plus silencieux sur ces deux transitions majeures

### 14. Le succès complet d'indexation est maintenant explicite
- `insert_file_data_batch` pose maintenant `status_reason = 'indexed_success_full'` sur le chemin nominal complet
- conclusion:
  - `indexed` n'est plus un état final sans cause
  - la lecture opératoire peut désormais distinguer un succès complet d'un état final dégradé ou d'un reliquat historique

### 15. Le premier run long invalide l'hypothèse dominante "page cache DuckDB"
- run réel `90s` mesuré via `scripts/monitor_runtime_v2.py`
- mesures observées:
  - `RSS`: ~`6.99 GB` à `7.51 GB`
  - `RssAnon`: ~`6.93 GB` à `7.44 GB`
  - `RssFile`: ~`67-68 MB`
  - base DuckDB totale: ~`6.16 GB`
- conclusion:
  - la mémoire occupée par Axon n'est pas majoritairement du cache fichier OS
  - le problème mémoire est beaucoup plus probablement du côté heap/runtime/allocation/working set anonyme

### 16. Le serveur MCP est disponible, mais sa latence reste instable sous run réel
- benchmark HTTP réel en `3` passes pendant le run:
  - `15/16` succès à chaque passage
  - `axon_simulate_mutation` reste en erreur
  - latence moyenne observée: `~173 ms`, `~51 ms`, `~178 ms`
- conclusion:
  - la disponibilité MCP n'est pas le premier problème sur cette fenêtre
  - la qualité de service n'est pas encore stable, surtout sur `axon_query`, `axon_audit`, `axon_batch`, parfois `axon_impact`

### 17. Le phénomène `0 indexing` persiste pendant un backlog massif
- pendant la fenêtre mesurée:
  - `49_008` fichiers connus
  - `504` terminés
  - `48_504` pending
  - `0` indexing
  - aucune cause backlog dominante visible (`none`)
- conclusion:
  - l'incohérence runtime/statuts n'est pas résolue
  - c'est maintenant le prochain point causal à investiguer avant de conclure sur le scheduler ou le goulot DB

### 18. Le bon amortisseur n'est pas un WAL disque d'ingress mais un tampon memoire
- la distinction correcte est:
  - DuckDB = verite canonique durable
  - IngressBuffer = bruit amorti et reduit en memoire
- il n'est pas necessaire de persister la file brute watcher/scanner dans le MVP
- le redemarrage peut reconstruire l'etat transitoire par rescan + watcher + FileIngressGuard

### 19. La separation DB utile n'est pas "reader toujours", mais "reader quand la lecture reste fraiche"
- un `reader_ctx` DuckDB long-vivant peut observer une verite stale juste apres des ecritures writer
- une bascule naive de toutes les lectures sur `reader_ctx` casse des invariants de fraicheur
- la bonne correction retenue est:
  - lectures pures sur `reader_ctx`
  - sauf dans une tres courte fenetre post-write, ou la lecture repasse sur le writer pour garantir la fraicheur
- conclusion:
  - la separation lecture/ecriture est maintenant reelle sans sacrifier la coherence immediate des tests operatoires

### 20. Le dernier trou majeur de causalite etait le faux succes après echec writer
- avant correction, le writer actor pouvait:
  - echouer a commit le batch DB
  - puis emettre quand meme le feedback succes `FileIndexed`
- ce trou etait plus grave qu'un simple manque de `status_reason`, car il contaminait la verite runtime
- correction retenue:
  - aucun succes emis avant commit DB reussi
  - requeue explicite des fichiers claims si le commit batch echoue
  - raison persistée: `requeued_after_writer_batch_failure`

### 21. La strategie memoire active doit rester prudente et conditionnelle
- le run long a montre un RSS majoritairement `RssAnon`, pas `RssFile`
- il etait donc defendable d'ajouter un mecanisme de relachement memoire
- mais pas en permanence et pas en charge
- la strategie retenue est:
  - trim system allocator Linux
  - uniquement en phase idle
  - uniquement au-dessus d'un seuil anon configurable
  - kill switch explicite
- conclusion:
  - on n'a pas reduit la voilure d'Axon
  - on a ajoute un mecanisme de relachement opportuniste, pas une politique d'appauvrissement permanent
  - `DuckDB` = vérité canonique des fichiers, statuts, graphes et scheduling
  - `ingress` = événements bruts de découverte produits par watcher et scanner
- conclusion de design retenue:
  - les événements bruts n'ont pas besoin d'être persistés dans le MVP
  - un tampon mémoire suffit car le système peut reconstruire cette pression au redémarrage via scan + watcher + hydratation du `FileIngressGuard`
  - le vrai manque actuel est une couche de réduction entre découverte et écriture canonique

### 19. Le pipeline cible doit séparer détection brute et décision canonique
- cible retenue:
  - `Watcher/Scanner -> IngressBuffer -> IngressPromoter -> DuckDB File -> claim -> QueueStore -> workers`
- implication:
  - `Watcher` et `Scanner` deviennent des producteurs d'ingress
  - `File.status = pending` n'est écrit qu'après promotion batchée
  - les batchs vers DuckDB sont préférables aux écritures unitaires continues

### 20. Un plan détaillé d'implémentation existe maintenant pour cette tranche
- nouveaux artefacts:
  - `docs/plans/2026-04-02-ingress-buffer-design.md`
  - `docs/plans/2026-04-02-ingress-buffer-implementation-plan.md`
- la migration prévue reste progressive:
  - buffer isolé
  - promoteur batch
  - watcher producteur
  - scanner producteur
  - vérité MCP/opératoire réalignée ensuite

### 21. La tranche `IngressBuffer` est maintenant réellement implémentée
- nouveau module canonique:
  - `src/axon-core/src/ingress_buffer.rs`
- nouvelles responsabilités effectives:
  - `IngressBuffer` absorbe et fusionne les événements bruts en mémoire
  - `GraphStore::promote_ingress_batch(...)` pousse les décisions canoniques réduites vers `File`
  - `spawn_ingress_promoter(...)` vide le buffer par batchs dans le runtime
- conclusion:
  - le projet ne repose plus uniquement sur `FileIngressGuard`
  - il existe maintenant bien une couche séparée entre découverte brute et écriture canonique

### 22. `Watcher` et `Scanner` ne sont plus contraints d'écrire directement dans DuckDB
- le scanner passe désormais par:
  - `scan_with_guard_and_ingress`
  - `scan_subtree_with_guard_and_ingress`
- le watcher passe désormais par:
  - `enqueue_hot_delta_with_guard`
  - `enqueue_hot_deltas_with_guard`
- les événements de répertoire watcher deviennent des `subtree_hints`, plus des restagings récursifs immédiats
- conclusion:
  - la détection reste rapide
  - mais la canonisation `pending` est maintenant amortie et batchée

### 23. La tranche est validée de bout en bout
- validations fraîches:
  - `cargo test --manifest-path src/axon-core/Cargo.toml` vert (`155` + `44`)
  - `devenv shell -- bash -lc 'cd src/dashboard && mix test'` vert (`31`)
  - `bash scripts/stop-v2.sh && bash scripts/start-v2.sh && bash scripts/stop-v2.sh` vert
- surfaces opératoires réalignées:
  - `RuntimeTelemetry` expose l'état du buffer d’ingress
  - `axon_debug` affiche désormais aussi l’état du buffer
- conclusion:
  - la tranche d’architecture `IngressBuffer` est fermée
  - les prochaines questions redeviennent des questions de comportement mesuré en run long, pas de design manquant

### 24. Le cockpit Phoenix devait etre refondu, pas simplement repeint
- le cockpit precedent restait centre sur une narration "unit/unit" et des signaux machine peu utiles
- il ne rendait pas assez visibles:
  - la completude du workspace
  - les causes du backlog
  - la readiness par projet
  - le buffer d'ingress
  - la decomposition memoire utile
- conclusion:
  - la refonte correcte etait une refonte de structure d'information, pas un simple lifting CSS

### 25. Les CDN etaient un defaut reel dans le dashboard
- `root.html.heex` chargeait encore Google Fonts
- `Axon.Watcher.Layouts` injectait encore `phoenix` et `phoenix_live_view` depuis jsdelivr
- conclusion:
  - le dashboard n'etait pas conforme a la contrainte de souverainete des assets
  - la refonte devait aussi supprimer ces dependances externes

### 26. Le cockpit doit maintenant montrer l'etat operatoire qu'un humain et un LLM peuvent exploiter
- vue cible retenue et implementee:
  - workspace
  - backlog et causes
  - projets
  - runtime
  - ingress
  - memoire
  - activite recente
- conclusion:
  - on ne montre plus seulement des parametres machine
  - on montre enfin la verite d'exploitation du systeme

### 27. `soll.db` derive encore du schema courant
- fait confirme:
  - le restore `axon_restore_soll` a d'abord echoue car `soll.Vision` ne portait pas `goal`
  - apres correction additive, le restore a ensuite echoue car `soll.Decision` ne portait pas `rationale`
- conclusion:
  - `CREATE TABLE IF NOT EXISTS` ne suffit pas pour `SOLL`
  - il faut une migration additive explicite des tables `soll.*` au boot, comme pour `File`

### 28. Les lectures `SOLL` sur `reader_ctx` peuvent etre stale sur la base attachee
- fait confirme:
  - `axon_soll_manager` retournait un succes de creation `Stakeholder`
  - mais `SELECT * FROM soll.Stakeholder` renvoyait `0` ligne via le dataplane
  - un test file-backed a reproduit le symptome apres expiration de la fenetre de fraicheur writer
- cause:
  - `reader_ctx` lisait une vue stale de `sanctuary/soll.db`
  - les ecritures etaient bien faites par le writer, mais la lecture read-only n'en voyait pas toujours l'etat courant
- correction:
  - toute requete `soll.` est maintenant routee vers `writer_ctx`
- conclusion:
  - pour `SOLL`, la coherence immediate compte plus que l'isolation lecture seule
