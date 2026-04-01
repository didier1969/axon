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
  - résultat: `35 passed, 0 failed`
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
Elle est concentrée dans la chaîne suivante:

- `Axon.Watcher.Server`
- `Axon.Watcher.Staging`
- queues `Oban` d’indexation legacy
- `Axon.Watcher.IndexingWorker`
- `Axon.Watcher.PoolFacade.parse_batch`
- `Axon.Watcher.Tracking`
- `Axon.BackpressureController`
- `Axon.Watcher.TrafficGuardian`

Le prochain travail doit partir de cette dette réelle, pas d'un récit de migration déjà finie.

# Recommended Next Step

Exécuter la tranche "de-authorize remaining Elixir ingestion authority" de façon prouvable:

1. écrire ou compléter les tests de frontière côté dashboard
2. réduire `BackpressureController` à de l'affichage/telemetry only
3. neutraliser les chemins où `Watcher.Server`, `BatchDispatch`, `Staging`, `IndexingWorker` ou `PoolFacade` gardent une autorité canonique
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

# Files Updated During Reprise

- `/home/dstadel/projects/axon/task_plan.md`
- `/home/dstadel/projects/axon/findings.md`
- `/home/dstadel/projects/axon/progress.md`
- `/home/dstadel/projects/axon/docs/working-notes/2026-04-01-reprise-handoff.md`
