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
- Le dépôt a depuis été nettoyé.
- Les artefacts locaux `.devenv/*`, `src/axon-core/target/`, `src/dashboard/priv/native/*.so` et `.codex` sont ignorés.
- Le worktree n’est plus censé dériver pour de simples raisons d’environnement ou de build.

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

Elle révèle un système exécutable et testablement sain, mais dont la migration Rust-first reste incomplète sur quelques surfaces dashboard et sur le durcissement final retrieval/runtime.

# Residual Migration Debt

La dette de migration réellement active n’est plus diffuse.
Après suppression de la chaîne legacy de contrôle `Server/Staging/PathPolicy/Oban/IndexingWorker/BatchDispatch`
et des modules read-side morts (`Tracking`, `StatsCache`, `Auditor`, `PoolEventHandler`, `StatusLive`, `IndexedProject`, `IndexedFile`),
elle est maintenant concentrée dans les reliquats suivants:

- `Axon.Watcher.PoolFacade`
- `Axon.BackpressureController`
- les gates retrieval / impact / audit qui restent à pousser jusqu’au niveau livraison

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

# Update 2026-04-01 Dead Legacy Dashboard Modules Slice

Une septième tranche a maintenant été validée côté dashboard:

- `AxonDashboardWeb.StatusLive` a été retiré du code compilé
- `Axon.Watcher.StatsCache` a été retiré du code compilé
- `Axon.Watcher.PoolEventHandler` a été retiré du code compilé
- `Axon.Watcher.Auditor` a été retiré du code compilé
- `Axon.Watcher.Tracking` a été retiré du code compilé
- `Axon.Watcher.IndexedProject` a été retiré du code compilé
- `Axon.Watcher.IndexedFile` a été retiré du code compilé
- le commentaire résiduel de `BridgeClient` ne présuppose plus l’existence d’un auditor legacy
- la frontière de tests dashboard prouve désormais explicitement que ces modules ne sont plus chargés

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/dashboard && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null && mix test test/axon_dashboard/legacy_control_plane_boundary_test.exs'` -> `4` tests verts
- `devenv shell -- bash -lc 'cd src/dashboard && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null && mix test'` -> `40` tests verts
- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `151` tests verts (`109` lib + `42` bin)
- `bash scripts/start-v2.sh` -> vert
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- la dette read-side visible n’est plus polluée par des modules morts simplement encore compilés
- le prochain reliquat legacy rationnel est désormais `PoolFacade`, puis la clarification du rôle exact de `BackpressureController`

# Update 2026-04-01 PoolFacade Narrowing Slice

Une huitième tranche a maintenant été validée côté dashboard:

- `Axon.Watcher.PoolFacade` n’expose plus `query_json/1`
- `Axon.Watcher.Progress` lit désormais `Axon.Watcher.SqlGateway` directement
- `Axon.BackpressureController` n’expose plus `get_chunk_size/1`, reliquat de guidage legacy qui n’avait plus de consommateur réel
- la frontière dashboard interdit maintenant explicitement:
  - `PoolFacade.parse_batch/1`
  - `PoolFacade.pull_pending/1`
  - `PoolFacade.query_json/1`
  - `BackpressureController.get_chunk_size/1`

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test test/axon_dashboard/legacy_control_plane_boundary_test.exs'` -> `5` tests verts
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `40` tests verts
- `bash scripts/start-v2.sh` -> vert
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- `PoolFacade` est maintenant plus proche d’un bridge télémétrie/scan que d’une façade applicative générale

# Update 2026-04-01 Structure-Only Degradation Slice

Une neuvième tranche a maintenant été validée côté runtime Rust:

- l’admission canonique peut désormais choisir `ProcessingMode::StructureOnly` avant un refus `oversized_for_current_budget`
- ce choix n’est possible qu’après la probation déjà existante pour un candidat froid
- le worker Rust ne retient plus le contenu complet d’un fichier en mode `structure_only`
- le writer persiste toujours la vérité structurelle (`Symbol`, `CONTAINS`, relations), mais n’écrit pas de `Chunk` dans ce mode
- le statut persistant devient explicitement `indexed_degraded`
- la raison persistante est `degraded_structure_only`

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `156` tests verts (`112` lib + `44` bin)
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `40` tests verts
- `bash scripts/start-v2.sh` -> vert
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- Axon a maintenant un vrai chemin `degradation-before-refusal`, pas seulement une probation avant `oversized`
- les gros fichiers qui ne tiennent plus en `full` mais tiennent encore en `structure_only` continuent à produire une vérité utile au lieu de sortir du pipeline
- `BackpressureController` reste un moniteur read-only, mais a perdu un reliquat d’autorité de sizing qui ne reflétait plus la réalité Rust-first
- le prochain bloc rationnel reste le resserrement ou renommage final des surfaces read-side restantes, puis les gates retrieval / impact / audit orientées usage développeur

# Update 2026-04-01 Repo Hygiene Slice

Une neuvième tranche a maintenant été validée sur l’hygiène du dépôt:

- `.gitignore` couvre désormais explicitement:
  - `.devenv` transitoire (`nix-eval-cache`, `tasks.db`, `profile`, `run`, `shell-*`)
  - `src/axon-core/target/`
  - `src/dashboard/priv/native/*.so`
  - `.codex`
- les artefacts historiquement suivis par erreur ont été retirés de l’index Git sans suppression locale:
  - caches `.devenv`
  - artefacts `src/axon-core/target/`
  - binaire natif `libaxon_scanner.so`
- les modules morts déjà exclus par les tests ont aussi été effectivement retirés du tree:
  - `AxonDashboardWeb.StatusLive`
  - `Axon.Watcher.StatsCache`
  - `Axon.Watcher.PoolEventHandler`
  - test legacy associé

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `40` tests verts
- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `151` tests verts
- `bash scripts/start-v2.sh` -> vert
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- `git status` cesse d’être pollué par les artefacts de build/runtime les plus bruyants
- ce qui reste visible côté code reflète bien mieux le vrai chantier encore ouvert

# Update 2026-04-01 Structure-Only Degradation Slice

Une dixième tranche a maintenant été validée sur le scheduler et le writer Rust:

- `QueueStore` distingue désormais `ProcessingMode::Full` et `ProcessingMode::StructureOnly`
- un candidat qui ne tient plus dans l’enveloppe `full` mais tient encore dans l’enveloppe `structure_only` est admis en mode dégradé après sa probation, au lieu d’être basculé directement en `oversized_for_current_budget`
- `WorkerPool` n’envoie plus le contenu complet au writer quand une tâche passe en `StructureOnly`
- `GraphStore::insert_file_data_batch` persiste alors la vérité structurelle sans matérialiser les `Chunk`
- le statut de fichier résultant devient explicitement `indexed_degraded`
- la raison canonique persistée est `degraded_structure_only`

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `156` tests verts (`112` lib + `44` bin)
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `40` tests verts
- `bash scripts/start-v2.sh` -> vert
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- Axon a maintenant un vrai chemin `degradation-before-refusal`, pas seulement une probation avant `oversized`
- la qualité d’ingestion baisse de façon explicite et traçable avant le refus final
- la prochaine tranche rationnelle du plan maître peut se concentrer sur les gates retrieval/impact/audit et sur les derniers reliquats read-side Elixir

# Files Updated During Reprise

- `/home/dstadel/projects/axon/task_plan.md`
- `/home/dstadel/projects/axon/findings.md`
- `/home/dstadel/projects/axon/progress.md`
- `/home/dstadel/projects/axon/docs/working-notes/2026-04-01-reprise-handoff.md`
