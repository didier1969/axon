# Bench `axon-bench-pipeline-v2` — Output interpretation guide

**REQ** : REQ-AXO-289 (streaming pipeline v2) — slice S6a iter 7 deliverable
**Audience** : operator running S6b (real GPU bench) + future LLM sessions

---

## Quickstart

```bash
# Sustained GPU bench, 60-second window, 10-sec warmup, 3000-file pool cycled
AXON_B2_BATCH_SIZE=128 AXON_A3_WORKERS=8 \
  scripts/dev/bench-v2.sh --source /home/dstadel/projects/axon

# CSV for machine consumption
scripts/dev/bench-v2.sh --csv > bench_v2_gpu.csv

# Smoke test without GPU / PG dependency
scripts/dev/bench-v2.sh --noop --duration-secs 10 --human
```

The wrapper sets `ORT_DYLIB_PATH` / `LD_LIBRARY_PATH` / `AXON_GPU_*` env so the GPU embedder can dlopen TensorRT, then invokes `.axon/cargo-target/release/axon-bench-pipeline-v2` (auto-rebuilds on `.rs` change).

---

## Reading the metrics

The bench reports two throughput numbers and per-stage atomics. Anatomy:

```
axon-bench-pipeline-v2: 2470 files / 47812 chunks in 60.0s
→ wall    : 41.16 files/s · 796.86 chunks/s
→ sustained (post-warmup): 53.42 files/s · 1023.50 chunks/s
a1 in/out/err/bp = 12500/12500/0/4231
a2 in/out/err/bp = 12500/12498/2/127
a3 in/out/err/bp = 2470/2470/0/0
b1 in/out/err/bp = 47823/47812/11/0
b2 in/out/err/bp = 47812/47812/0/0
b3 in/out/err/bp = 47812/47812/0/0
PG rows: Symbol=1530 Chunk=47812 IndexedFile=2470 ChunkEmbedding=47812
cycle=true duration_secs=60 warmup_secs=10 pool_size=3000
```

### Throughput

- **wall** — counts every receipt observed since program start. Includes the warmup window (TensorRT compile, OS file cache fill, deadpool ramp-up). **Use only when `--warmup-secs 0`.**
- **sustained (post-warmup)** — counts receipts between the warmup snapshot and bench deadline. **This is the operator north-star metric** to compare against the ≥250 ch/s target / ~47.84 ch/s legacy baseline.

### Per-stage counters

Each stage emits four atomics, displayed as `in/out/err/bp`:

| Field | Meaning |
|---|---|
| `in` | `items_in_total` — work() was invoked this many times on this stage |
| `out` | `items_out_total` — work() returned Ok and the result was forwarded downstream |
| `err` | `errors_total` — work() returned Err (logged, item dropped, NO retry) |
| `bp` | `backpressure_blocks_total` — number of times the worker's downstream `tx.send()` observed a full channel (`tx.capacity() == 0`). A non-zero `bp` on stage S means **stage S+1 is the bottleneck** (S can produce faster than S+1 consumes). |

### Bottleneck identification rule

The slowest stage has the highest **incoming** backpressure (from upstream `bp`). Bisect by reading the `bp` column top-down :

- `a1_bp > 0` → A2 is slower than A1 (tree-sitter parsing limits throughput)
- `a2_bp > 0` → A3 is slower than A2 (PG write rate)
- `a3_bp = 0` typically — A3→B1 is `try_send` non-blocking, doesn't backpressure
- `b1_bp > 0` → B2 is slower than B1 (GPU batch dispatch — this is **expected** for a healthy bench, B2 is the natural bottleneck under GPU mode)
- `b2_bp > 0` → B3 is slower than B2 (pgvector UPSERT rate)

A clean bench shows :
- **GPU-limited regime** : `b1_bp` high (B1 piling chunks for the GPU), `b2/b3 bp` near zero
- **DB-write-limited regime** : `a1/a2 bp` high (PG write rate is the ceiling)
- **I/O-limited regime** : nothing backpressured, GPU idle (very rare — usually only on cold storage / network FS)

### A3→B1 chunk fan-out

A3 emits N chunk_ids per file (avg ~14 for Rust source under BGE-Large chunker). Under sustained-mode with file cycling, the *same* chunk_ids are emitted on each cycle (idempotent UPSERT on PG side, just a re-emit on the channel). So `b1_in` counts every chunk-emission, not unique chunk_ids.

**Read this as** : "GPU lane processed `b1_in` payloads" — including re-cycled duplicates.

### Items in flight at deadline

Under `--duration-secs N`, the bench cuts off at exactly N seconds. Items in-flight downstream are LOST from the counters because their respective stages never recorded the completion. Typical pattern :

- `a1_in == a1_out` exactly (A1 is fast)
- `a2_in` slightly > `a2_out` (a few items mid-parse at cutoff)
- `a3_in == a3_out` if A3 finishes its UPSERT before deadline
- `b1_in > b1_out > b2_in > b2_out > b3_in > b3_out` — each downstream stage has *some* items still in transit at cutoff

This is **expected** for time-boxed sustained-bench mode. To minimize the tail, prefer `--duration-secs 60` over `--duration-secs 5` (longer warm → smaller relative tail).

### PG row sanity

After the bench, the wrapper queries `Symbol / Chunk / IndexedFile / ChunkEmbedding` row counts via the writer ctx (not reader — see commit 294e09c for the why). They should reconcile :

- `IndexedFile` = unique paths seen by A3 (≈ pool size, not cycle count — idempotent UPSERT)
- `Symbol` = unique symbols across the pool (~10× the file count for Rust)
- `Chunk` = unique chunks (~1.7× the symbol count under BGE-Large chunker on Rust)
- `ChunkEmbedding` = should match `Chunk` if B3 caught up (otherwise it's slightly less due to in-flight tail)

A `ChunkEmbedding` count significantly below `Chunk` after a long bench (e.g. 50% behind) is a sign of GPU starvation — B2's batched worker isn't getting fed fast enough. Usually means A3 is too slow (raise `AXON_A3_WORKERS`).

---

## Tuning knobs

| Env var | Default | When to raise |
|---|---|---|
| `AXON_B2_BATCH_SIZE` | 64 | Always raise to 128 for BGE-Large peak throughput |
| `AXON_B2_BATCH_TIMEOUT_MS` | 200 | Lower (50-100ms) for low-latency probing; keep 200 for sustained bench |
| `AXON_A1_WORKERS` | 4 | Match SSD I/O parallelism — diminishing returns past 8 |
| `AXON_A2_WORKERS` | 8 | Match physical CPU cores (tree-sitter is CPU-bound) |
| `AXON_A3_WORKERS` | 6 (bench-min) | Match PG deadpool capacity — too high thrashes the connection pool |
| `AXON_B1_WORKERS` | 4 | Match PG read parallelism |
| `AXON_B2_WORKERS` | 1 | One per physical GPU — multi-GPU is future REQ |
| `AXON_B3_WORKERS` | 2 | pgvector UPSERT — too high triggers HNSW index contention |
| `AXON_PIPELINE_INTERNAL_CHANNEL_CAP` | 1024 | Lower (256) to surface backpressure faster ; raise (4096) for more in-flight under bursty load |
| `AXON_PIPELINE_A3_TO_B1_BUFFER_CAP` | 10_000 | Cross-pipeline buffer — raise only if `try_send` drops are visible (unlikely) |

## Comparing to baseline + northstar

After the bench prints its CSV row :

```bash
# Quick extract of sustained chunk throughput
grep '^v2-bench' bench_v2_gpu.csv | awk -F, '{print "sustained chunks/s:", $7}'
```

Reference values :
- **Legacy baseline** : `~47.84 ch/s` end-to-end (session 13, PG-promoted, BGE-Large)
- **Operator northstar** : `≥250 ch/s sustained` — gate for REQ-AXO-292 FTS unlock
- **GPU-saturation ceiling** : `~280 ch/s peak` (VAL-AXO-050 isolated embedder bench, batch=128) — pipeline-overhead absorbs ~10% → expect ~250 ch/s ceiling under v2

If sustained < 250 ch/s, bisect via the `bp` column:
- B2 bottleneck → raise `AXON_B2_BATCH_SIZE` (try 128, 192) ; check VRAM
- A3 bottleneck → raise `AXON_A3_WORKERS` ; check PG deadpool size, indexer pg_stat_activity
- B3 bottleneck → raise `AXON_B3_WORKERS` ; check pgvector HNSW concurrent-update settings

## Smoke verification (no GPU / no PG)

```bash
scripts/dev/bench-v2.sh --noop --duration-secs 10 --warmup-secs 2 --human
```

This bench uses `NoOpEmbedder` (deterministic `[1, 0, 0, ...]` vectors) and a `/tmp/`-backed legacy embedded store. Useful to verify the bench wiring + bench script + tooling without touching live infra. Expected output : ~60-200 ch/s under NoOp (CPU bound by the bench infrastructure itself, not the actual GPU).

## Tags

`req-axo-289-evidence`, `s6a-iter-7`, `operator-tooling`, `pre-s6b`, `northstar-250-chps`
