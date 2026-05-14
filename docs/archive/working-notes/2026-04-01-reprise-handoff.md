---
title: Reprise Handoff
date: 2026-04-01
branch: feat/rust-first-control-plane
status: delivery-validated
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
  - résultat courant: `169 passed, 0 failed`
- Dashboard:
  - `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
  - résultat courant: `31 passed, 0 failed`
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

Elle révèle un système exécutable et testablement sain. Les dernières tranches ont ensuite fermé la migration Rust-first jusqu’au contrat de livraison courant.

# Delivery Finding

La dette de migration qui restait concentrée sur retrieval/runtime est maintenant fermée au niveau livraison:

- `axon_query` ne charge plus un modèle jetable par requête; il réutilise le worker sémantique Rust isolé déjà chargé
- la capacité sémantique temps réel reste gardée par `ServicePressure`
- sous pression, Axon retombe explicitement en mode structurel au lieu de forcer une voie coûteuse ou de mentir sur la similarité
- les tests dashboard ne dépendent plus d’un état ETS sale entre cas

Le prochain travail ne part plus d’une dette de migration bloquante, mais d’améliorations produit futures.

# Update 2026-04-01 Delivery Closure Slice

Une tranche finale de fermeture a maintenant été validée:

- `batch_embed` réutilise le worker sémantique Rust via un canal interne au lieu de ré-instancier le modèle à chaque requête
- ce chemin est couvert par des tests dédiés de round-trip et de déconnexion worker
- `Axon.Watcher.Telemetry` expose maintenant un `reset!` explicite, utilisé par les tests cockpit pour éliminer les faux négatifs liés à l’état ETS partagé
- `axon_query` reste honnête en runtime réel:
  - mode `hybride (structure + similarite semantique)` si le worker sémantique est prêt et si la pression service l’autorise
  - mode `structurel (embedding temps reel indisponible)` sinon

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `173` tests verts
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `31` tests verts
- `bash scripts/start-v2.sh` -> vert
- `curl -sS -X POST http://127.0.0.1:44129/mcp ... axon_query` -> réponse valide en runtime réel
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- la dernière gate retrieval utile au quotidien est fermée sous un contrat prudent et explicite
- le plan maître peut être considéré livré dans son périmètre courant

# Update 2026-04-01 Retrieval Truthfulness and Derived-Layer Slice

Une tranche supplémentaire a maintenant été validée sur la fidélité des réponses développeur et du cockpit:

- `axon_audit`, `axon_health`, `axon_impact` et `axon_query` respectent maintenant explicitement le `project_code` demandé, sans dépendre d'une sous-chaîne de chemin
- `axon_inspect` respecte aussi le `project_code` pour des symboles homonymes entre projets
- quand un scope contient des fichiers `indexed_degraded`, les outils MCP exposent maintenant une bannière de `verite partielle` au lieu de présenter silencieusement une réponse complète
- `indexed_degraded` est maintenant rendu comme succès dégradé dans le cockpit Phoenix, pas comme erreur
- `Axon.Watcher.Progress` ne maintient plus d'overlay mutable local; la progression dashboard reste issue de SQL seulement
- `GraphProjection` inclut désormais les arêtes `CALLS_NIF`
- les tombstones et réindexations invalidisent maintenant les `GraphProjection`, `GraphProjectionState` et `GraphEmbedding` dépendants, y compris les ancres voisines qui pointaient vers des symboles supprimés

Validation fraîche:

- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'` -> `169` tests verts
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'` -> `31` tests verts
- `bash scripts/start-v2.sh` -> vert jusqu'aux vérifications internes `Dashboard`, `SQL` et `MCP`
- `bash scripts/stop-v2.sh` -> vert

Conséquence:

- la frontière read-only Phoenix est plus stricte qu'avant
- les couches dérivées sont plus honnêtes et plus cohérentes avec la vérité structurelle
- le résiduel de livraison se concentre encore davantage sur le niveau d'utilité retrieval/semantics, pas sur la sûreté runtime ou la frontière d'autorité

# Recommended Next Step

Le prochain cycle n’est plus une reprise de livraison. C’est un cycle produit optionnel, par exemple:

1. enrichir la retrieval sémantique au-delà des symboles
2. exposer un état explicite de disponibilité sémantique au cockpit
3. renforcer encore l’ergonomie développeur des outils MCP

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
- cette tranche a depuis été absorbée par la fermeture complète de la livraison

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
- cette tranche a depuis été absorbée par la fermeture complète de la livraison

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
- cette tranche a depuis été absorbée par la fermeture complète de la livraison

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

# Files Updated During Reprise

- `/home/dstadel/projects/axon/task_plan.md`
- `/home/dstadel/projects/axon/findings.md`
- `/home/dstadel/projects/axon/progress.md`
- `/home/dstadel/projects/axon/docs/working-notes/2026-04-01-reprise-handoff.md`
