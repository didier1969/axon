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
  - `173` tests passés (`129` lib + `44` bin)
  - `0` échec
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
  - `31` tests passés
  - `0` échec
- `bash scripts/start-v2.sh`
  - dashboard prêt
  - SQL prêt
  - MCP prêt
- `bash scripts/stop-v2.sh`
  - arrêt propre
- `curl -sS -X POST http://127.0.0.1:44129/mcp ... axon_query`
  - réponse valide en runtime réel
  - mode explicite `hybride` si le worker sémantique est prêt et si la pression service le permet
  - fallback explicite `structurel (embedding temps reel indisponible)` sinon

## Contrat d’architecture actuel

- **Rust**
  - autorité de runtime
  - ingestion canonique
  - admission canonique par budget mémoire dynamique
  - estimation par `parser class + size bucket + confiance observée`
  - vérité `IST`
  - surfaces `MCP` et `SQL`
  - embeddings de requête MCP servis par le worker sémantique Rust isolé déjà chargé, pas par un chargement de modèle jetable par requête
- **Elixir/Phoenix**
  - visualisation
  - télémétrie opérateur read-only issue du bridge Rust
  - projections et surface cockpit
  - affichage du budget Rust courant, des réservations en vol, du taux d’épuisement, de la profondeur de queue, du mode runtime, des refus `oversized` et des entrées en mode dégradé
  - affichage de la pression hôte observée (`HOST_CPU`, `HOST_RAM`, `HOST_IO_WAIT`) et d’un état hôte dérivé du runtime Rust, sans reprendre l’autorité de scheduling

Il n’existe plus de voie canonique `Titan` dans le runtime Rust.
Les gros fichiers sont désormais traités par budget, packing et refus explicite `oversized_for_current_budget`, pas par un seuil métier fixe.
Les gros fichiers différés accumulent aussi maintenant une dette de fairness persistante (`defer_count`) afin d’éviter leur affamement derrière des vagues infinies de petits fichiers.
Avant un refus `oversized` final, Axon accorde désormais une courte probation de déferrement aux candidats encore froids pour éviter qu’une estimation initiale trop conservatrice ne les exclue trop tôt.
Si l’enveloppe `full` ne passe pas mais qu’une enveloppe `structure_only` passe encore, Axon admet désormais le fichier en mode dégradé au lieu de le refuser immédiatement.
Un commit `structure_only` persiste la vérité structurelle (`Symbol`, `CONTAINS`, relations) sans matérialiser les `Chunk`, et marque explicitement le fichier `indexed_degraded` avec la raison `degraded_structure_only`.
Les outils MCP et le cockpit Phoenix rendent maintenant cette dégradation explicitement:
- `axon_query`, `axon_inspect`, `axon_impact`, `axon_audit` et `axon_health` annoncent une `verite partielle` quand le scope demandé contient des fichiers `indexed_degraded`
- `indexed_degraded` est compté comme succès dégradé dans le cockpit, pas comme erreur
- `Progress` ne garde plus d'overlay mutable local; la progression affichée reste dérivée de SQL
- `GraphProjection` inclut maintenant `CALLS_NIF`
- les tombstones et réindexations invalident désormais les projections, états de projection et embeddings graphe dépendants
- `axon_query` scope désormais les recherches projet sur `project_slug`, pas sur une sous-chaîne de chemin
- `axon_query` réutilise maintenant le worker sémantique isolé pour les embeddings de requête quand la pression service reste `healthy` ou `recovering`
- sous pression `degraded` ou `critical`, `axon_query` retombe explicitement en mode structurel au lieu de bloquer ou d’inventer une similarité sémantique
- la télémétrie cockpit en tests est maintenant remise à zéro explicitement entre cas pour éviter les faux négatifs liés à l’état ETS partagé
Le cockpit Phoenix ne dépend plus d’une double télémétrie Elixir: `BridgeClient` est l’unique ingress read-only, `RuntimeTelemetry` transporte aussi les signaux hôte, et `TelemetryHandler`, `PoolFacade`, `BackpressureController` et `ResourceMonitor` ont disparu du chemin actif.

## Livraison

Le plan maître de livraison est maintenant fermé au sens du contrat courant:

- Rust reste l’unique autorité canonique de runtime et d’ingestion
- Phoenix reste strictement read-only pour le cockpit
- les gros fichiers sont gérés par budget, dégradation contrôlée et refus explicites
- la retrieval développeur est livrée sous un contrat honnête:
  - hybride si la capacité sémantique temps réel est disponible
  - structurelle explicite si la pression runtime impose la prudence
- les couches dérivées restent subordonnées à la vérité structurelle
- les docs canoniques sont réalignées sur cette réalité

La chaîne legacy suivante a déjà été retirée du dashboard:

- `Axon.Watcher.Server`
- `Axon.Watcher.Staging`
- `Axon.Watcher.PathPolicy`
- `Axon.Watcher.IndexingWorker`
- `Axon.Watcher.BatchDispatch`
- configuration `Oban` d’indexation
- API Elixir de lot `PoolFacade.parse_batch/1` et `PoolFacade.pull_pending/1`
- façade SQL `PoolFacade.query_json/1`
- `Axon.Watcher.TrafficGuardian`
- `Axon.Watcher.PoolFacade`
- `Axon.BackpressureController`
- `Axon.ResourceMonitor`
- `AxonDashboard.TelemetryHandler`
- modules morts `AxonDashboardWeb.StatusLive`, `Axon.Watcher.StatsCache`, `Axon.Watcher.PoolEventHandler`, `Axon.Watcher.Auditor`, `Axon.Watcher.Tracking`, `Axon.Watcher.IndexedProject` et `Axon.Watcher.IndexedFile`

Les travaux ultérieurs relèvent désormais d’améliorations produit, pas de blockers de livraison.

## Comment lire le repo sans se tromper

- lire `README.md` et `docs/getting-started.md` avant toute autre doc
- traiter `docs/archive/` comme historique
- traiter les anciens récits `KuzuDB`, Triple-Pod, HydraDB ou `v1/v2` comme contexte de migration, pas comme contrat courant
- traiter `.devenv` transitoire, `src/axon-core/target/`, `src/dashboard/priv/native/*.so` et `.codex` comme artefacts locaux ignorés, pas comme source canonique
