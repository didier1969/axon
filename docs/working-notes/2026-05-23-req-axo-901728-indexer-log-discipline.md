---
status: draft (awaiting MCP-up to write to SOLL via soll_manager)
type: requirement
proposed_id: REQ-AXO-901728
links:
  parent: PIL-AXO-9006 (observability / operational hygiene — to verify on SOLL probe)
  refines: CPT-AXO-029 (IST freshness gate — tangential)
tags: [simplifiable, robustness, deliverability, observability]
originator: forensic diagnosis 2026-05-23 SSD saturation post-mortem (session 54)
---

# REQ-AXO-901728 — Indexer log retention discipline

## Problème observé (2026-05-23)
Saturation SSD WSL ayant rendu le système irresponsif. Forensique post-reboot :
- `.axon/run-indexer` = 4.3 Go cumulés (fichiers individuels jusqu'à **2 Go**)
- `.axon-dev/run-indexer` = 842 Mo
- Pas de rotation : un fichier `axon-indexer.log.2026-05-13` a grossi à 2 Go en ~24h
- 99% du volume = `INFO axon_core::watcher_probe: WatcherProbe checkpoint=watcher.{received,buffered_subtree_hint,control_file,buffered_batch}` (per-évènement inotify, logué à INFO alors qu'il devrait être DEBUG/TRACE)
- Memory pressure warnings (`Memory reclaimer shed N subtree hint(s)`) noyés sous le bruit per-évènement

## Spécification rétention (opérateur, 2026-05-23)
| Sévérité | Fenêtre conservée | Sink |
|---|---|---|
| TRACE / DEBUG | rien (filtré) | — |
| **INFO** | **dernières 20 minutes** rolling | fichier `info.log` rotaté par fenêtres de ~5 min, max 4 fichiers (= 20 min total) |
| **WARN / ERROR** | **dernières 24 heures** rolling | fichier `errors.log` rotaté horaire, max 24 fichiers |
| Au-delà | purge automatique | — |

## Implémentation cible (tracing-subscriber Rust)
```rust
// src/axon-core/src/observability/tracing_setup.rs (à créer)
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{filter, prelude::*};

let info_appender = RollingFileAppender::builder()
    .rotation(Rotation::MINUTELY)  // tracing-appender minimum = MINUTELY ;
                                   //   wrapper custom pour fenêtres de 5 min
    .filename_prefix("info")
    .filename_suffix("log")
    .max_log_files(4)              // ≈ 20 min de rétention
    .build(&run_dir)?;

let error_appender = RollingFileAppender::builder()
    .rotation(Rotation::HOURLY)
    .filename_prefix("errors")
    .filename_suffix("log")
    .max_log_files(24)
    .build(&run_dir)?;

let info_layer = tracing_subscriber::fmt::layer()
    .with_writer(info_appender)
    .with_filter(filter::LevelFilter::INFO)
    .with_filter(filter::filter_fn(|md| md.level() <= &tracing::Level::INFO));

let error_layer = tracing_subscriber::fmt::layer()
    .with_writer(error_appender)
    .with_filter(filter::LevelFilter::WARN);  // WARN + ERROR

tracing_subscriber::registry()
    .with(info_layer)
    .with(error_layer)
    .init();
```

Points d'attention :
1. `tracing-appender::rolling::Rotation` ne supporte pas nativement `EVERY_5_MINUTES`. Soit on accepte la granularité MINUTELY (= max 20 fichiers de 1 min), soit on implémente un `RollingFileAppender` custom pour des fenêtres de 5 min.
2. Le filtre `watcher_probe = WARN` (ou `OFF`) à ajouter dans `RUST_LOG`/EnvFilter pour réduire à la source le bruit per-évènement avant même la rotation. Cela évite d'écrire pour ensuite jeter.
3. Vérifier que `tracing_appender` flushe sur shutdown (sinon perte des derniers évènements).

## Démotion préalable (gain ~95% immédiat)
Avant la rotation, démotion des probes per-évènement :
```rust
// src/axon-core/src/watcher_probe.rs — emit! macros
- info!(target: "watcher_probe", ...)
+ debug!(target: "watcher_probe", ...)  // ou trace! pour très-haute fréquence
```
Conserver à `info!` uniquement les agrégats : `watcher.buffered_batch` avec compteur, `watcher.received` avec total/min.

## Acceptance criteria
- [ ] Aucun fichier log dans `run-{brain,indexer}/` ne dépasse 100 Mo individuellement (sanity bound)
- [ ] Volume total `run-{brain,indexer}/` < 500 Mo en steady-state
- [ ] Recherche d'un évènement WARN/ERROR sur 24h reste possible (sans grep sur des Go)
- [ ] Aucune ligne `watcher_probe` à `INFO` en logs steady-state
- [ ] Test bench pipeline-v2 : volume log < 50 Mo pour run de 80 fichiers
- [ ] Diagnostic memory_pressure (`Memory reclaimer shed N`) reste visible dans `errors.log`

## Impact (à valider via `impact` MCP)
- Surface code : `src/axon-core/src/main_background.rs`, `src/axon-core/src/watcher_probe.rs`, point d'init tracing (probablement `src/axon-core/src/bin/axon-indexer.rs` + `axon-brain.rs`)
- Pas de rupture API
- Risque régression : si watcher_probe DEBUG demandé pour forensique → activable via `RUST_LOG=axon_core::watcher_probe=debug` à la demande

## Prochaines étapes
1. Redémarrer brain-only (sans indexer pour éviter récidive)
2. Loguer ce REQ via `soll_manager(action=create, type=requirement, id=REQ-AXO-901728, ...)`
3. Linker à PIL/CPT pertinents via `soll_manager(action=link)`
4. Implémenter (PR distincte ; estimé ~2-3h dev + test)
5. Smoke test : démarrer indexer 10 min, vérifier rotation + volume

## Originator
LLM (forensic post-mortem session 54, 2026-05-23, refined par opérateur :
"20 minutes d'INFO + 24h d'ERROR largement suffisantes").
