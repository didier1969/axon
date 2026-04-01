# État du Projet : Axon

## Snapshot vérifié

Date de référence: `2026-04-01`

Ce document décrit l’état **prouvé** du projet, pas son récit aspiratoire.

## Ce qui est vérifié

- environnement officiel: `devenv shell`
- core Rust: tests verts
- dashboard Elixir: tests verts
- runtime canonique: `scripts/start-v2.sh` monte correctement dashboard, SQL et MCP
- backend nominal courant: **Canard DB** (`DuckDB`)

## Validation fraîche connue

- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'`
  - `147` tests passés (`108` lib + `39` bin)
  - `0` échec
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
  - `38` tests passés
  - `0` échec
- `bash scripts/start-v2.sh`
  - dashboard prêt
  - SQL prêt
  - MCP prêt

## Contrat d’architecture actuel

- **Rust**
  - autorité de runtime
  - ingestion canonique
  - admission canonique par budget mémoire dynamique
  - estimation par `parser class + size bucket + confiance observée`
  - vérité `IST`
  - surfaces `MCP` et `SQL`
- **Elixir/Phoenix**
  - visualisation
  - télémétrie opérateur
  - projections et surface cockpit
  - affichage du budget Rust courant, des réservations en vol, du taux d’épuisement, de la profondeur de queue et du mode runtime

Il n’existe plus de voie canonique `Titan` dans le runtime Rust.
Les gros fichiers sont désormais traités par budget, packing et refus explicite `oversized_for_current_budget`, pas par un seuil métier fixe.

## Dette encore ouverte

Le socle exécutable est sain, mais la migration `Rust-first` n’est pas totalement terminée côté dashboard.

Les zones de dette encore visibles sont principalement:

- `Axon.Watcher.Tracking`
- `Axon.Watcher.StatsCache`
- `Axon.Watcher.Auditor`
- `Axon.Watcher.PoolFacade` comme pont encore trop large
- `Axon.Watcher.PoolEventHandler.process_pending/1`
- `Axon.BackpressureController`

La chaîne legacy suivante a déjà été retirée du dashboard:

- `Axon.Watcher.Server`
- `Axon.Watcher.Staging`
- `Axon.Watcher.PathPolicy`
- `Axon.Watcher.IndexingWorker`
- `Axon.Watcher.BatchDispatch`
- configuration `Oban` d’indexation
- API Elixir de lot `PoolFacade.parse_batch/1` et `PoolFacade.pull_pending/1`
- `Axon.Watcher.TrafficGuardian`

## Comment lire le repo sans se tromper

- lire `README.md` et `docs/getting-started.md` avant toute autre doc
- traiter `docs/archive/` comme historique
- traiter les anciens récits `KuzuDB`, Triple-Pod, HydraDB ou `v1/v2` comme contexte de migration, pas comme contrat courant
