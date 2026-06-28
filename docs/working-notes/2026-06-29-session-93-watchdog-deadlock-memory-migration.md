# Session 93 — 2026-06-29 — Watchdog OOM, deadlock fix, migration mémoire (audit-only)

> Append-only. NE remplace PAS le session_pointer (CPT-AXO-052) ni la SOLL. Référencé par CPT-AXO-052.

## Contexte d'entrée
Opérateur signale 3 `wsl --shutdown` successifs (crash/gel récurrent). Soupçon : une modif récente cause un OOM.

## Diagnostic crash (vérité-sol kern.log)
- VM WSL2 (cap 32 Go) saturée en mémoire anonyme → tempête `oom-killer` global (21:56-22:33, puis 00:05). 3 reboots (boots 23:14, 23:40, 00:07).
- Hog = **28× `python3.11` = 19-26 Go = opencog-v2** (Prefect + moteur fovéal). **Axon = 3,4 Go, innocent** (brain 1,2-1,7 + indexer 1,5-2,4). python3.11 ≠ Axon (Axon=python3.12 devenv).
- Cause code opencog : REQ-OPV-398 Levier B (world-model en RAM) × Levier C (parallélisation loky par date) → chaque worker réplique ~660 Mo (univers 558 firmes, pas les 67 Mo estimés sur 8) → N×660 Mo = OOM. Fix opencog `memory_safe_n_jobs` adaptatif (commit 3c380cbe, NON poussé, géré par l'équipe opencog).
- Méthode diag consignée : `reference_wsl2_oom_diagnostic_kernlog` + practice id 86.

## Livré Axon (build live v0.8.0-1280-g99b5a35b, poussé)
- **902149** promu (partitionnement multi-agent practice_* role/model) — live était en retard, expliquait la staleness role/model côté clients.
- **902152** (watchdog OOM, scope léger) : watchdog rendu ACTIF + VM-aggregate-aware (MemAvailable host-wide = coordination cross-process sans IPC) ; backoff intake A1 ; reclaimer sous pression. Fix C deadlock `dashboard_state_full⇄stage_a3` : `SET LOCAL lock_timeout 250ms` < deadlock_timeout → 0× 40P01 (load-test clean_axon_dev validé). 29 tests verts.
- **902146** migration dogfood : ~50 feedbacks universels → practice_* scope '*' + 27 AXO ; doc best-practices-memory.md rafraîchi ; MEMORY.md noté.
- **902153** découvrabilité : CLAUDE.md projet Tool Routing enrichi practice_*/mailbox_*.

## Frictions / résiduels (SOLL)
- **902154** (P2) : write-gate practice_put sur-rejette les directives OPÉRATIONNELLES AXO (NLI confond directive/claim) → workaround cadrage mode-d'échec. Sibling 902132.
- **902155** (P3) : should_fuse dead_code vs IST 1-caller (fusion 902137 peut-être pas pleinement câblée).
- 902153 résiduel : tables tool des skills (gaté writing-skills) + usage_examples_for_tool.
- 902152 AC3 (backpressure) : mécanisme unit-testé, PAS stress-validé (induire OOM imprudent).
- doc best-practices-memory.md : section modèle-de-données encore stale (refresh partiel).

## Décisions méthodo notables
- N'avoir PAS reflexivement promu/load-testé pendant l'incident OOM sans vérifier le headroom (practice id 90).
- Axon restauré indexer_full (REQ-902042 standing), auto-pause GPU dev/live confirmée (DEC-AXO-067).
