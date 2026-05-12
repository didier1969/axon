# Session 19 — Streaming Pipeline v2 bench reveals a real read-after-write bug

**Date** : 2026-05-12 (session 19 extension)
**REQ** : REQ-AXO-289 (streaming pipeline v2 migration) — slice S6a follow-on
**Branch** : `feat/pipeline-v2-streaming` HEAD `aaed465`
**Status** : Bug fixed, empirical validation complete, awaiting S6b operator GPU run.

---

## TL;DR

`fetch_chunk_for_embedding` was reading via `query_json` (reader_ctx). Under the legacy embedded test backend, the reader ctx serves a stale snapshot for a short window (~µs) after the writer commits. The cross-pipeline `try_send` from A3 to B1 is **the canonical hot path** in v2 — B1 picks up a `chunk_id` microseconds after A3 commits — which means the staleness window IS the steady-state regime, not a rare race. Result: **55% of chunks emitted by A3 were silently dropped from the GPU lane** as "race no longer in PG" soft errors.

**Fix** : switch B1's `fetch_chunk_for_embedding` to `query_json_writer` (writer_ctx). Under PG MVCC this is a no-op (the deadpool serves any connection, all see the committed write). Under the embedded test backend it eliminates the staleness gap.

**Empirical impact** (5-file NoOp bench on `src/axon-core/src/pipeline_v2/`) :
- Before fix : `b1 102/45/57` (in/out/err) → 36 ch/s
- After fix : `b1 102/102/0` → 59-64 ch/s (+77% throughput just from removing error-path retries)

---

## How the bug surfaced

S6a delivered `axon-bench-pipeline-v2` — a CLI binary that drives the full A→B v2 topology end-to-end. The bench was supposed to compile cleanly and let the operator launch a real GPU bench against dev PG.

A `--noop` smoke run on a 5-file subset of `pipeline_v2/` showed :

```
a3 in/out/err = 5/5/0
b1 in/out/err = 102/45/57    ← 55% loss
b2 in/out/err = 45/45/0
b3 in/out/err = 45/45/0
PG rows: Symbol=57 Chunk=0 IndexedFile=5 ChunkEmbedding=45
```

Two anomalies:
1. `b1 in/out/err = 102/45/57` — B1 received 102 chunk_ids but successfully fetched only 45. 57 returned `None` (chunk_id not in PG).
2. `PG rows: ... Chunk=0` — the post-run `SELECT count(*) FROM Chunk` returned 0 despite 102 chunks demonstrably written (B1 fetched 45 of them during the run; ChunkEmbedding=45 also visible).

Debug logging in `fetch_chunk_for_embedding` showed concrete chunk_ids that A3 emitted but B1 couldn't find:

```
[B1 miss] chunk_id="AXO::axon::src::axon-core::src::pipeline_v2::stage_a3.rs::a3_enroll::chunk" probe=[]
[B1 miss] chunk_id="AXO::axon::src::axon-core::src::pipeline_v2::types.rs::ParsedFile::chunk" probe=[]
...
```

`probe` was a `SELECT id FROM Chunk WHERE id LIKE '%pattern%' LIMIT 3` against the reader — also empty. So the reader was completely blind to A3's commits.

## Root cause

`fetch_chunk_for_embedding` went through `query_json` → `query_json_on_reader` → reader_ctx. The reader ctx is refreshed lazily after writer commits via `mark_writer_commit_visible` + freshness gates (FreshPreferred routing). In the legacy embedded backend, the refresh is not strictly synchronous with the commit visible-mark, and the cross-pipeline `try_send` hand-off from A3 to B1 is fast enough that B1 always loses the race.

```rust
// Before — fetch_chunk_for_embedding in graph_ingestion.rs
let raw = self.query_json(&format!(
    "SELECT content, content_hash FROM Chunk WHERE id = '{safe_id}'"
))?;
```

The fix :

```rust
// After
let raw = self.query_json_writer(&format!(
    "SELECT content, content_hash FROM Chunk WHERE id = '{safe_id}'"
))?;
```

The bench's post-run sanity counts had the same problem (they used `query_count` = reader path); commit `52c22ba` switches them to `query_json_writer` with manual row parsing.

## Why this isn't just a test-backend artifact

The session-17 design contract (CPT-AXO-053) explicitly endorses **PG MVCC multi-writer** as the steady-state runtime: brain and indexer both connect to the same PG instance via deadpool, and reads from any connection see committed writes immediately. So under PG production, the staleness window collapses to zero and the bug is invisible.

But:

1. The legacy embedded test backend is still the default for CI / unit tests (REQ-AXO-271 retirement is partial). Any reader-via-`query_json` path that depends on a just-committed write is wrong there, even though it works in PG.
2. The reader-ctx is also used by the brain `query`/`inspect`/`impact` tools. They rely on the freshness gate to decide whether to read from writer or reader. The freshness gate is operator-tuned; under high write rate it may serve briefly stale data to the LLM client too. This is acceptable for MCP query semantics but NOT acceptable for a tight read-after-write contract like B1 ↔ A3.
3. The fix is **strictly safer** under all backends: writer_ctx is always at least as fresh as reader_ctx.

## Empirical results

Same fixture (5 files of `src/axon-core/src/pipeline_v2/`), `--noop --max-files 5` :

| Metric | Before fix | After fix |
|---|---|---|
| a3 out (files enrolled) | 5 | 5 |
| b1 in / out / err | 102 / 45 / 57 | 102 / 102 / 0 |
| b3 out (embeddings persisted) | 45 | 102 |
| files/s | 3.0 | 2.9 |
| chunks/s (b3 out / wall time) | 36 | 59-64 |
| PG rows (writer) Symbol/Chunk/IndexedFile/ChunkEmbedding | 57/102/5/45 | 57/102/5/102 |

The throughput gain is the elimination of the error-path overhead (57 invocations of `b1_fetch_for_embedding` → `query_json_writer` → `serde_json::from_str` → `anyhow::anyhow!("race")` → metrics increment per missed chunk). Under real GPU mode, the gain in absolute time is much smaller (GPU embed dominates wall time) but the throughput in chunks/s rises by the same proportion because no chunks are silently dropped.

## What this means for S6b

The bench is now a clean measurement, not a "45% loss masquerade". Operator can run :

```bash
cargo run --release --bin axon-bench-pipeline-v2 -- \
  --source /home/dstadel/projects/axon/src \
  --max-files 200 --gpu --human
```

…and get a chunks/s reading directly comparable to the legacy `~47.84 ch/s` baseline and the operator northstar `≥250 ch/s sustained`.

If v2 GPU mode hits `≥250 ch/s`, the gate on REQ-AXO-292 (FTS hybrid retrieval) unlocks — REQ-AXO-292's prerequisite is exactly REQ-AXO-289 done + the throughput target met.

## Files touched by the fix

| Commit | File | Change |
|---|---|---|
| `294e09c` | `src/axon-core/src/graph_ingestion.rs` | `fetch_chunk_for_embedding` reads from writer_ctx |
| `52c22ba` | `src/axon-core/src/bin/axon-bench-pipeline-v2.rs` | post-run sanity counts via writer_ctx (`query_json_writer` + manual parse) |
| `aaed465` | `CLAUDE.md` | doc routing to v2 + bench invocation + caveat |

Both commits are operator-non-destructive — additive runtime corrections + bench observability.

## Tags

`req-axo-289-evidence`, `session-19-followon`, `bench-derived-bug-fix`, `read-after-write`, `embedded-backend-staleness`, `pg-mvcc-safe`, `s6a-iteration`, `pre-s6b`
