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
  - `151` tests passés (`109` lib + `42` bin)
  - `0` échec
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
  - `40` tests passés
  - `0` échec
- `bash scripts/start-v2.sh`
  - dashboard prêt
  - SQL prêt
  - MCP prêt
- `bash scripts/stop-v2.sh`
  - arrêt propre

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
  - affichage du budget Rust courant, des réservations en vol, du taux d’épuisement, de la profondeur de queue, du mode runtime, des refus `oversized` et des entrées en mode dégradé
  - affichage de la pression hôte observée (`HOST_CPU`, `HOST_RAM`, `HOST_IO_WAIT`) et de l’état contraint/repris des queues, sans reprendre l’autorité de scheduling

Il n’existe plus de voie canonique `Titan` dans le runtime Rust.
Les gros fichiers sont désormais traités par budget, packing et refus explicite `oversized_for_current_budget`, pas par un seuil métier fixe.
Les gros fichiers différés accumulent aussi maintenant une dette de fairness persistante (`defer_count`) afin d’éviter leur affamement derrière des vagues infinies de petits fichiers.
Avant un refus `oversized` final, Axon accorde désormais une courte probation de déferrement aux candidats encore froids pour éviter qu’une estimation initiale trop conservatrice ne les exclue trop tôt.
`StatsCache` n’est plus supervisé sur le chemin actif du dashboard, et `PoolFacade` alimente désormais `Telemetry` directement pour les événements `FileIndexed` et `RuntimeTelemetry`.

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
