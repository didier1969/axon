# Session 50 — Promote-live transactional + Dashboard rebuild + Watcher diagnostic

Date : 2026-05-21
Active session_pointer : CPT-AXO-052
Umbrella : REQ-AXO-901635 (session 50 hardening umbrella)
HEAD start : `357d6cc9` · HEAD end : `310e431f`
Live build promoted : `v0.8.0-612-ge4e80ad6` (install generation `live-20260521T164329Z`)

## What shipped (10 commits)

| Commit | REQ/DEC | Scope |
|---|---|---|
| `4ba9754c` | DEC-AXO-901598 + REQ-AXO-901636/637/639 | `scripts/stop.sh` canonical/derived TCP port split (PHX_PORT excluded from `--verify`) + binary-anchored process identity helpers (`canonical_axon_processes_alive_pids`, `collect_canonical_listener_pids`) + regression test `tests/shell/test_stop_canonical_scope.sh` |
| `f994c0b8` | REQ-AXO-901637 hardening | `pgrep` / `ss` no-match must not trip `set -euo pipefail` (helpers capture into local + `\|\| true`) |
| `e4e80ad6` | REQ-AXO-901638 | `promote_live.sh` + `rollback_live.sh` poll_until polling (5s/100ms for `assert_live_stopped` ; 150s/2s post-check) + `--resume` flag + bin/* sha256 coherence check + diagnostic visibility (no more `>/dev/null 2>&1`) + recovery menu on failure + `stop.sh` `sleep 0.20` → `epmd -names` poll fallback |
| `3961e53c` | REQ-AXO-901638 followup | `--resume` must skip `preflight --check-pending` |
| `11ec202f` | REQ-AXO-901638 followup | `start-brain.sh` force `AXON_RUNTIME_MODE=brain_only` regardless of inherited env (was hitting wrong tmux session via `${AXON_RUNTIME_MODE:-brain_only}` inheritance from indexer start) |
| `1f75c6c4` | REQ-AXO-901638 followup | `--resume` must reuse pending `install_generation` (regenerated value was breaking post-check matching) |
| `d0513d70` | REQ-AXO-901640/41/42/43/44 (expert subagent) | Fresh-machine reproducibility : `_w5/_w7_runner.sh` paths derived from `$ROOT/src`, `AXON_NVML_LIBRARY_PATH` resolution chain (WSL→x86_64→lib64→ldconfig), `devenv.nix` +12 canonical tools declared (jq, ss, rg, flock, tmux, util-linux, etc.) + `validate-devenv.sh` enforced contract (25 tools), `beamPackages.erlang` explicit, `scripts/setup.sh --dry-run` bootstrap plan |
| `cd42234f` | REQ-AXO-901647 (expert subagent) | Dashboard full rebuild : `PipelineLive` (`/` `/cockpit` 1Hz PubSub) + `ProjectsLive` (`/projects` 5s SQL) + `McpLive` (`/mcp` 30s catalog) + `Nav.shell` glassmorphism, pure SVG `PipelineTopology` hook, mount `IndexerHeartbeat` + `McpPoller` to supervisor (they existed but weren't started), `/legacy/` route preserves old `CockpitLive` |
| `310e431f` | REQ-AXO-901648 | Dashboard `Plug.Static gzip: false` hard-disable + manual cleanup of stale `priv/static/assets/*.gz` + `cache_manifest.json` (root cause of "dashboard not styled" incident — Phoenix preferred 2-week-old gzipped bundles over the live Tailwind/esbuild watcher output) |

## State at session end

- **Brain LIVE** PID 15439, mode `brain_only`, MCP `http://127.0.0.1:44129/mcp` responsive
- **Indexer LIVE** PID 24383, mode `indexer_full --tensorrt`, GPU active (~5.5 cores busy, RSS 7 GB), but indexing FROZEN (see watcher bug below)
- **Dashboard LIVE** http://127.0.0.1:44127/ — 3 routes operational, Tailwind glassmorphism, real-time SVG pipeline topology
- **PG** healthy (5 idle + 1 active connections on axon_live, max 100)

## Open bug — REQ-AXO-901652 (P0, fix-pending, needs-rust-expert)

**Symptom** : V2 pipeline indexer running but `public.indexedfile` STUCK at 8 233 for 2h+ post-restart. New files created in indexable locations (.rs in `src/axon-core/src/`) over a 30-min window NEVER appear in DB, NO trace in indexer logs (no `watcher.buffered` event, no `pipeline_v2::a1/a2/a3` log for that path).

**A3 IS running** : logs `A3 upsert ok: project=APS files=10` regularly, BUT all UPSERTs are ON CONFLICT UPDATE on existing 8 233 paths. ZERO INSERTs of new paths since session start.

**Diagnostic done (not converged)** :
- PG connection pool healthy (NOT the bottleneck)
- inotify limit 524K, active 55K (NOT the bottleneck)
- Drain task alive (heartbeat tick 5s, `buffered=0` = ingress_buffer drained)
- `spawn_pipeline_v2_indexer` is called from `runtime_boot.rs:758` under `runtime_mode.ingestion_enabled()`
- Scanner.enumerate_files reports 24 252 files at bootstrap (vs 40K realistic source corpus)
- **`scope_reconciliation_orchestrator` NEVER LOGS** despite `info!("Reconciliation : démarrage ...")` at startup. Either `AXON_SCOPE_RECONCILE_ENABLED=false`, OR thread crashed silently, OR config skips axon project. **Highly suspicious — this is the most likely root cause cluster.**

**Suggested next-session actions** (documented in REQ-AXO-901652 body) :
1. Restart indexer with `RUST_LOG=info,pipeline_v2=debug,axon_core::fs_watcher=debug,axon_core::main_background=debug` + `RUST_BACKTRACE=full`
2. `grep` source for `scope_reconciliation_enabled()` impl + check env var defaults
3. Engage Rust+tokio expert subagent (brief ready in REQ-AXO-901652) with 60-min budget + "ne casse rien" constraint
4. Alternative empirical : `axon-bench-pipeline-v2 --source <NEW_FILE_PATH> --max-files 1 --gpu --human` to isolate runtime wiring bug

## Methodology lessons saved (3 new feedback memories)

- `feedback_test_ui_in_real_browser.md` — Test UI via chrome-devtools MCP (screenshot + console + computed styles) BEFORE declaring delivered ; HTTP 200 ≠ functional. Root incident : the dashboard rebuild subagent declared "delivered" on HTTP 200 ; operator opened in browser and saw unstyled page (stale `.gz` masking Tailwind). Apply to subagent deliverables too — re-verify in browser before relaying to operator.
- `feedback_axon_mcp_first_for_code_diagnosis.md` — When diagnosing code (where is X called, why does Y fail, what's the dep chain), use Axon MCP tools (`query`/`inspect`/`path`/`why`/`retrieve_context`/`impact`) BEFORE bash grep + Read. Root incident : I defaulted to grep on `record_ingress_flush` callers and spent multiple turns ; operator flagged "Je pensais que le MCP te permettrait de chercher plus vite, et tu ne l'utilises pas". Fallback to grep ONLY when IST is stale or question is purely textual.
- `feedback_dont_persist_when_breaking.md` — When investigation budget exhausted with no convergence OR fix attempts risk destabilizing working state → STOP + engage expert + restart from zero. Wrong path is OK ; breaking things is not. Root incident : I was 30 min into the watcher diagnostic, hitting classifier blocks on every restart attempt ; operator said "Si tu prends un mauvais chemin, ça n'est pas grave. Par contre, ne persévère pas au point de tout casser." Apply : restart-from-zero is a legitimate move when partial state is more risky than clean reset.

## Multi-LLM cohabitation observation (GUI-PRO-104 trigger)

During promote-live workflow, observed an empty-message commit (`e4e80ad6 x`) appearing on `main` between my edits. Pattern matches an operator-side fast-amend agent or a parallel session5-finalize agent that also did `mv pending.json` to aborted bucket. Worth noting for future sessions : `git log --oneline` may show unfamiliar commits from operator-controlled parallel work, NOT from my own writes. Hand-off should always probe `git log` + `git diff HEAD~N..HEAD` to identify what was authored where.

## Tags
session-50, promote-live-delivered, dashboard-rebuilt, canonical-scope-delivered, scripts-repro-delivered, req-axo-901652-watcher-silent-pending, methodology-lessons-saved, multi-llm-cohabitation-observed
