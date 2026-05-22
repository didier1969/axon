# Session 51 — TOC discipline + legacy purge slice 1 + drain fix + Wallaby scaffold

**Date** : 2026-05-22
**Branch** : `main`
**Operator** : Didier Stadelmann (autonomous mode, end-to-end)

## Goal

Resume post-session-50 with REQ-AXO-901649 (drain leak) + REQ-AXO-901652 (watcher silent on new files) open as P0 blockers on live indexing. Session 50 left an uncommitted drain-fix in `pipeline_v2_runtime.rs` + Wallaby scaffold ready but undispatched.

## Methodology — TOC applied

Operator challenged the rush to `cargo test pipeline_v2` with : « selon la théorie des contraintes, comment vas-tu débuguer ce pipeline ? » Forced the methodology back to Goldratt 5 steps :

1. **Identifier** la contrainte — observational, not assumed.
2. **Exploiter** existing capacity.
3. **Subordonner** other stages.
4. **Élever** capacity (last resort).
5. **Recommencer** — re-measure post-fix.

Saved as new feedback memory `feedback_toc_discipline_for_pipeline_debug.md`.

## TOC step 1 — measurement reframed the problem

`tmux capture-pane -t axon-indexer:core` revealed continuous spam :

```
[pg_query_count] db error | SELECT count(*) FROM GraphProjectionQueue WHERE status = 'queued'
[pg_query_count] db error | SELECT count(*) FROM FileVectorizationQueue ...
[pg_query_count] db error | SELECT count(*) FROM axon_runtime.VectorPersistOutbox ...
ERROR axon_core::embedder: Semantic Graph Worker [N]: graph projection fetch error: Graph plugin error: query: db error
```

8+ "Semantic Graph Worker" threads in error loops polling DROP'd tables (post-MIL-AXO-017 / REQ-AXO-289). Brain MCP `sql` + `project_status` calls also timed out under PG conn pool contention.

**Conclusion** : real contrainte = legacy dead code (Semantic Graph Worker + queue helpers + File state-machine) drowning logs + saturating PG conn pool. REQ-AXO-901649 drain leak was a downstream symptom, not the dominant bottleneck.

## Operator constraint that resolved scope

> « Ce système n'est pas livré pour l'instant, donc je ne veux rien garder de legacy. On veut le meilleur système au monde. »

Scope = **delete legacy, not env-gate**. Validated SOLL umbrella REQ-AXO-901653.

## Deliveries

### Session 50 closeouts (evidence attach + status delivered)

- REQ-AXO-901635 (`stop.sh --verify` canonical scope) → 6 evidence items (DEC-AXO-901598 + commits 4ba9754c, f994c0b8 + test + 2 files). Status `delivered`.
- REQ-AXO-901648 (Plug.Static gzip:false hard-disable) → 2 evidence items (commit 310e431f + endpoint.ex). Status `delivered`.
- REQ-AXO-901629 (reproducibility fresh-machine umbrella) → 7 evidence items (commit d0513d70 + 6 modified files across REQ-AXO-901640..901644 children). Status `delivered`.

### Session 51 commits

| Commit | REQ | Scope | LOC |
|---|---|---|---|
| `2717359b` | REQ-AXO-901653 Slice 1 | `embedder.rs` — graph_worker_loop spawn + impl + GraphWorkerLivenessGuard + dead imports removal | -153/+19 |
| `cc5c4887` | REQ-AXO-901649 | `pipeline_v2_runtime.rs` — bootstrap try_send + drain hint-completion-first + try_send + 5s heartbeat observability | +100/-16 |
| `381ab2af` | REQ-AXO-901654 | Wallaby E2E scaffold (15 files : devenv.nix chromedriver/chromium + mix deps + 5 feature tests + helpers + runner script + fixtures) | +971/-7 |
| `de9f4160` | (docs) | working-notes sessions 49-50 append-only audit | +157 |

### SOLL umbrella created

REQ-AXO-901653 — Full legacy purge post-MIL-AXO-017 / REQ-AXO-289. 8 slices documented (Slice 1 delivered, 2-8 pending). Tags : legacy-purge, duckdb-residue, pipeline-v2-canonical, session-51.

REQ-AXO-901654 — Wallaby E2E dashboard test suite scaffold. BELONGS_TO PIL-AXO-009. P1.

### Memory updates

- `feedback_toc_discipline_for_pipeline_debug.md` — Goldratt 5 steps applied to pipeline debug.
- `project_legacy_purge_req901653.md` — slice progression tracker.
- `MEMORY.md` index updated with 2 new pointer lines.

## REQ-AXO-901653 slices remaining (multi-session)

| # | Slice | Effort estimate |
|---|---|---|
| 2 | Remove `vector_worker_loop` + maintenance loop (~2000 LOC dead code) | ~1h |
| 3 | Delete `graph_ingestion/{graph_projection_queue,vectorization_queue}.rs` + 18+ callsite cleanups | ~2-3h, multi-file |
| 4 | Delete DDL CREATE/DROP for dropped tables | ~30min |
| 5 | Delete `public.File` state-machine columns ; consolidate on `IndexedFile` 3-col | ~3h, 25+ files |
| 6 | Update MCP tools surface (remove legacy state fields) | ~1-2h |
| 7 | Delete legacy tests (fixes REQ-AXO-901634 pre-existing failure) | ~1h |
| 8 | promote-live + qualify + 1h observation + verify REQ-AXO-901649/901652 residual | session-end |

## Promote-live attempt 1

Failed at release-preflight : tree dirty (Wallaby scaffold uncommitted). Recovery : commit Wallaby (REQ-AXO-901654) + working-notes → tree clean → re-run.

## Promote-live attempt 2 — FAILED (same bug)

Reproduced the tmux send-keys truncation. promote_live.sh --resume hung at same `Waiting for Axon Infrastructure to rise`. tmux pane showed identical truncated `"bin/axon-in` + bash continuation prompt `>`.

## Recovery — ROLLBACK + brain-only attempt + STOP

1. Killed promote --resume subtree (PIDs 22279, 22048, 21500, 21501, 21480).
2. Archived `pending.json` → `pending.aborted-20260522T-session51-tmux-bug.json`.
3. Ran `bash scripts/release/rollback_live.sh --manifest .axon/live-release/current.json` → staged rollback (no `--restart-live` to avoid start.sh tmux bug).
4. Archived new rollback `pending.json` → `pending.aborted-20260522T-session51-recovery-restore-prior.json`.
5. Manually restored bin/ binaries via `cp .axon/releases/artifacts/<sha>/axon-{brain,indexer,ctl} bin/` (sha256 verified against current.json).
6. Cleaned stale `.axon/run-{brain,indexer}/runtime.env`.
7. `./scripts/axon-live start --brain-only` (shorter env-string, avoids tmux bug) → brain process up (PID 31024 supervised) BUT MCP port 44129 never binds. Brain stuck post-`Semantic query Worker [0]: BGE-Large model loaded successfully (provider_effective=cpu).`. start.sh timed out after 120s.
8. **STOP per `feedback_dont_persist_when_breaking`**. `./scripts/axon-live stop --hard` → clean down. Hand Off engaged.

## Operator methodology callout

Operator interrupted recovery with : « Pourquoi avons-nous l'environnement de développement ? »

**Direct violation of `feedback_dev_before_live_testing.md`**. New binaries (Slice 1 + REQ-AXO-901649 + devenv.nix chromedriver/chromium) NEVER tested in dev before promote-live. If dev `./scripts/axon-dev start --indexer-full --tensorrt` had been run first, the tmux send-keys truncation would have manifested in dev (port 44137 isolated, PIL-AXO-004 topology). Live would have stayed UP on v0.8.0-612 during diagnosis.

New feedback memory saved : `feedback_dev_first_no_exception.md` (strengthens parent rule with "no exception, no matter how safe the fix feels").

## TWO new REQs to log next session (brain MCP down, defer logging)

### REQ-1 (P0) — start.sh:973 tmux send-keys 2KB+ truncation

**Symptom** : `promote_live_safe.sh` hangs at `Waiting for Axon Infrastructure to rise`. tmux pane shows truncated env-string + axonctl command, unclosed quote, bash `>` continuation prompt.

**Root cause** : start.sh:973 sends a single 2KB+ string via `tmux send-keys`. The chromedriver+chromium+gcc-14+gcc-15 LD_LIBRARY_PATH extensions (REQ-AXO-901642/901647) pushed cumulative env-string past tmux buffer threshold. Session 50 worked because LD_LIBRARY_PATH was shorter.

**Fix design** : write env+exec to /tmp/axon-launch-<role>-$$.sh, then `tmux send-keys "bash /tmp/launch.sh" C-m` (short string, no truncation). Apply same to start.sh:322,324 (dashboard launch).

**BLOCKER** : LIVE PROMOTE IMPOSSIBLE until fixed.

**Tags** : `axon-bug`, `tmux-bug`, `promote-live-blocker`, `start-sh`, `session-51`, `fix-pending`.

**Attach** : BELONGS_TO PIL-AXO-005 (promote-live discipline).

### REQ-2 (P0) — brain-only mode hangs post-ONNX init, MCP port 44129 never binds

**Symptom** : `./scripts/axon-live start --brain-only` launches brain, brain loads BGE-Large model on CPU, then no further log, MCP port not bound, start.sh times out 120s.

**Hypotheses** : REQ-AXO-91572 (embedder_provider singleton race in brain_only) OR IST RAM warmup hang OR PG connection pool race OR brain waits for indexer signal that never comes.

**Investigation needed** : `RUST_LOG=info,axon_core::runtime_boot=debug,axon_core::mcp=debug,axon_core::ist_ram=debug` startup ; try `AXON_IST_RAM_ENABLED=0` workaround ; compare brain_only vs brain+indexer boot logs.

**Impact** : brain-only mode unusable as recovery fallback after failed promote.

**Tags** : `axon-bug`, `brain-only`, `mcp-no-bind`, `startup-hang`, `session-51`, `related-req-91572`.

**Attach** : BELONGS_TO PIL-AXO-001 (one telemetry-backed truth).

## Final session state

| Item | Status |
|---|---|
| 4 commits livrés en repo `main` | ✅ |
| bin/ binaries | v0.8.0-612 restored (sha256 ✅) |
| current.json | v0.8.0-612 promoted (cohérent) |
| pending.json | archivé propre |
| Live brain + indexer | down (clean stop) |
| Live MCP | DOWN |
| start.sh tmux bug | identified, NOT fixed |
| brain-only hang bug | identified, NOT diagnosed |
| Slice 1 + 901649 deployment | DEFERRED to next session |
| Working notes | this file, to commit |
| 2 P0 REQs | bodies above, to log next session via soll_manager |

## Next session — recommended sequence

1. **Cold-start** GUI-PRO-102 Phase A + read CPT-AXO-052 (session_pointer).
2. **Start dev first** : `./scripts/axon-dev start --brain-only` (port 44137 isolated). Verify dev MCP responds.
3. **Log REQ-1 (start.sh tmux bug) + REQ-2 (brain-only hang)** via `soll_manager` (bodies in this working note).
4. **Fix start.sh:973 tmux send-keys bug** (tmpfile pattern). Test in dev first.
5. **Diagnose brain-only hang** via debug logs in dev (REQ-2 investigation steps).
6. **Once both fixed + dev validated 5+ min** : re-attempt promote-live with Slice 1 + 901649 binaries (commits `2717359b` + `cc5c4887` already in repo).
7. **Continue REQ-AXO-901653 Slices 2-8** legacy purge (multi-session umbrella).


## TOC step 5 acceptance criteria (post-promote)

| Métrique | Pre-Slice-1 | Cible |
|---|---|---|
| `Semantic Graph Worker [N]: graph projection fetch error` per min | ~50 | **0** |
| `[pg_query_count] db error` per min | ~50 | ~5-10 (Slice 3 callsites restants) |
| `pgrep axon-indexer` md5 | `f81e06fafbcd9d00549c3eceb2c2575f` | NEW (post-promote) |
| `indexedfile` growth post-promote | 8233 figé | observation |
| `ingress_subtree_hint_in_flight` | 256 saturé | doit décroître |

## Next session — recommended

1. Read promote-live attempt 2 outcome (`/tmp/session51-promote-2.log`).
2. If green : observe live tmux 30 min, attach evidence to REQ-AXO-901653 Slice 8 (qualify + observation).
3. Continue REQ-AXO-901653 Slice 2 (vector_worker_loop removal — smaller scope than Slice 3).
4. Surface session_pointer CPT-AXO-052 update (Hand Off Step 1).

## Originator

Session 51 driven by REQ-AXO-901652 P0 LIVE INDEXING BLOCKER from session 50 pointer. Methodology shift forced by operator TOC challenge mid-session. Scope evolved from "fix drain leak" to "full legacy purge umbrella" once measurement revealed the dominant contrainte.
