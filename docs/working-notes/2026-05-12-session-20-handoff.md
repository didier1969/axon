# Session 20 hand-off — REQ-AXO-289 ~95% done, S6b GPU run blocked on WSL2 DXG deadlock

**Date** : 2026-05-12 ~21:00 (post session 19 + 20 work)
**Branch** : `feat/pipeline-v2-streaming` HEAD `3d411a4` — **22 commits ahead of main, local non-pushé** (stop A operator-gated)
**Tests** : 1209 lib + 25 bins + 47/47 pipeline_v2 + 2 doc-invariants + 2 bench unit tests — all green
**MCP** : live brain DOWN (operator stopped earlier during S6b debug). PG still up at 127.0.0.1:44144. Read this MD file ; SOLL CPT-AXO-052 update was attempted but rolled back due to brain being down.

---

## TL;DR for the next session

1. **WSL2 DXG adapter lock is wedged** — 7 processes stuck in `dxgglobal_acquire_process_adapter_lock`. Empirically demonstrated : killing nvitop, killing brain, killing the bench did NOT release the lock. Only fix : **`wsl.exe --shutdown` from Windows PowerShell**.
2. After WSL reboot, the bench tooling is **complete and operator-ready**. Just run :
   ```bash
   cd /home/dstadel/projects/axon
   AXON_DEV_DATABASE_URL=postgres://axon@127.0.0.1:44144/axon_dev \
   AXON_B2_BATCH_SIZE=128 AXON_A3_WORKERS=8 \
     scripts/dev/bench-v2.sh --csv > bench_v2_gpu.csv
   ```
3. **The deliverable** : `sustained_chunks_per_sec` column. Compare to 47.84 ch/s legacy baseline + ≥250 ch/s northstar.

---

## What happened in session 20

### S6b GPU bench attempt → blocked

1. Bench launched (commit `3d411a4` wrapper). First run hung 9 min in `dxgvmb_send_sync_msg` (WSL2 paravirt GPU bus).
2. Diagnosed contention with stale `axon-indexer` dev pid 20481 (debug build, 13h45 alive, CUDA OOM fallback to CPU, holding IST guard).
3. Operator killed 20481 + 42472 (stale monitoring poll).
4. Bench still wedged. Stale `nvitop` pid 36921 (TUI monitor, 2 days alive in `Dl+`) identified as holding `/dev/dxg` fd. Operator authorized kill ; nvitop went zombie (`Zl`) but **the WSL2 DXG adapter lock did NOT release at the kernel level**.
5. Stopped axon-live brain as alternative diagnosis — confirmed false : the 7 D-state procs (1 bench + 6 nvidia-smi probes) **stay wedged on `dxgglobal_acquire_process_adapter_lock` even after brain dies**. The lock is genuinely orphaned in `dxgkrnl`.
6. Operator and I converged on `wsl.exe --shutdown` as the only working fix. Operator typed "Axon Hand Off" — handing off to next session for the WSL reboot + bench retry.

### What was DELIVERED before the block

Session 19 + 20 net result : **REQ-AXO-289 streaming pipeline v2 is implementation-complete and bench-ready, only blocked on the WSL infrastructure issue for the final empirical validation.**

22 commits on `feat/pipeline-v2-streaming` (recent → old, all 2026-05-12) :

| SHA | Slice | Headline |
|---|---|---|
| `3d411a4` | S6a iter 7 | scripts/dev/bench-v2.sh wrapper + interpretation guide |
| `4fad3b7` | S6a iter 6 | sustained-throughput bench (--duration-secs, --warmup-secs, implicit --cycle) |
| `bde600e` | S4b' | B2 batched worker (real GPU throughput) + bench north-star defaults |
| `81000d2` | docs | refresh pipeline_v2 module rustdoc + lock invariants in tests |
| `b363278` | docs | session 19 read-after-write bug fix evidence working note |
| `aaed465` | S10a docs | CLAUDE.md project file points to v2 + bench + code_search routing |
| `52c22ba` | S6a fix | bench post-run counts via writer ctx |
| `294e09c` | **critical fix** | B1 fetch reads from writer ctx → bridges cross-pipeline read-after-write gap |
| `e80c113` | S6a | bench prints post-run PG sanity counts |
| `88cdbc8` | S6a fix | bench releases input_tx so pipeline drains |
| `8c36c77` | S6a | end-to-end bench binary scaffold |
| `8aae277` | S4d | GpuB2Embedder production wrapper around ORT/TensorRT |
| `cc02f45` | S4c | B1 cold-start poll DB pathway |
| `0206ddb` | S4b | B2 embedder trait + B3 ChunkEmbedding UPSERT + spawn_pipeline_b_full |
| `38a1215` | S4a session-19 | A persists graph+chunks+FTS atomically, B1 fetches from PG |
| `e937576` | S3d | A3 graph-only atomic UPSERT (session 18, superseded by S4a) |
| `ca99222` | S5 | Pipeline A orchestrator wires A1→A2→A3 + E2E test |
| `238d752` | S3c | A3 IndexedFile UPSERT (idempotent ON CONFLICT) |
| `b55f662` | S3b | A2 Transformation (tree-sitter spawn_blocking) |
| `8080f7e` | S3a | A1 Preparation (tokio::fs read + SHA-256 + mtime) |
| `fac5046` | S2 | IndexedFile DDL + arc-swap cache |
| `bbeaa06` | S1 | scaffolding (StageMetrics, spawn_stage_workers, channel caps) |

### Critical commits to remember

- **`294e09c`** : B1 fetch reads from writer ctx, not reader. Real bug : reader_ctx stale during cross-pipeline try_send window → 55% of chunks were silently dropped. Validated by NoOp bench going 36 → 64 ch/s after fix.
- **`38a1215`** (session 19 pivot) : A is CPU autoritative (graph + chunks + FTS in one PG transaction), B is pure GPU lane (fetch → embed → UPSERT). Lexical retrieval works without GPU. Pattern SOTA hybrid retrieval.
- **`bde600e`** : B2 batched worker. Without batching, GPU would have measured batch=1 (~10 ch/s) instead of batch=64+ (~280 ch/s peak). Operator-flagged gap, fixed before any GPU run.

---

## Process state at hand-off

### Live runtime

- **Live brain** : DOWN (`./scripts/axon-live stop --hard` ran cleanly during S6b debug)
- **Live indexer** : DOWN (since session 9, intentional)
- **Dev brain / indexer** : DOWN (axonctl stop happened, plus stale debug process was killed)
- **PostgreSQL** : UP on 127.0.0.1:44144 (devenv-managed, not GPU-affected)
- **MCP gateway (port 44129)** : DOWN (brain is down)

### Wedged processes (will die on WSL shutdown)

| pid | state | wchan | cmd | notes |
|---|---|---|---|---|
| 75298 | `D` | dxgglobal_acquire_process_adapter_lock | axon-bench-pipeline-v2 --gpu | pending SIGTERM, can't deliver |
| 73175 | `D+` | dxgglobal_acquire_process_adapter_lock | nvidia-smi (stale probe) | |
| 73259 | `D` | dxgglobal_acquire_process_adapter_lock | nvidia-smi (stale probe) | |
| 74190 | `D` | dxgglobal_acquire_process_adapter_lock | nvidia-smi (stale probe) | |
| 74513 | `D` | dxgglobal_acquire_process_adapter_lock | nvidia-smi (stale probe) | |
| 74660 | `D` | dxgglobal_acquire_process_adapter_lock | nvidia-smi (stale probe) | |
| 74864 | `D` | dxgglobal_acquire_process_adapter_lock | nvidia-smi (stale probe) | |
| 36921 | `Zl` | (zombie) | nvitop (root cause) | parent 58321 didn't reap |

All clear after `wsl.exe --shutdown` and WSL restart.

---

## Recovery procedure for next session

### Step 1 — WSL hard reset (Windows host)

From PowerShell or CMD on Windows :

```powershell
wsl.exe --shutdown
```

Then reopen any WSL terminal. WSL will boot fresh ; `dxgkrnl` adapter state is reset.

### Step 2 — Bring up the stack

```bash
cd /home/dstadel/projects/axon

# devenv shell brings up PG + tooling
devenv shell  # or open a new tmux window with the existing devenv

# Verify GPU is responsive (must NOT hang)
nvidia-smi --query-gpu=name,memory.used --format=csv

# Optional : restart live brain for ongoing MCP service
./scripts/axon-live start --brain-only
```

### Step 3 — Run the bench

```bash
AXON_DEV_DATABASE_URL=postgres://axon@127.0.0.1:44144/axon_dev \
AXON_B2_BATCH_SIZE=128 AXON_A3_WORKERS=8 \
  scripts/dev/bench-v2.sh --human
```

Or for CSV machine-output :

```bash
scripts/dev/bench-v2.sh --csv > bench_v2_gpu_$(date +%Y%m%dT%H%M%SZ).csv
```

Defaults : `--source $ROOT/src --max-files 3000 --duration-secs 60 --warmup-secs 10 --gpu`. The wrapper auto-rebuilds the bench binary if any `.rs` is newer. First GPU run will take ~5-10s for ORT/TensorRT engine load (cache exists from prior runs at `~/.cache/axon/fastembed/tensorrt/engine-cache/`).

### Step 4 — Interpret the output

Reference doc : `docs/working-notes/2026-05-12-bench-v2-interpretation-guide.md`.

Quick read of the `sustained_chunks_per_sec` column / line :
- **≥250 ch/s** → REQ-AXO-289 close → S7 cut-over (operator-gated destructive) → S8 DROP `public.file` → REQ-AXO-292 FTS unlock
- **<250 ch/s** → bisect via backpressure column :
  - `b1_bp > 0` → B2 is the bottleneck → raise `AXON_B2_BATCH_SIZE` (try 192, 256)
  - `a2_bp > 0` → A3 is the bottleneck → raise `AXON_A3_WORKERS` (try 10, 12)
  - `b2_bp > 0` → B3 is the bottleneck → raise `AXON_B3_WORKERS`

### Step 5 — Capture evidence in SOLL

Once a clean bench result is in hand :

```bash
# Attach the CSV as evidence on REQ-AXO-289
mcp__axon__soll_attach_evidence \
  entity_type=requirement entity_id=REQ-AXO-289 \
  artifacts='[{"kind":"file","artifact_ref":"<csv-path>","note":"S6b GPU sustained-throughput measurement, batch=128, A3=8 workers"}]'
```

Then create or update a `VAL-AXO-NNN` validation node tracing the result back to REQ-AXO-252 + REQ-AXO-289.

---

## Open backlog after S6b validates

| Slice | Status | Type |
|---|---|---|
| S6b | gated on WSL reboot + GPU run | operator |
| S7a (wire v2 into runtime_boot under flag) | not started | additive |
| S7b (flip default + delete legacy paths) | not started | DESTRUCTIVE, operator-gated |
| S8 (DROP public.file) | not started | DESTRUCTIVE, operator-gated |
| S5 (MCP status integration) | not started | post-S7, cross-process plumbing |
| REQ-AXO-292 (FTS migration) | gated on REQ-AXO-289 close | future |

---

## Files of note

- `src/axon-core/src/pipeline_v2/` — module root, 13 files, 47 tests
- `src/axon-core/src/bin/axon-bench-pipeline-v2.rs` — bench binary
- `scripts/dev/bench-v2.sh` — operator wrapper
- `docs/architecture/visualize-nexus-pull.html` — session 19 topology diagram
- `docs/working-notes/2026-05-12-session-19-bench-readafterwrite-fix.md` — B1 read-after-write bug evidence
- `docs/working-notes/2026-05-12-bench-v2-interpretation-guide.md` — operator-facing reading guide
- `docs/working-notes/2026-05-12-session-20-handoff.md` — **this file**
- SOLL `CPT-AXO-052` (canonical session pointer, not updated this session due to MCP down) + `CPT-AXO-054` (implementation contract) + `REQ-AXO-289` (umbrella with attached evidence) + `REQ-AXO-292` (planned, gated)

## Tags

`session-20-handoff`, `s6b-blocked-wsl-deadlock`, `dxgkrnl-adapter-lock-orphan`, `wsl-shutdown-required`, `branch-feat-pipeline-v2-streaming-22-commits`, `req-axo-289-95-percent`, `req-axo-292-still-gated`, `mcp-down-due-to-debug`
