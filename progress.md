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
