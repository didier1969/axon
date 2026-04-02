# Progress Log

## 2026-04-01 - Reprise Reality-First
- Relecture des documents de reprise existants: `README.md`, `STATE.md`, `docs/working-notes/reality-first-stabilization-handoff.md`, `docs/plans/2026-03-30-rust-first-stabilization-execution-plan.md`.
- Audit Git initial:
  - branche active `feat/rust-first-control-plane`
  - aucun changement staged
  - diff non staged dominé par `.devenv/*`, `src/axon-core/target/release/axon-core.d`, et `src/dashboard/priv/native/libaxon_scanner.so`
  - nombreux fichiers non trackés sous `.devenv/` et exports `src/axon-core/docs/vision/SOLL_EXPORT_*`
- Validation environnement:
  - `./scripts/validate-devenv.sh` échoue dans le shell courant
  - `devenv shell -- bash -lc './scripts/validate-devenv.sh'` réussit
- Mise à jour des fichiers de pilotage:
  - `task_plan.md` réaligné sur la reprise actuelle
  - `findings.md` réinitialisé pour distinguer héritage documentaire et réalité terrain

## Next Immediate Action
- Prendre un premier signal exécutable côté Rust et côté dashboard dans l'environnement Devenv officiel.

## 2026-04-01 - Validation exécutable
- Core Rust validé dans l'environnement officiel:
  - commande: `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'`
  - résultat: `127` tests passés, `0` échec
- Dashboard validé dans l'environnement officiel:
  - commande: `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
  - résultat: `35` tests passés, `0` échec
- Runtime canonique validé:
  - commande: `bash scripts/start-v2.sh`
  - résultat: dashboard, SQL et MCP prêts
  - probes directes:
    - `/sql` répond avec les tables métier attendues
    - `/mcp` répond avec la liste d'outils attendue
- Vérification de dette structurelle:
  - recherche `rg -n "BackpressureController|PoolFacade|IndexingWorker|BatchDispatch|Watcher" src/dashboard/lib/axon_nexus/axon`
  - conclusion: l'autorité résiduelle Elixir est toujours présente dans le code
- Runtime refermé proprement après validation:
  - commande: `bash scripts/stop-v2.sh`
  - résultat: session `tmux` fermée, sockets/locks nettoyés

## Current Resume Point
- Le projet n'est pas "à reprendre depuis zéro".
- Le socle exécutable actuel est sain.
- La prochaine tranche rationnelle est de traiter la migration incomplète Rust-first côté dashboard/Watcher et d'aligner les documents de statut sur cette réalité.

## 2026-04-01 - Nettoyage documentaire et vérité de reprise
- Plan d'implémentation ajouté:
  - `docs/plans/2026-04-01-document-truth-cleanup-plan.md`
- Durcissement code:
  - export/restore `SOLL` réaligné sur `docs/vision/` au niveau racine du dépôt, indépendamment du `cwd`
  - test Rust ajouté pour éviter le retour des faux exports sous `src/axon-core/docs/vision/`
- Nettoyage documentaire:
  - création de `docs/archive/README.md`
  - déplacement des anciennes docs `v1.0` et `v2` sous `docs/archive/`
  - déplacement de `INSTALL_AUDIT.md` et `expert_prompt.md` sous `docs/archive/root-docs/`
  - déplacement de `80` exports `SOLL` vers `docs/archive/soll-exports/`
  - ajout d'une règle `.gitignore` pour les exports `SOLL` mal placés sous `src/axon-core/docs/vision/`
- Réalignement docs canoniques:
  - `README.md`
  - `docs/getting-started.md`
  - `STATE.md`
  - `ROADMAP.md`
  - `docs/working-notes/reality-first-stabilization-handoff.md`
- Vérifications:
  - `cargo test` Rust complet vert
  - `mix test` dashboard vert
  - contrôle filesystem:
    - `src/axon-core/docs/vision/` vide
    - `docs/archive/soll-exports/` contient `80` fichiers

## Errors Encountered
- `cargo test` initialement appelé avec plusieurs noms de tests dans une seule commande
  - résolution: rerun en commandes ciblées séparées puis suite complète
- `mix test` a initialement échoué car `Hex` n'était pas préinstallé dans cette session shell
  - résolution: rerun avec `mix local.hex --force` et `mix local.rebar --force` avant `mix test`

## 2026-04-01 - Retrait de la chaîne legacy Elixir d’ingestion
- TDD de frontière ajouté côté dashboard:
  - `src/dashboard/test/axon_dashboard/legacy_control_plane_boundary_test.exs`
  - objectif: verrouiller l'absence de configuration `Oban` et d'API batch legacy côté `PoolFacade/PoolProtocol`
- Tranche de suppression exécutée:
  - suppression de `Axon.Watcher.Server`
  - suppression de `Axon.Watcher.Staging`
  - suppression de `Axon.Watcher.PathPolicy`
  - suppression de `Axon.Watcher.IndexingWorker`
  - suppression de `Axon.Watcher.BatchDispatch`
  - suppression de la migration `Oban` historique
  - retrait de la dépendance `Oban` du dashboard
  - réduction de `PoolFacade` au scan explicite, à la télémétrie entrante et aux requêtes SQL
  - réduction de `PoolProtocol` à `split_lines/1`
- Réalignement read-side:
  - `BackpressureController` ne garde plus de faux point d’extension `oban_mod`
  - tests dashboard mis à jour pour refléter la frontière visualization-only
- Validation fraîche:
  - `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `38` tests verts
  - `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `147` tests verts
  - `bash scripts/start-v2.sh` -> vert
  - `bash scripts/stop-v2.sh` -> vert

## 2026-04-01 - Télémétrie runtime Rust visible dans le cockpit
- Nouveau flux bridge `RuntimeTelemetry` ajouté côté Rust:
  - budget mémoire courant
  - mémoire réservée en vol
  - taux d’épuisement
  - profondeur de queue
  - `claim_mode`
  - `service_pressure`
- Le cockpit Phoenix affiche maintenant ces signaux Rust en lecture seule sur la route `/cockpit`.
- `PoolFacade` met aussi à jour `Axon.Watcher.Telemetry` à partir de `RuntimeTelemetry`, ce qui évite de dépendre de `TrafficGuardian`.
- Régression couverte par TDD:
  - test Rust de sérialisation `BridgeEvent::RuntimeTelemetry`
  - test LiveView du cockpit racine
  - test dashboard de frontière legacy
  - test watcher `pipeline_maillons`

## 2026-04-02 - Formalisation de la tranche FileIngressGuard
- Relecture et consolidation des constats runtime sur:
  - churn massif de `pending`
  - goulot `writer_ctx`
  - qualité MCP hétérogène
  - absence de filtre amont avant `bulk_insert_files` / `upsert_hot_file`
- Décision documentée:
  - nouveau composant canonique nommé `FileIngressGuard`
  - DuckDB reste seule autorité de priorité, claim et statut
  - pas de hash fichier dans le MVP
  - pas de favoritisme canonique du repo courant
- Aide experte consolidée:
  - invariants non négociables de cohérence DB/cache
  - séquence TDD minimal-risque pour rollout watcher puis scanner
- Artefacts produits:
  - `docs/plans/2026-04-02-file-ingress-guard-design.md`
  - `docs/plans/2026-04-02-file-ingress-guard-implementation-plan.md`

## Next Immediate Action
- Relire les deux artefacts avec l'utilisateur, puis passer à l'implémentation TDD de la phase `FileIngressGuard`.

## 2026-04-02 - Durcissement après revue experte
- Trois experts ont relu le design et le plan.
- Verdict convergent initial: `valide avec réserves`, pas `100%`.
- Corrections intégrées dans les deux artefacts:
  - `kill switch` explicite
  - update du guard depuis la ligne `File` commitée
  - shadow state MVP réduit
  - invariant de boot explicite
  - cas `indexing + metadata changed` rendu explicite
  - invalidation/rebuild du guard clarifiée

## Next Immediate Action
- relancer la revue experte binaire sur la version corrigée

## 2026-04-02 - Implémentation TDD du FileIngressGuard
- Rouge:
  - tests du contrat `FileIngressGuard` ajoutés dans `src/axon-core/src/tests/maillon_tests.rs`
  - premier run: échec attendu car le module n’existait pas
- Vert:
  - création de `src/axon-core/src/file_ingress_guard.rs`
  - ajout du kill switch `AXON_ENABLE_FILE_INGRESS_GUARD`
  - helpers de relecture `File` commitée dans `src/axon-core/src/graph_ingestion.rs`
  - branchement du scanner et du watcher avec variantes `with_guard`
  - hydratation au boot dans `src/axon-core/src/main.rs`
  - passage du guard partagé vers `main_background`
  - ajout de la télémétrie minimale du guard dans `bridge.rs`, `main_background.rs`, `main_telemetry.rs`
- Incident de validation:
  - pollution inter-tests par variable d’environnement du kill switch
  - corrigée via verrou statique dans les tests
- Vérifications fraîches:
  - `cargo test` complet `src/axon-core` vert (`138` + `44` + doctests)
  - `mix test` complet `src/dashboard` vert (`31`)
  - `stop-v2 -> start-v2 -> stop-v2` vert

## Next Immediate Action
- préparer la prochaine tranche d’observabilité/causalité des requeues et l’investigation mémoire sur relâchement du working set

## 2026-04-02 - Pré-analyse mémoire DuckDB/allocateur/WSL
- Investigation locale du code:
  - absence de `malloc_trim`
  - absence de `DuckDB memory_limit` explicite
  - absence de `temp_directory` / `max_temp_directory_size`
  - présence d’un `CHECKPOINT` seulement au bootstrap
  - allocateur courant = système par défaut, pas `jemalloc`
- Recherche web croisée:
  - docs officielles DuckDB
  - docs GNU/glibc
  - docs WSL
  - retours communauté sur RSS DuckDB élevé après pics
- Conclusion provisoire documentée:
  - on ne doit pas encore changer l’allocateur à l’aveugle
  - il faut d’abord séparer `RssAnon` et `RssFile`, puis exposer `duckdb_memory()` et `duckdb_temporary_files()`
  - `CHECKPOINT`/WAL et relâchement du RSS sont deux sujets distincts

## Next Immediate Action
- ouvrir une tranche dédiée d’instrumentation mémoire post-pic avant toute tentative de purge ou changement d’allocateur

## 2026-04-02 - Observabilité mémoire + première causalité `pending`
- Rouge:
  - tests Rust ajoutés pour la forme enrichie de `RuntimeTelemetry`
  - test MCP ajouté pour `axon_debug` opératoire
  - test dashboard ajouté pour le bridge mémoire enrichi
  - tests Rust ajoutés pour `status_reason` sur:
    - rescan scanner
    - hot delta watcher
    - recovery après redémarrage
- Vert:
  - nouveau module partagé `runtime_observability`
  - `GraphStore` conserve maintenant le chemin DB pour calculer `ist.db` / WAL
  - `RuntimeTelemetry` expose mémoire process + stockage DB + mémoire DuckDB
  - `axon_debug` affiche backlog réel, mémoire détaillée et stockage DuckDB
  - ETS dashboard absorbe les nouveaux champs mémoire
  - `File.status_reason` est ajouté et renseigné sur les premiers chemins critiques
- Validation fraîche:
  - `cargo test --manifest-path Cargo.toml` dans `src/axon-core` vert (`144` + `44`)
  - `mix test` dans `src/dashboard` vert (`31`)
  - `stop-v2 -> start-v2 -> stop-v2` vert

## Next Immediate Action
- exploiter `status_reason` pour expliquer le backlog dans les vues opératoires
- mesurer un snapshot réel `RssAnon` vs `RssFile` sur run long avant toute purge mémoire

## 2026-04-02 - MCP operatoire: backlog explique + completude de scope
- Rouge:
  - test MCP ajoute pour les causes dominantes du backlog dans `axon_debug`
  - test MCP ajoute pour la completude d'un scope projet dans `axon_query`
- Vert:
  - `axon_debug` affiche maintenant les causes dominantes du backlog global (`status_reason`)
  - `axon_query`, `axon_inspect`, `axon_impact`, `axon_audit` et `axon_health` exposent une note de completude du scope projet
  - la note de completude annonce:
    - fichiers termines / fichiers connus
    - backlog visible
    - repartition `pending` / `indexing`
    - causes backlog dominantes quand elles existent
  - `axon_audit` et `axon_health` utilisent maintenant `project_slug` uniquement pour compter le scope, plus de fallback sur un `path LIKE` ambigu
- Validation fraiche:
  - `cargo test --manifest-path Cargo.toml` dans `src/axon-core` vert (`146` + `44`)

## Next Immediate Action
- exploiter ces nouvelles notes operatoires sur un vrai run long pour separer:
  - backlog reel
  - backlog historiquement rematerialise
- mesurer `RssAnon` vs `RssFile` en charge soutenue avant toute action de purge memoire

## 2026-04-02 - Causalite de scheduling: claim + defer explicites
- Rouge:
  - test Rust ajoute pour verifier qu'un `fetch_pending_batch` pose une raison `claimed_for_indexing`
  - test Rust ajoute pour verifier qu'un deferrement pose `deferred_by_scheduler` puis qu'une claim remplace cette raison
- Vert:
  - `fetch_pending_batch` et `claim_pending_paths` renseignent maintenant `status_reason = 'claimed_for_indexing'`
  - `mark_pending_files_deferred` renseigne maintenant `status_reason = 'deferred_by_scheduler'`
  - les transitions `pending -> indexing` et `pending` differe deviennent enfin explicables sans lecture de code
- Validation fraiche:
  - `cargo test --manifest-path Cargo.toml` dans `src/axon-core` vert (`147` + `44`)

## Next Immediate Action
- fermer le reste des transitions encore silencieuses de la machine d'etat
- mesurer ensuite un vrai run long memoire + backlog sur cette base causale plus complete

## 2026-04-02 - Statut final `indexed` rendu explicite
- Rouge:
  - test Rust ajoute pour verifier qu'un commit complet pose une raison finale explicite au lieu de `NULL`
- Vert:
  - `insert_file_data_batch` renseigne maintenant `status_reason = 'indexed_success_full'` pour un commit complet reussi
  - le succes `indexing -> indexed` n'est plus un statut final muet
- Validation fraiche:
  - `cargo test --manifest-path Cargo.toml` dans `src/axon-core` vert (`148` + `44`)
  - `mix test` dans `src/dashboard` vert (`31`)

## Next Immediate Action
- mesurer un vrai run long memoire + backlog sur cette base causale encore plus complete
- inventorier ce qui reste silencieux dans les transitions rares avant de declarer la machine d'etat quasi fermee

## 2026-04-02 - Run long V2: memoire reelle + disponibilite MCP
- Vert:
  - nouveau script operatoire [scripts/monitor_runtime_v2.py](/home/dstadel/projects/axon/scripts/monitor_runtime_v2.py)
  - monitoring reel sur `90s` avec export CSV dans `.axon/observability/runtime_monitor_2026-04-02.csv`
  - benchmark MCP HTTP reel relance en `3` passes pendant le run
- Resultats observes:
  - backlog stable et anormalement peu actif pendant la fenetre:
    - `49_008` fichiers connus
    - `504` termines
    - `48_504` pending
    - `0` indexing
  - memoire:
    - `RSS` entre `6.99 GB` et `7.51 GB`
    - `RssAnon` entre `6.93 GB` et `7.44 GB`
    - `RssFile` stable autour de `67-68 MB`
    - base DuckDB totale autour de `6.16 GB`
  - MCP:
    - `15/16` succes sur chacun des `3` passages
    - `axon_simulate_mutation` reste en erreur
    - latence moyenne observee selon le passage: `~173 ms`, `~51 ms`, `~178 ms`
    - pics visibles sur `axon_query`, `axon_audit`, `axon_batch`, parfois `axon_impact`
- Conclusion technique:
  - la memoire observee n'est pas principalement du cache fichier DuckDB (`RssFile` tres faible)
  - le gros du RSS est du cote `RssAnon`, donc plutot heap/runtime/allocation que page cache
  - la disponibilite MCP est bonne sur la fenetre mesuree, mais la latence n'est pas stable
  - la table `File.status` reste problematique pendant ce run (`0 indexing` malgre un backlog massif)

## Next Immediate Action
- investiguer pourquoi la fenetre mesuree montre `0 indexing` avec backlog massif
- ouvrir ensuite la correction architecturale lecture/ecriture DB avec preuves runtime en main

## 2026-04-02 - Plan detaille pour le tampon memoire d'ingress
- Nouvelle decision formalisee:
  - `Watcher` et `Scanner` doivent devenir des producteurs d'ingress, pas des ecrivains canoniques directs dans `File`
  - `DuckDB` reste la seule verite canonique pour `pending/indexing/indexed`
  - le MVP du tampon d'ingress est **memoire seulement**
  - pas de WAL disque ni de seconde base de donnees pour l'ingress brut dans le MVP
- Nouveaux artefacts:
  - `docs/plans/2026-04-02-ingress-buffer-design.md`
  - `docs/plans/2026-04-02-ingress-buffer-implementation-plan.md`
- Cible d'architecture:
  - `Watcher/Scanner -> IngressBuffer -> IngressPromoter -> DuckDB File -> claim -> QueueStore -> workers`
- Motif principal:
  - separer detection brute et decision canonique
  - batcher les ecritures vers DuckDB
  - reduire le churn artificiel de `pending`

## Next Immediate Action
- executer le plan `IngressBuffer` en TDD, en gardant le patch local d'exploration watcher hors de la tranche tant qu'il n'est pas requalifie contre ce nouveau design

## 2026-04-02 - Tranche `IngressBuffer` executee et validee
- Rouge:
  - contrat `IngressBuffer` gele par tests sur:
    - collapse multi-evenements pour un meme path
    - priorite maximale retenue
    - tombstone prioritaire sur une observation stale
    - hints de sous-arbre sans restaging recursif direct
    - promotion batch canonique reduite
  - invariants additionnels ajoutes:
    - scanner avec ingress buffer necrit plus canoniquement avant promotion
    - watcher avec ingress buffer necrit plus canoniquement avant promotion
- Vert:
  - nouveau module `src/axon-core/src/ingress_buffer.rs`
  - ajout du kill switch `AXON_ENABLE_INGRESS_BUFFER`
  - ajout de `GraphStore::promote_ingress_batch(...)`
  - `Scanner` converti en producteur d'ingress via `scan_with_guard_and_ingress` / `scan_subtree_with_guard_and_ingress`
  - `fs_watcher` converti en producteur d'ingress via `enqueue_hot_delta_with_guard` / `enqueue_hot_deltas_with_guard`
  - les evenements de dossier watcher deviennent des `subtree_hints`, plus des restagings recursifs immediats
  - boucle runtime `IngressPromoter` ajoutee dans `main_background`
  - mise a jour du `FileIngressGuard` strictement apres verite committee
  - `RuntimeTelemetry` et `axon_debug` exposent maintenant l'etat du buffer:
    - active ou non
    - entrees bufferisees
    - subtree hints
    - evenements collapses
    - flush count
    - dernier flush
    - dernier lot promu
  - ETS dashboard absorbe aussi ces nouveaux champs read-only
- Validation fraiche:
  - `cargo test --manifest-path src/axon-core/Cargo.toml` vert (`155` + `44`)
  - `devenv shell -- bash -lc 'cd src/dashboard && mix test'` vert (`31`)
  - `bash scripts/stop-v2.sh && bash scripts/start-v2.sh && bash scripts/stop-v2.sh` vert

## Next Immediate Action
- exploiter maintenant le buffer en run long pour mesurer:
  - reduction du churn canonique `pending`
  - effet reel sur la disponibilite MCP
  - comportement memoire apres stabilisation de l'ingress

## 2026-04-02 - Fermeture hors dashboard: DB routing, causalite d'echec, relachement memoire
- Vert:
  - lectures read-only routées sur `reader_ctx` avec garde de fraicheur tres courte apres write pour eviter une verite stale
  - helpers read-only réalignés:
    - `query_json`
    - `query_count`
    - `fetch_unembedded_symbols`
    - `fetch_unembedded_chunks`
    - chargement des jobs `GraphEmbedding`
  - gateway SQL brute introduite:
    - lectures -> chemin read-only
    - mutations -> writer canonique avec reponse `{\"ok\":true}`
  - les chemins `RAW_QUERY` et MCP SQL passent par cette gateway
  - la causalite des echecs de scheduling/commit est maintenant explicite:
    - `requeued_after_queue_push_failure`
    - `requeued_after_writer_batch_failure`
  - le writer n’envoie plus de feedback `FileIndexed` si le commit batch DuckDB echoue
  - reclaimer memoire Linux ajoute, tres conservateur:
    - idle-only
    - activable/desactivable via `AXON_ENABLE_MEMORY_RECLAIMER`
    - seuil anon via `AXON_MEMORY_RECLAIMER_MIN_ANON_MB`
- Validation fraiche:
  - `cargo test --manifest-path src/axon-core/Cargo.toml` vert (`159` + `47`)

## Next Immediate Action
- valider encore `mix test` dashboard puis `start-v2.sh` / `stop-v2.sh`
- si vert, geler la doc, commit et push de la tranche finale hors dashboard
