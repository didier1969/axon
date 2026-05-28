# Session 59 — Hand-off (2026-05-28)

Audit-only narrative. Canonical state = SOLL `CPT-AXO-052` (session_pointer).
This file = prose context only.

## Scope compressed

13 commits since session 57 base `2aca60e9`. Live brain promoted to
`v0.8.0-757-g6b75d7f7` (generation `live-20260528T221633Z`).

## What shipped

### Pipeline v2 — demand-pull architecture (DEC-AXO-901619 + 901620)
Replaces supply-push cold-start by PG `LISTEN/NOTIFY` wake on
`file_discovered` / `chunk_pending_embed`. Two-value threshold model
(reorder + batch), poison-pill bounded retries, durable discovery queue
via `IndexedFile.status`, adaptive safety poll 1s when draining / 30s
when idle. 7 + 4 expert-review fixes after first cut. Unit tests on
metrics + threshold logic + constants (`32269bc1`).

### DDL idempotent for existing instances (`02c04e26`)
`ALTER TABLE … ADD COLUMN IF NOT EXISTS` for the 6 IndexedFile columns
+ 1 Chunk column added in session 58. Without this commit, the live
brain bootstrap crashed at startup on any DB that pre-dated session 58
(observed live this session — root cause of the live brain DOWN at
session start).

### Bench fidelity fix (`6b75d7f7`)
The pipeline_v2 bench was not hydrating `load_embedding_dedup_cache`,
while the production runtime (`pipeline_v2_runtime.rs:284`) does.
Consequence : B1 skipped nothing, B2 re-embedded 12k chunks already in
PG every run, B3 silently de-duped at UPSERT. Wall throughput was
dominated by wasted GPU cycles.

Direct measurement before/after on the same 857-file source dir with
109k existing embeddings in PG :

| | pre-patch | post-patch |
|---|---|---|
| Wall | 189.6s | 16.1s (12×) |
| Files/s | 4.11 | 50.29 |
| Chunks embedded by B2 | 12439 | 71 |
| B2 t_work_ratio | 99.6% | 10.2% |
| Goldratt drum | A2 @ 99.94% | A3 @ 90.89% |
| Cache hydrated | none | 109063 entries |

The drum shifts from A2 (tree-sitter parse) to A3 (graph write) once
the bench reflects production steady-state — A2 is NOT the limiting
factor in normal operation, that was a measurement artifact.

### Code quality
- 12 warnings → 0 (`ba143b2c`)
- pipeline v2 error-path tests REQ-AXO-901777 (`2d18a8be`) : A2
  corrupted file, B2 OOM, B3 dimension mismatch
- Phantom CL slices 1-5 (`089aa140`)

## Methodology decisions

### Against (s, S) on the demand-pull puller
Operator asked whether the adaptive wait 1s/30s was a kludge that should
be replaced by a logistics (s, S) reorder model. Investigation :
the bench reports `bp = 0` on all six stages of pipeline v2. The puller
is never the bottleneck, even on a 200-file backlog drain. Implementing
(s, S) would add ~80 LOC of A1↔puller coupling for measured gain ≈ 0.
Decision : keep the current adaptive polling, do not implement (s, S).

### F-05 already fixed in production
Operator interpreted the second bench run (GPU saturated 100% for 3+
minutes embedding chunks that produced +5 row deltas in PG) as
confirmation of session-55 audit finding F-05 (post-write dedup cache
not updated). Re-reading the code :

- `stage_b3.rs:213-220` already contains `cache.insert(chunk_id,
  source_hash)` after successful upsert. F-05 fix landed pre-session-59.
- `pipeline_v2_runtime.rs:284` hydrates the cache at boot via
  `load_embedding_dedup_cache(&store)` then threads it through
  `spawn_pipeline_b_full_multi(…, embedding_dedup)`.

Production is correct. The observed GPU waste was bench-side : the
bench bypassed both branches. Fixed by `6b75d7f7`.

## New REQ logged this session

- **REQ-AXO-901782** — `promote_live.sh` step 5 spawns brain in
  `brain_only` mode while post-check expects `indexer_ready=True`.
  Reproduces on every promote. Workaround documented in CPT-AXO-052
  blockers : start indexer + dashboard via process-compose REST, then
  `promote-live --finalize-only`. REFINES REQ-AXO-901735.

## Promote sequence used this session

Twice this session, same pattern :
1. `bash scripts/release/promote_live_safe.sh --project AXO` → builds,
   manifest, then **step 5 timeout** at post-check (`indexer_ready=False`)
2. `curl -X POST http://127.0.0.1:8080/process/start/axon-indexer` +
   same for dashboard
3. `./scripts/axon promote-live --manifest <pending> --finalize-only`
   → finalize succeeds (live MCP build_id matches), manifest swapped

## Next-session entry points

Cf CPT-AXO-052 §Next-session actions for the canonical list. Summary :

1. **REQ-AXO-901782** — fix promote `--restart-live` to bring brain +
   indexer + dashboard up, not brain_only. Closes the workaround loop.
2. **Tune `b2_batch_size: 64 → 128 or 256`** — measure with the now-
   trustworthy bench. Current GPU power draw ~115W on a 220W RTX 3070
   ceiling, util oscillates 25-100% with bursty pattern during real
   work.
3. **Investigate source_hash 71→5 ratio** — post-fix bench embeds 71
   chunks but only +5 PG rows persist. The 66 deduped at UPSERT mean
   B1 cache didn't catch them (chunk_id present in cache but content_hash
   differs from what A2/A3 computed THIS run). Possible non-determinism
   in hash computation between bench process and live indexer.

## Reference IDs

- Session pointer : `CPT-AXO-052`
- Demand-pull architecture : `DEC-AXO-901619`, `DEC-AXO-901620`
- Promote-live bug : `REQ-AXO-901782` (REFINES `REQ-AXO-901735`)
- Bench fix commit : `6b75d7f7`
- Discipline (session 57) : `GUI-AXO-1023` Swiss-hiking
