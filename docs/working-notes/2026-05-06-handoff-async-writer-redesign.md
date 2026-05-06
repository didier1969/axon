# Handoff — REQ-AXO-193 direction E (async writer redesign)

**Author:** Claude Opus 4.7 V3
**Date:** 2026-05-06 ~12:30 UTC
**Context:** session ended at 46% remaining; full async writer redesign deserves a fresh session with full context budget.
**Branch HEAD:** `61cd9c0` on `main` (pushed)
**Lib tests:** 943/0/2 (preserve)

## Why this is now the priority

VAL-AXO-040 functionally validated DEC-AXO-074 Direction A (background archiver moves Chunk.content to Parquet AFTER embedding). The architecture works:
- 463-539/699 files reach `vector_ready=TRUE` (was 12 baseline — only 0-chunk files)
- 4455-4711 chunks archived per probe to Parquet `chunk_content/`

But: throughput dropped to 25 ch/s vs 57 baseline (-56%) when env enabled. The Bug 2 fix (`mark_file_vectorization_work_done` ALSO updates `File.vector_ready=TRUE` in execute_batch) is **architecturally required for correctness post L.1+L.2** — without it, files indexed under Parquet embeddings never reach vector_ready. The throughput cost comes from added Writer Actor mutex contention. Tuning archiver knobs (batch 1000→200, interval 30s→15s, busy threshold 50→500) showed no measurable improvement.

DEC-AXO-072 K.1 (commit `bb97fdb`) already documented the same dynamic at a smaller scale: when the writer mutex is held longer, all other writers (vector_lane, archiver, graph projection) starve. Direction E breaks this by routing ALL writes through a single async thread doing bulk transactions.

## What to build

Per operator's spec (REQ-AXO-193 update):

```
1. Producer side (graph projection + embed): tout en mémoire,
   structures Rust (HashMap, Vec). Aucun touch DuckDB synchrone.
2. Channel buffered: le producer pousse des "diffs" (Vec) dans un
   channel borné. Si le consumer est lent, le producer attend
   (backpressure naturelle) — mais le producer ne bloque pas un par
   un, il pousse par lots de 1000+.
3. Single writer thread: consume le channel, fait des bulk INSERTs
   de 10k lignes par transaction DuckDB. Plus de 50ms par transaction
   = 50k chunks/s écrits.
4. WAL flush asynchrone: DuckDB checkpoint en background.
```

Target throughput: **150-200 ch/s** (vs current 57 baseline, 25 with Bug 2 fix).

## Phased plan (~860 LOC total)

### E.1 — Diff types + producer-side accumulator (~80 LOC)

**New file:** `src/axon-core/src/graph_ingestion/async_writer.rs`

```rust
pub enum WriteDiff {
    Symbols(Vec<SymbolRow>),
    Chunks(Vec<ChunkRow>),
    Contains(Vec<(String, String, String)>),
    Calls(Vec<(String, String, String)>),
    FileStateUpdate {
        paths: Vec<String>,
        status: FileStatus,
        stage: FileStage,
        graph_ready: bool,
        vector_ready: bool,
        // ... (mirror current UPDATE File columns)
    },
    FileVectorizationDone(Vec<FileVectorizationWork>),  // DELETE FVQ + UPDATE vector_ready
    ChunkEmbeddingPersist(Vec<(String, String, Vec<f32>)>),  // chunk_id, source_hash, embedding
}
```

The accumulator collects diffs and renders bulk SQL on flush:
```rust
pub struct WriteAccumulator {
    symbols: Vec<SymbolRow>,
    chunks: Vec<ChunkRow>,
    // ... etc.
}

impl WriteAccumulator {
    pub fn absorb(&mut self, diff: WriteDiff);
    pub fn row_count(&self) -> usize;
    pub fn render_bulk_queries(&self) -> Vec<String>;
    pub fn reset(&mut self);
}
```

### E.2 — Async writer thread (~150 LOC)

Spawn at `EmbeddingService::new` (alongside parquet store installs). Single thread, owns the writer mutex via `graph_store.execute_batch()`.

```rust
fn writer_loop(rx: Receiver<WriteDiff>, graph_store: Arc<GraphStore>) {
    let mut accumulator = WriteAccumulator::new();
    let mut last_flush = Instant::now();
    const ACCUMULATOR_BATCH: usize = 10_000;
    const FLUSH_IDLE: Duration = Duration::from_millis(50);

    loop {
        match rx.recv_timeout(FLUSH_IDLE) {
            Ok(diff) => accumulator.absorb(diff),
            Err(RecvTimeoutError::Timeout) => { /* fall through to flush check */ }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let ready_to_flush = accumulator.row_count() >= ACCUMULATOR_BATCH
            || (accumulator.row_count() > 0 && last_flush.elapsed() >= FLUSH_IDLE);
        if ready_to_flush {
            let queries = accumulator.render_bulk_queries();
            if !queries.is_empty() {
                if let Err(e) = graph_store.execute_batch(&queries) {
                    error!("async writer flush failed: {:?}", e);
                }
            }
            accumulator.reset();
            last_flush = Instant::now();
        }
    }
}
```

### E.3 — Producer refactor (~250 LOC)

`insert_file_data_batch_with_vectorization_policy` (currently 600+ lines, builds SQL strings inline).

Refactor: instead of `chunk_values.push(format!(...))`, push `ChunkRow { ... }` to a `Vec<ChunkRow>`. Then build `WriteDiff::Chunks(rows)` and send via `writer_tx.send()`. Same for Symbol, CONTAINS, CALLS, FileStateUpdate.

Channel: `crossbeam_channel::bounded(100)` (already a dep). Push backpressures if writer is behind.

### E.4 — Vector lane integration (~50 LOC)

`mark_file_vectorization_work_done` becomes `writer_tx.send(WriteDiff::FileVectorizationDone(work))`.

Vector lane's `update_chunk_embeddings` (DuckDB ChunkEmbedding INSERT) becomes `WriteDiff::ChunkEmbeddingPersist(rows)`. When L.1+L.2 active (Parquet store enabled), the write goes to Parquet directly (no DuckDB diff needed) — keep the existing branch.

### E.5 — Background CHECKPOINT (~20 LOC)

Already exists (`bb97fdb` K.1). Verify cadence (5s) is appropriate. Move out of the main writer thread if it's there.

### E.6 — Validation

`scripts/dev/probe_val38.sh val41-E-runN 90 10` × 3 fresh runs, both Parquet envs on, NO live indexer concurrent.

**Acceptance:**
- ≥150 ch/s mean across 3 probes (vs current 25)
- σ ≤15%
- Lib tests preserved ≥943/0/2 + new async_writer tests
- Functional: `vector_ready=TRUE` for ≥95% of chunked files
- Functional: archiver moves chunks (chunk_content/ partitions populated)

Capture VAL-AXO-041.

## Anti-patterns (DO NOT DO)

- ❌ Keep any synchronous `self.execute()` in producer paths. The point is to remove ALL synchronous DuckDB writes from graph_projection and vector_lane.
- ❌ Bypass the writer thread for 'small' updates (e.g., FVQ DELETE). Mutex contention is the root cause.
- ❌ Per-file flushes. Let the accumulator batch naturally up to 10k rows or 50ms idle.
- ❌ Split into multiple writer threads. The mutex moves but doesn't disappear.
- ❌ Touch DEC-AXO-073 L.1 (parquet_embedding_store) or DEC-AXO-074 M.1 (parquet_chunk_content_store) modules. They're correct and stay.
- ❌ Drop the Bug 2 correctness fix. `mark_file_vectorization_done` MUST still propagate `vector_ready=TRUE` post-embed — just route it through the diff channel.

## Risks + mitigations

| Risk | Mitigation |
|---|---|
| Producer crashes between push and flush → diffs lost | Accept (data loss bounded to ~50ms / 10k rows). Producer can replay via FVQ retry mechanism if needed. |
| Channel saturation deadlocks producer | Bounded channel (100). Backpressure naturally throttles graph_projection. |
| Writer thread crashes | Supervise via existing `axonctl` infrastructure. Emit fault → restart writer. |
| Bulk transaction (10k rows) takes too long → blocks reads | reader uses ist-reader.db (separate replica). Writer doesn't block readers. |
| Existing tests break (940+ tests assume sync writes) | Audit each test. Most test individual queries via `graph_store.execute()` — keep that as a TEST-ONLY pathway. Production code goes through diff channel. |

## Carried over (not blocking E)

- **Phase L.4 crash safety** (~50 LOC): ArrowWriter `.tmp` rename + boot-scan orphan cleanup for both Parquet stores. Low priority. Defer to after E lands.
- **REQ-AXO-189** P0 batch (4 MCP friction items). Defer.
- **REQ-AXO-190** Writer Actor commit_ms growth secondary driver. Largely subsumed by E (single writer + bulk = no growth).

## Pre-flight (before starting E in next session)

1. `git pull --ff-only origin main` — ensure tip is `61cd9c0` or newer
2. `./scripts/axon-live status` — verify live brain healthy (don't disturb)
3. `./scripts/axon-dev status` — verify dev DOWN (clean slate)
4. Run baseline probe: `bash scripts/dev/probe_val38.sh baseline-pre-E 90 10` with both env vars OFF — confirm 57 ch/s reproduces. If lower, calibrate the live indexer overhead.
5. Read `src/axon-core/src/graph_ingestion.rs` lines 440-1000 (the producer hot path)
6. Read `src/axon-core/src/embedder/vector_worker_loop.rs` lines 200-450 (vector lane mark_done path)
7. Read `src/axon-core/src/worker.rs` lines 350-500 (existing WorkerPool batching)
8. Then start E.1.

## Live runtime status (preserve)

- `bin/axon-brain` pid=6471 (started 2026-05-05 ~21:06 UTC) — HEALTHY, serving MCP clients
- `bin/axon-indexer` pid=3802 (started 2026-05-06 ~12:00 UTC) — graph-only, indexing `/home/dstadel/projects` (cold-start in progress as of session end, ~229 in graph_projection_queue)

**DO NOT** stop live processes during the next session. Operator authorized live activation explicitly. Live indexer will eventually drain its queue and become quiet (~15 min from session end). Probes during that warm-up will show ~13 ch/s overhead per VAL-AXO-040 calibration; once warm, overhead should be <2 ch/s.
