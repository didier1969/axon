---
title: Reprise Handoff
date: 2026-04-01
branch: feat/rust-first-control-plane
status: reprise-validated
---

# Scope

Ce handoff capture la réalité minimale nécessaire pour reprendre Axon sans dépendre de la mémoire de session.

# What Was Verified

## Environment truth

- Le shell courant hors `devenv shell` n'est pas fiable pour diagnostiquer Axon.
- `devenv shell -- bash -lc './scripts/validate-devenv.sh'` passe correctement.

## Git truth

- Branche active: `feat/rust-first-control-plane`
- Le worktree est très sale, mais le diff non staged visible au moment de la reprise est dominé par:
  - `.devenv/*`
  - `src/axon-core/target/release/axon-core.d`
  - `src/dashboard/priv/native/libaxon_scanner.so`
- Aucun changement staged au moment du contrôle.

## Executable truth

- Rust core:
  - `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'`
  - résultat: `146 passed, 0 failed`
- Dashboard:
  - `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
  - résultat: `38 passed, 0 failed`
- Runtime canonique:
  - `bash scripts/start-v2.sh`
  - dashboard prêt
  - SQL prêt
  - MCP prêt
- Probes directes après démarrage:
  - `/sql` expose les tables attendues (`File`, `Symbol`, `RuntimeMetadata`, `Chunk`, `GraphProjection`, ...)
  - `/mcp` expose les outils Axon attendus
- Runtime ensuite arrêté proprement par `bash scripts/stop-v2.sh`

# Dominant Finding

La reprise ne révèle pas un système cassé.

Elle révèle un système exécutable et testablement sain, mais dont la migration Rust-first reste incomplète dans le code Elixir du dashboard.

# Residual Migration Debt

La dette de migration réellement active n’est plus diffuse.
Après suppression de la chaîne legacy de contrôle `Server/Staging/PathPolicy/Oban/IndexingWorker/BatchDispatch`,
elle est maintenant concentrée dans les reliquats read-side suivants:

- `Axon.Watcher.Tracking`
- `Axon.Watcher.StatsCache`
- `Axon.Watcher.Auditor`
- `Axon.Watcher.PoolFacade`
- `Axon.Watcher.PoolEventHandler.process_pending/1`
- `Axon.BackpressureController`
- `Axon.Watcher.TrafficGuardian`

Le prochain travail doit partir de cette dette réelle, pas d'un récit de migration déjà finie.

# Recommended Next Step

Exécuter la tranche "de-authorize remaining Elixir ingestion authority" de façon prouvable:

1. écrire ou compléter les tests de frontière côté dashboard
2. exposer au dashboard les métriques Rust de budget/réservations/exhaustion/oversized
3. réduire `BackpressureController`, `TrafficGuardian` et le pont `PoolFacade` à de l'affichage/telemetry only
4. réaligner `STATE.md` et les handoffs pour distinguer clairement:
   - stabilité prouvée
   - migration encore ouverte

# Update 2026-04-01 Memory Scheduler Slice

Une première tranche concrète de cette dé-authorisation est désormais engagée:

- `Axon.Watcher.Server` ne classe plus les gros fichiers vers `indexing_titan`
- `Axon.Watcher.IndexingWorker` ne transmet plus une sémantique de lane canonique à Rust
- `QueueStore` côté Rust réserve désormais un budget mémoire en vol par fichier admis
- le runtime Rust ralentit ou suspend les claims non seulement sur RSS/pression service, mais aussi sur le taux d'épuisement de ce budget
- le worker Rust ne skippe plus un fichier uniquement parce qu'il dépasse `1MB`; l'admission repose désormais sur le coût estimé taille/extension et sur le budget réellement disponible

Conséquence: la protection des vagues de gros fichiers commence désormais dans le runtime Rust, pas dans un détour de classification Elixir.

# Update 2026-04-01 Dynamic Admission and Titan Removal

Une seconde tranche a maintenant été validée dans le runtime Rust:

- `TaskLane::Titan` a disparu du runtime canonique
- la queue Rust est désormais organisée en `hot + common`, avec budget mémoire comme seule règle canonique d’admission
- l’estimation de coût démarre de façon prudente puis se détend par `parser class + size bucket + confiance observée`
- l’ingestor Rust choisit désormais un lot packable de candidats sous budget au lieu de dépendre d’un ordre FIFO naïf
- un fichier trop gros même seul est marqué explicitement `oversized_for_current_budget`
- le throttling Rust combine maintenant les pressions `queue + budget + RSS + service` pour produire une cadence progressive, au lieu de dépendre uniquement de paliers fixes

Conséquence:

- le concept `Titan` n’est plus un contrat d’ingestion valide pour Axon
- le reliquat structurel suivant à supprimer est clairement côté Elixir, pas côté runtime Rust

# Update 2026-04-01 Elixir Control-Plane Removal Slice

Une troisième tranche a maintenant été validée côté dashboard:

- `Axon.Watcher.Server`, `Axon.Watcher.Staging`, `Axon.Watcher.PathPolicy`, `Axon.Watcher.IndexingWorker` et `Axon.Watcher.BatchDispatch` ont été retirés du tree actif
- la configuration `Oban` d’ingestion legacy a disparu du dashboard
- `Axon.Watcher.PoolFacade` n’expose plus `parse_batch/1` ni `pull_pending/1`
- `Axon.Watcher.PoolProtocol` ne garde plus de sémantique d’ack batch legacy
- le pont Elixir restant sert le scan explicite, la télémétrie entrante et les requêtes SQL, pas l’admission canonique
- la validation fraîche couvre désormais:
  - `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `38` tests verts
  - `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `147` tests verts
  - `bash scripts/start-v2.sh` puis `bash scripts/stop-v2.sh` -> verts

Conséquence:

- la dette critique n’est plus la chaîne de dispatch legacy
- la prochaine tranche rationnelle est l’exposition cockpit des métriques Rust et la réduction des reliquats read-side Elixir

# Update 2026-04-01 Rust Runtime Telemetry and Fairness Slice

Une quatrième tranche a maintenant été validée entre Rust et Phoenix:

- le runtime Rust émet périodiquement `RuntimeTelemetry` sur le bridge
- le payload exporte désormais:
  - `budget_bytes`
  - `reserved_bytes`
  - `exhaustion_ratio`
  - `queue_depth`
  - `claim_mode`
  - `service_pressure`
  - `oversized_refusals_total`
  - `degraded_mode_entries_total`
- le cockpit racine Phoenix affiche ces métriques en lecture seule
- `PoolFacade` reflète aussi `RuntimeTelemetry` dans `Axon.Watcher.Telemetry`, sans recréer d’autorité de scheduling côté Elixir
- les fichiers `pending` accumulent maintenant une dette de fairness persistante (`defer_count`, `last_deferred_at_ms`) lorsque le scheduler Rust les diffère
- une claim effective remet cette dette à zéro, ce qui permet à un gros fichier durablement repoussé d’être finalement promu sans casser le packing par défaut
- un fichier `oversized` froid n’est pas classé trop tôt comme refus définitif: le scheduler lui laisse d’abord une probation de quelques reports avant de le basculer en `oversized_for_current_budget`

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `38` tests verts
- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `151` tests verts (`109` lib + `42` bin)
- `bash scripts/start-v2.sh` -> vert après durcissement du lancement Phoenix pour exécuter `mix local.hex --force` et `mix local.rebar --force` aussi dans le shell tmux réel
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- le cockpit principal commence à refléter la vérité Rust au lieu d’un proxy Elixir heuristique
- la fairness n’est plus un TODO théorique mais une propriété persistante du scheduler Rust
- la prochaine tranche rationnelle est la dégradation avant refus final au-delà de cette probation, puis la réduction des reliquats read-side (`Tracking`, `StatsCache`, `Auditor`, `PoolFacade`)

# Update 2026-04-01 Dashboard Read-Side Reduction Slice

Une cinquième tranche a maintenant été validée côté dashboard:

- `Axon.Watcher.StatsCache` n’est plus supervisé sur le chemin actif
- `Axon.Watcher.PoolFacade` écrit directement dans `Axon.Watcher.Telemetry` pour `FileIndexed` et n’utilise plus `StatsCache` comme agrégateur parallèle
- la preuve UI côté tests couvre maintenant explicitement que:
  - `StatsCache` n’est plus un child actif du supervisor
  - un `FileIndexed` reçu sur le bridge hydrate bien `Telemetry` directement

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `39` tests verts
- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `151` tests verts (`109` lib + `42` bin)
- `bash scripts/start-v2.sh` -> vert
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- le cockpit actif dépend encore moins d’un read-side Elixir parallèle
- la dette read-side restante est désormais plus concentrée dans `Tracking`, `Auditor`, les restes morts comme `StatusLive`, et l’étroitesse encore insuffisante de `PoolFacade`

# Update 2026-04-01 Cockpit Host Pressure Slice

Une sixième tranche a maintenant été validée côté cockpit actif:

- `Axon.Watcher.Telemetry` persiste aussi les signaux de pression hôte reçus via télémétrie Elixir:
  - `cpu_load`
  - `ram_load`
  - `io_wait`
  - `queues_paused`
  - `indexing_limit`
- `Axon.Watcher.CockpitLive` n’ignore plus totalement les événements `:axon, :backpressure, ...`:
  - `pressure_computed`
  - `queues_paused`
  - `queues_resumed`
  - `limit_adjusted`
- le cockpit racine affiche désormais en lecture seule:
  - `HOST_CPU`
  - `HOST_RAM`
  - `HOST_IO_WAIT`
  - `HOST_STATE`
  - `HOST_GUIDANCE`
- cette visibilité reste read-side uniquement: Elixir reflète la contrainte hôte, mais ne redevient pas plan de contrôle canonique

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `40` tests verts
- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `151` tests verts (`109` lib + `42` bin)
- `bash scripts/start-v2.sh` -> vert
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- le cockpit principal montre maintenant la pression hôte utile à l’opérateur, pas seulement l’état interne du scheduler Rust
- `BackpressureController` devient plus défendable comme source de télémétrie read-side tant qu’il n’a pas d’autorité canonique sur l’ingestion
- la prochaine tranche rationnelle reste la réduction des reliquats morts ou trop larges (`StatusLive`, `StatsCache`, `PoolEventHandler`, `Tracking`, `Auditor`, puis resserrement de `PoolFacade`)

# Update 2026-04-01 Cockpit Host-Pressure Slice

Une sixième tranche a maintenant été validée sur le cockpit actif:

- `Axon.Watcher.Telemetry` persiste maintenant aussi la pression hôte observée:
  - `cpu_load`
  - `ram_load`
  - `io_wait`
  - `queues_paused`
  - `indexing_limit`
- `Axon.Watcher.CockpitLive` consomme directement les événements `[:axon, :backpressure, ...]` pertinents et affiche désormais:
  - `HOST_CPU`
  - `HOST_RAM`
  - `HOST_IO_WAIT`
  - `HOST_STATE`
  - `HOST_GUIDANCE`
- le cockpit actif continue à rester read-only: il reflète la contrainte hôte observée, mais ne recrée pas de logique canonique d’admission côté Elixir

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `40` tests verts
- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `151` tests verts (`109` lib + `42` bin)
- `bash scripts/start-v2.sh` -> vert
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- le cockpit principal montre maintenant la pression hôte utile à l’opérateur, au lieu de n’exposer que les signaux internes Rust
- la prochaine tranche rationnelle reste la suppression des reliquats morts/read-side (`StatusLive`, `StatsCache`, `PoolEventHandler`, puis `Tracking`/`Auditor` selon preuve d’usage)

# Files Updated During Reprise

- `/home/dstadel/projects/axon/task_plan.md`
- `/home/dstadel/projects/axon/findings.md`
- `/home/dstadel/projects/axon/progress.md`
- `/home/dstadel/projects/axon/docs/working-notes/2026-04-01-reprise-handoff.md`
