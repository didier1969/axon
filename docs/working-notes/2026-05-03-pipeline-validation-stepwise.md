# Pipeline validation stepwise — 2026-05-03

> Working note for the embedding-pipeline diagnostic. Live brain v0.8.0-160-gdd579c0 unchanged. Dev IST wiped clean before this session.

## Goal

Validate the two pipelines independently before any benchmark:
- **Pipeline 1** : Watcher → Graph (File rows → Symbol + Chunk + CALLS + CONTAINS)
- **Pipeline 2** : Graph → Embedding (Chunk → ChunkEmbedding via vector worker + GPU model)

Success criterion for Pipeline 2 (per Didier) : produce **at least one ChunkEmbedding row in 30 seconds** after vector worker is loaded.

## Setup

```
stop dev (clean) → rm -rf .axon-dev/graph_v2/* → start dev --indexer-full
```

Dashboard dev : http://172.31.148.130:44137/

Build : v0.8.0-160-gdd579c0
GPU : RTX 3070 Laptop, 8 GB VRAM
Mode : `indexer_full`

## Observations

### T+0 (Ready ✅)
- File: 13 607 (watcher initial scan)
- Symbol: 96
- Chunk: 115
- ChunkEmbedding: 0
- FileVectorizationQueue: 25 queued
- GPU: 695 MiB (dashboard only — BGE-Large NOT loaded)

### T+30s
- File: 13 607 (stable)
- Symbol: 96 → **281** (+185)
- Chunk: 115 → **400** (+285)
- ChunkEmbedding: 0 → **0** (no progress)
- FileVectorizationQueue queued: 25 → **0** (consumed but no embedding produced)
- Files graph_ready: **72**
- GPU: 692 MiB (still no model)

## Verdict per pipeline

### Pipeline 1 — Watcher → Graph : ✅ FONCTIONNE

Evidence :
- File table populated (13 607 entries) on initial scan
- Symbol count growing (96 → 281 in 30s)
- Chunk count growing (115 → 400 in 30s)
- Files graph_ready ramping (72)
- Throughput Pipeline 1 : ≈ 6 symbols/sec, 9.5 chunks/sec, 2.4 files-graph-ready/sec

Pipeline 1 is healthy. Move on.

### Pipeline 2 — Graph → Embedding : ❌ BLOQUÉ

Evidence :
- ChunkEmbedding stays at 0 for 30s+ despite work available
- GPU memory stays at ~692 MiB (only dashboard) — **BGE-Large model never loaded**
- FileVectorizationQueue went from 25 queued → 0 — but ChunkEmbedding stays 0, so the rows didn't transition to a "completed" state with embeddings produced. Possible failure modes :
  1. Vector worker is initialized but never claims work
  2. Vector worker tries to load model and fails silently
  3. FileVectorizationQueue rows transition to a non-queued, non-completed state without producing embeddings
  4. AXON_GPU_EMBED_SERVICE_ENABLED defaulted off

Need to investigate:
- tmux pane `axon-dev-indexer:core` for vector worker startup logs
- log for `Semantic Vector Worker [0]` lines
- env vars on the running indexer process (proc fs)

## Bugs/REQs already logged this session relevant to bench

- REQ-AXO-166 : qualify_runtime cold-reset doesn't propagate AXON_RUNTIME_MODE
- REQ-AXO-167 : dev_baseline_wait grep MCP-only markers
- REQ-AXO-168 : bench script swallows rc=1
- REQ-AXO-169 : seed-dev-from-live partial copy
- REQ-AXO-170 : main.File row corruption (path/stage/status concat race)

## Next steps in this session

1. Inspect dev indexer tmux pane for vector worker / model load logs
2. Identify why Pipeline 2 is silent
3. Document root cause as REQ-AXO-NNN
4. Defer fix-then-bench to next session if context insufficient

## Status of live brain

Live brain v0.8.0-160-gdd579c0 healthy throughout — pid 93340, port 44129, MCP up. NOT impacted by dev session.
