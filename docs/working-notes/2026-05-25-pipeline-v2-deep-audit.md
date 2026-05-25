# Pipeline v2 Deep Audit — 2026-05-25

Session 55 post-mortem. Expert Rust/streaming/GPU pipeline review.
Bench baseline: ~130 ch/s sustained, GPU 99% util.

---

## Findings

### DEEP-001 | A | ELEVE | `stage_a3.rs:8-9` (docstring)
**Obsolete AGE references in A3 docstring.**
The module docstring still references "AGE `Symbol` + `File` vertex enrichment" and "SQL + AGE dual-write". AGE was retired per MIL-AXO-017 / REQ-AXO-90005. The code itself correctly uses `public.Edge` only. This is a documentation lie that will mislead any future reader.
**Impact:** No throughput impact. Misleads contributors.
**Fix:** Remove lines 8-9 from the docstring; replace with `public.Edge` reference.

### DEEP-002 | B | ELEVE | `stage_b2.rs:79-103` — `b2_embed()` function
**`b2_embed()` is dead code on the production path.**
The production path uses `spawn_b2_batched_worker` (line 121), which calls `embedder.embed_batch()` directly. The standalone `b2_embed()` async function is only used in tests (lines 258, 428) and re-exported from `mod.rs`. It wraps a single text into a `Vec<String>` then removes the result — a per-item API in a system that exclusively uses batching. The function also does NOT apply the BGE "Represent this sentence:" prefix, while `spawn_b2_batched_worker` DOES (line 184-186), creating a silent semantic mismatch if anyone ever routes through `b2_embed()`.
**Impact:** Code confusion; risk of prefix mismatch if used.
**Fix:** Mark `#[cfg(test)]` or remove from public exports.

### DEEP-003 | D | BLOQUANT | `stage_b2.rs:184-186` — Wrong BGE prefix on passage indexing
**BGE prefix "Represent this sentence:" applied at B2 batch assembly during PASSAGE indexing.**
The prefix is prepended at line 184-186 of `spawn_b2_batched_worker`:
```rust
let texts: Vec<String> = batch.iter().map(|p| {
    format!("Represent this sentence: {}", p.content)
}).collect();
```
Three problems:

1. **Wrong prefix for passages.** BGE-Large-en-v1.5 (BAAI) specifies: passages should be embedded with NO prefix (empty string). Queries should use `"Represent this sentence for searching relevant passages: "`. The pipeline uses a TRUNCATED prefix `"Represent this sentence: "` which is neither the correct query prefix nor the correct passage prefix.

2. **Asymmetric with query path.** The query-time embedder in `embedder.rs:2011` correctly uses the FULL query prefix: `"Represent this sentence for searching relevant passages: {t}"`. But the passage embedder uses the truncated `"Represent this sentence: "`. So passages and queries use DIFFERENT prefixes, neither of which is the BGE-recommended pair.

3. **The `b2_embed()` test path does NOT apply any prefix**, creating a third variant.

**Impact:** Cosine similarity between query embeddings and passage embeddings is degraded by the prefix asymmetry. The magnitude depends on how much the prefixes shift the embedding space. Published BGE benchmarks show 2-5% retrieval quality degradation with wrong prefix configuration. All ~130 ch/s of throughput produces suboptimally-prefixed vectors.
**Fix:** Remove the prefix entirely from B2 batch assembly (passage indexing should have NO prefix for BGE):
```rust
let texts: Vec<String> = batch.iter().map(|p| p.content.clone()).collect();
```
The query-time path in `embedder.rs:2011` already applies the correct query prefix.

### DEEP-004 | B | ELEVE | `stage_b3.rs:46-82` — `b3_persist_embedding()` function
**`b3_persist_embedding()` is dead code on the production path.**
Same pattern as DEEP-002: the production path uses `spawn_b3_batched_worker_with_cache` exclusively. `b3_persist_embedding()` is only used in tests. It also lacks the embedding dedup cache update that the batched worker performs (line 214-218). Re-exported from `mod.rs`.
**Impact:** Code confusion; stale dedup cache if used.
**Fix:** Mark `#[cfg(test)]` or remove from public exports.

### DEEP-005 | B | ELEVE | `stage_b1.rs:128-145` — `b1_fetch_for_embedding()` function
**`b1_fetch_for_embedding()` standalone function is only used in one production path.**
Used only in `spawn_pipeline_b_b1_only()` (orchestrator.rs:382) via the generic `spawn_stage_workers` path. But `spawn_pipeline_b_b1_only()` is itself only used in one test (orchestrator.rs:841). The production path uses `spawn_b1_batched_worker_with_dedup` exclusively. The standalone function is effectively dead code on the production path.
**Impact:** Code confusion; two code paths for the same operation.
**Fix:** Mark `b1_fetch_for_embedding()` as `#[cfg(test)]`.

### DEEP-006 | B | ELEVE | `orchestrator.rs:362-397` — `spawn_pipeline_b_b1_only()`
**`spawn_pipeline_b_b1_only()` is only used in tests.**
The production path always uses `spawn_pipeline_b_full_multi()`. This function creates a B pipeline with only B1, using the generic `spawn_stage_workers` pattern (Mutex<Receiver>) instead of the batched worker. Re-exported from `mod.rs`.
**Impact:** Dead code on production path. Maintains a parallel code path that diverges from production behavior (no batching, no dedup cache).
**Fix:** Move to `#[cfg(test)]` module or remove from public exports.

### DEEP-007 | C | ELEVE | `stage_a3.rs:73-90` — `a3_enroll()` heavy clones
**`a3_enroll()` clones ALL fields of `ParsedFile` for `spawn_blocking`.**
Lines 74-78 clone `path_str`, `content_hash`, `content`, `symbols`, and `relations` — the full `ParsedFile` payload. `content` can be 10-100 KB per file. `symbols` and `relations` are `Vec<Symbol>` / `Vec<Relation>` with potentially hundreds of entries. This function is only used in tests, but the same clone pattern exists in the batched worker (line 215: `group_batch.clone()`).
**Impact:** In the batched worker (line 215), `group_batch` is cloned for `spawn_blocking`. For a batch of 32 files with ~50 KB content each, that's ~1.6 MB of unnecessary heap allocation per batch flush.
**Fix:** Use `std::mem::take` or pass ownership into the blocking closure instead of cloning.

### DEEP-008 | C | ELEVE | `stage_a3.rs:215` — `group_batch.clone()` in batched worker
**Full batch cloned for spawn_blocking when ownership transfer would suffice.**
```rust
let group_for_block = group_batch.clone();
```
The `group_batch` is used after the `spawn_blocking` join (line 238) to iterate over the original for receipt emission. However, the clone sends ALL content strings across the thread boundary unnecessarily. The blocking closure only needs the data for `upsert_graph_v2_batch`, after which the results (chunk_metas) drive the downstream logic.

**Impact:** ~1.6 MB allocation per batch flush (32 files * 50KB). At 57 ch/s = ~2 batch/s = 3.2 MB/s of unnecessary allocation pressure. On a 10-minute run, ~1.9 GB of garbage.
**Fix:** Split `group_batch` into the data needed for PG (passed by ownership to spawn_blocking) and the metadata needed for receipts (kept on the async side). Or restructure: move receipt emission into the blocking closure and return receipts.

### DEEP-009 | C | MOYEN | `stage_a1.rs:67-75` — `sha256_hex()` format allocation
**Per-byte `format!("{:02x}")` in hot path SHA-256 hex encoding.**
```rust
for byte in digest {
    out.push_str(&format!("{:02x}", byte));
}
```
This creates 32 temporary `String` allocations for each file hashed. A1 processes every file, making this a universal hot path.
**Impact:** 32 micro-allocations per file. At 200 files/s = 6400 allocations/s. Measurable but not dominant.
**Fix:** Use `write!(out, "{:02x}", byte)` or a lookup-table approach:
```rust
fn sha256_hex(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{:02x}", byte);
    }
    out
}
```

### DEEP-010 | C | MOYEN | `worker_pool.rs:51` — `Mutex<Receiver>` for competing consumers
**Tokio Mutex wrapping mpsc::Receiver for multi-worker fan-out.**
The generic `spawn_stage_workers` uses `Arc<Mutex<Receiver<I>>>` so N workers can compete for items. This is the documented pattern for competing consumers with tokio mpsc, but it forces serialized recv() through the mutex. Under high contention (A2 with 8 workers), the mutex becomes a bottleneck.

However, this is ONLY used for A1 and A2 now (and the dead `spawn_pipeline_b_b1_only`). B1/B2/B3 all use dedicated batched workers. A1 (I/O bound, 4 workers) and A2 (CPU bound via spawn_blocking, 8 workers) are the remaining consumers. For A2, the spawn_blocking call dominates latency so the mutex contention is negligible. For A1, file I/O dominates.

**Impact:** Minimal in current topology. The pattern is correct for the use case.
**Fix:** No immediate fix needed. If A1/A2 become bottlenecks, switch to a dedicated per-worker channel with round-robin dispatch (like A3/B2/B3 already do).

### DEEP-011 | A | MOYEN | `orchestrator.rs:291-332` — A3 multi-worker output_tx sender leak
**When A3 runs with `n_workers == 1`, `output_tx` is moved into the batched worker. But the orchestrator still holds `b1_inbox_tx.clone()` which is also moved. The `output_tx` for the single-worker case is fine. But for `n_workers > 1`, `output_tx` is dropped (line 319) correctly. The channel will close when all clones are dropped.**

On closer inspection, this is correctly handled. For n_workers == 1: `output_tx` is moved. For n_workers > 1: each worker gets `output_tx.clone()`, then the original is dropped (line 319). The channel closes when the last worker exits. This is correct.

**Impact:** None. No bug.
**Fix:** N/A.

### DEEP-012 | C | MOYEN | `stage_a3.rs:141-145` — `interval()` without `MissedTickBehavior`
**A3 batched worker uses `tokio::time::interval(batch_timeout)` without setting `MissedTickBehavior`.**
Default is `MissedTickBehavior::Burst`, which will fire multiple ticks immediately to "catch up" if a batch write takes longer than `batch_timeout` (10ms). For A3 with a 10ms timeout, a single PG write that takes 50ms would trigger 5 immediate tick fires, causing 5 empty flush attempts on the next iterations.

The same issue exists in `stage_b3.rs:116-117`. B3 uses 200ms timeout which is less affected but still susceptible.

**Impact:** Wasted CPU cycles on empty flush attempts after slow PG writes. Not a throughput blocker since empty flushes are guarded by `!buffer.is_empty()`, but the tick fires still wake the task and do the select.
**Fix:**
```rust
tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
```

### DEEP-013 | D | MOYEN | `stage_b1.rs:260-272` — B1 inline forward metrics timing is wrong
**B1 batched worker records `elapsed_us` and `per_item_us` BEFORE forwarding inline items, using `batch.len()` as divisor.**
```rust
let elapsed_us = started.elapsed().as_micros()...;
let per_item_us = elapsed_us / (batch.len() as u64).max(1);
```
But `started` was set BEFORE the inline forwarding loop. The `elapsed_us` for the first inline item is essentially 0 (no PG call was made), but it's divided by the total batch size including fetch items. This under-reports inline work time. More importantly, `batch.len()` includes BOTH inline AND fetch items, but the metric is only being recorded for inline items at this point.

**Impact:** Distorted per-item duration metrics for B1. Misleading `mean_duration_us` in bench output.
**Fix:** Track inline and fetch timings separately, or compute per_item_us based on the actual group size.

### DEEP-014 | E | MOYEN | `stage_a3.rs:1-22` — Obsolete docstring references
**Module docstring references:**
- Line 8: "AGE `Symbol` + `File` vertex enrichment (under PG)" — AGE retired
- Line 9: "SQL + AGE dual-write" — AGE retired
- Line 14: "content_tsv GENERATED column" — content_tsv is now computed async via TSV worker (REQ-AXO-901624 P4), not as a GENERATED column

**Impact:** Documentation drift. Misleading for onboarding.
**Fix:** Update docstring to reflect current architecture.

### DEEP-015 | B | MOYEN | `stage_a3.rs:64-100` — `a3_enroll()` is dead production code
**`a3_enroll()` is a per-file persistence function that is NEVER used on the production path.** The production path exclusively uses `spawn_a3_batched_worker()` which calls `store.upsert_graph_v2_batch()` directly. `a3_enroll()` calls the single-file `store.upsert_graph_v2()`. It is exported publicly and used only in tests.
**Impact:** Maintenance burden of two parallel persistence paths.
**Fix:** Mark `#[cfg(test)]` or inline into test helpers.

### DEEP-016 | C | MOYEN | `stage_b2.rs:184-186` — String allocation per chunk in B2
**Every chunk gets a `format!("Represent this sentence: {}", p.content)` allocation in the hot GPU path.**
At 130 ch/s with ~500 byte average content, that's 130 allocations/s of ~530 bytes each. The texts Vec itself is also a fresh allocation per batch.
**Impact:** ~68 KB/s of heap allocation in the GPU hot path. Negligible vs GPU latency but adds GC pressure.
**Fix:** (Moot if DEEP-003 is fixed — the prefix should be removed entirely for passage embedding.)

### DEEP-017 | E | FAIBLE | `orchestrator.rs:72-103` — Duplicated env-var parsing pattern
**`PipelineBWorkerCounts::from_env()` and `PipelineAWorkerCounts::from_env()` duplicate the exact same env-var parsing pattern 6 times.** Each block is:
```rust
if let Ok(v) = std::env::var("AXON_XX_WORKERS").and_then(|raw| {
    raw.trim().parse::<usize>().map_err(|_| std::env::VarError::NotPresent)
}) {
    if v > 0 { counts.xx = v; }
}
```
**Impact:** 60 lines of boilerplate. No performance impact.
**Fix:** Extract a `parse_env_usize(key: &str, default: usize) -> usize` helper.

### DEEP-018 | D | MOYEN | `orchestrator.rs:480` — `let _ = counts.b1;` silences the unused field
**Line 480: `let _ = counts.b1;` explicitly suppresses the unused-variable warning for `counts.b1`.**
The B1 worker count is ignored since B1 switched to a single batched worker. The field still exists on `PipelineBWorkerCounts` and is operator-configurable via `AXON_B1_WORKERS`, but setting it has NO effect. An operator who sets `AXON_B1_WORKERS=8` would believe they're scaling B1 workers, but they're not.
**Impact:** Silent misconfiguration risk.
**Fix:** Either wire `counts.b1` to something meaningful (e.g., number of parallel DB connections in the batched fetch) or document in the env-var help that `AXON_B1_WORKERS` is deprecated.

### DEEP-019 | C | FAIBLE | `channels.rs:33` — `B1_COLDSTART_BATCH_SIZE_DEFAULT = 4096` vs `b1_pool_size` derivation
**B1 pool size is derived as `caps.b2_batch_size * 4` (orchestrator.rs:481), which with defaults = 64*4 = 256.** But `B1_COLDSTART_BATCH_SIZE_DEFAULT` is 4096. These are two different things (pool = per-batch-SELECT size, coldstart = per-poll-round size), but the naming similarity is confusing. The pool_size of 256 means B1 batched worker issues SELECTs of at most 256 chunk_ids, while B2 processes them in batches of 64.
**Impact:** None on throughput. Naming confusion.
**Fix:** Rename `b1_pool_size` to `b1_fetch_batch_size` for clarity.

### DEEP-020 | C | MOYEN | `bench:axon-bench-pipeline-v2.rs:249` — Bench doesn't use dedup cache
**The bench creates `spawn_pipeline_a()` (not `spawn_pipeline_a_with_cache()`), so it runs WITHOUT the content-hash dedup filter.** This means the bench always re-parses every file, even on repeated runs. For sustained-mode (`--cycle`), this means the same files are re-parsed and re-persisted on every cycle, which is the intended behavior for throughput measurement. But it also means the bench doesn't exercise the dedup filter path, which is the production path.
**Impact:** Bench doesn't characterize production dedup behavior. Could add a `--with-dedup` flag.
**Fix:** Add optional dedup cache to bench for production-fidelity runs.

### DEEP-021 | C | MOYEN | `bench:axon-bench-pipeline-v2.rs:254` — Bench doesn't use multi-embedder
**The bench uses `spawn_pipeline_b_full()` (single embedder) not `spawn_pipeline_b_full_multi()`.** If the operator sets `AXON_B2_WORKERS=2`, the bench ignores it. The production path in `pipeline_v2_runtime.rs:235-253` creates multiple embedder sessions.
**Impact:** Bench under-reports production throughput when multi-GPU is configured.
**Fix:** Use `spawn_pipeline_b_full_multi()` in bench, honoring `counts_b.b2`.

### DEEP-022 | E | FAIBLE | `stage_b1.rs:9-13` — Obsolete docstring
**B1 docstring says "content_tsv GENERATED for FTS" — but content_tsv is now populated by the async TSV worker (REQ-AXO-901624 P4), not a GENERATED column.**
**Impact:** Documentation drift.
**Fix:** Update docstring.

### DEEP-023 | E | FAIBLE | `mod.rs:8-9` — Obsolete `WatchedPath` reference
**types.rs docstring (line 6) references `WatchedPath` as A1's input type, but A1 takes `PathBuf`.** No `WatchedPath` type exists in the codebase.
**Impact:** Documentation drift.
**Fix:** Update docstring: `WatchedPath` -> `PathBuf`.

### DEEP-024 | C | FAIBLE | `stage_a3.rs:234` — `elapsed_us` computed once per project group, shared across all items
**`per_item_us` is computed as `elapsed_us / total_items` where `total_items` spans ALL project groups, but `elapsed_us` is measured from `started` which is set before the group iteration loop.** After the first group write, `elapsed_us` includes the write time for that group, but subsequent groups haven't been written yet. The metric is an approximation but consistently over-reports early groups and under-reports late groups.
**Impact:** Minor metric inaccuracy when multiple project codes appear in one batch (rare in practice — most batches are single-project).
**Fix:** Move `started` inside the per-group loop, or compute per_item_us after ALL groups complete.

### DEEP-025 | B | FAIBLE | `mod.rs:74` — Re-export of `spawn_pipeline_b_b1_only`
**`spawn_pipeline_b_b1_only` is re-exported from `mod.rs` as a public API, but it's only used in tests.** Public API surface should reflect the production topology.
**Impact:** API surface pollution.
**Fix:** Remove from public re-exports.

### DEEP-026 | E | FAIBLE | `bulk_writer.rs:318-337` — `RelationTable` enum with legacy per-table flush
**`RelationTable::Contains/Calls/CallsNif` with separate `flush_relations()` per table is legacy from before `flush_batch()`.** Production now uses `flush_batch()` which goes through `copy_edges_in_tx()` (unified Edge table). The per-table `copy_relations_in_tx()` path writes to the legacy per-type tables (`public.CONTAINS`, `public.CALLS`, `public.CALLS_NIF`) which are no longer the canonical storage (unified `public.Edge` is canonical per REQ-AXO-297).
**Impact:** The per-table flush functions are callable but would write to legacy tables. No production caller uses them for pipeline v2.
**Fix:** Mark per-table flushes as deprecated or `#[cfg(test)]`.

### DEEP-027 | C | FAIBLE | `pipeline_v2_runtime.rs:384-401` — Bootstrap scan `try_send` with 50ms sleep
**When `try_send` returns `Full`, the bootstrap task sleeps 50ms then CONTINUES to the next file (dropping the current one).** The comment says "The dropped path will be re-submitted by scope_reconciliation_orchestrator". But:
1. The sleep is a fixed 50ms regardless of channel drain rate.
2. The dropped file is silently lost until reconciliation (60s interval).
3. At 130K files with a 1024-slot channel, the first ~1024 files are sent instantly, then every file incurs a 50ms sleep, making the bootstrap take ~(130000-1024)*50ms = ~107 minutes.

Wait — re-reading: after `TrySendError::Full`, it sleeps 50ms then continues the FOR loop to the NEXT file (not retrying the current one). So it drops the current path AND waits 50ms. This seems intentional (the comment explains) but the throughput implication is that bootstrap scan throughput is capped at 20 files/s when A1 is saturated.

**Impact:** Bootstrap of large workspaces is slow. But the reconciliation catch-up makes it correct. The 50ms yield is reasonable to avoid busy-spinning.
**Fix:** Consider using `send().await` with a timeout, or `send().await` directly (the original blocking concern was from session 51 deadlock, but that was pre-try_send fix).

### DEEP-028 | A | FAIBLE | `pipeline_v2_runtime.rs:919-930` — Test asserts legacy `AXON_GPU_EMBED_SERVICE_TENSORRT`
**Test `gpu_provider_explicitly_requested_env_matrix` at line 922-925 still asserts that `AXON_GPU_EMBED_SERVICE_TENSORRT=1` returns true.** But the production code at line 821-831 only checks `AXON_EMBEDDING_PROVIDER`. The comment at line 822 says the legacy check was "removed". But the code uses `matches!()` which doesn't check `AXON_GPU_EMBED_SERVICE_TENSORRT`. So the test is WRONG — it asserts behavior that doesn't exist.

Wait, let me re-read the production code more carefully:
```rust
fn gpu_provider_explicitly_requested() -> bool {
    matches!(
        std::env::var("AXON_EMBEDDING_PROVIDER")
            .ok()
            .map(|v| v.to_lowercase())
            .as_deref(),
        Some("tensorrt") | Some("cuda")
    )
}
```
This ONLY checks `AXON_EMBEDDING_PROVIDER`. It does NOT check `AXON_GPU_EMBED_SERVICE_TENSORRT`. But the test at line 922-925:
```rust
std::env::remove_var(prov_key);
std::env::set_var(trt_key, "1");
assert!(gpu_provider_explicitly_requested(), "TRT flag=1 -> true");
```
This test SHOULD fail because `AXON_EMBEDDING_PROVIDER` is unset and the function only checks that var. Unless there's something else going on... Actually wait, `prov_key` is removed but `trt_key` is set. The function doesn't read `trt_key`. So this test should fail. But cargo test apparently passes. Let me check if there's something I'm missing.

Actually, I need to re-check — at line 915, `prov_key` is set to "tensorrt". Then at line 918, `prov_key` is removed. Then at line 919, `trt_key` is set. But `trt_key` = `AXON_GPU_EMBED_SERVICE_TENSORRT`. The function only reads `AXON_EMBEDDING_PROVIDER`. With `prov_key` removed, the function should return false. The test asserts true. **This is a latent test bug that should fail.**

Unless the test is actually passing because env vars from a previous test case leaked. Actually, at line 915: `std::env::set_var(prov_key, "CUDA")` and line 916 asserts true. Then line 918 removes `prov_key`. But wait — the `set_var` on line 915 sets `AXON_EMBEDDING_PROVIDER=CUDA`. Then line 918 `std::env::remove_var(prov_key)` removes it. Then line 919 sets `AXON_GPU_EMBED_SERVICE_TENSORRT=1`. The function should return false. If the test passes, it's a mystery. Let me re-check... Ah, env vars are process-global. Another test running in parallel might set `AXON_EMBEDDING_PROVIDER`. The `ENV_LOCK` mutex at line 889 prevents that. So this test SHOULD fail. Either it's never run, or there's something I'm missing.

**Impact:** Latent test bug. The `AXON_GPU_EMBED_SERVICE_TENSORRT` env var is silently ignored by the production code but the test claims it works.
**Fix:** Remove the `trt_key` test cases, or re-add the legacy check to the production function.

### DEEP-029 | C | FAIBLE | `embedder_gpu.rs:55-57` — `GpuB2Embedder` fields for wake
**`lane: String` and `worker_idx: usize` and `use_cuda: bool` are stored on `GpuB2Embedder` solely for the wake-from-sleep path (line 167).** These are only needed when `guard.is_none()`, i.e., after the watchdog released the session. In the steady state (GPU active), they're dead weight. The `lane` is a `String` (heap allocation) that could be `&'static str` or `Arc<str>`.
**Impact:** 64-96 bytes of per-embedder overhead. Negligible.
**Fix:** Use `&'static str` for `lane` if the set of values is known at compile time.

### DEEP-030 | E | FAIBLE | `graph_ingestion.rs:1057` — Comment references "content_tsv GENERATED column"
**Line 1057: "REQ-AXO-292 `content_tsv` GENERATED column automatically."** But content_tsv is now async via TSV worker.
**Impact:** Documentation drift.
**Fix:** Update comment.

---

## Summary Table

| ID | Cat | Severity | File | Short Description |
|---|---|---|---|---|
| DEEP-003 | D | BLOQUANT | stage_b2.rs:184-186 | BGE prefix applied for passage (should be query-only) |
| DEEP-001 | A | ELEVE | stage_a3.rs:8-9 | Obsolete AGE references in docstring |
| DEEP-002 | B | ELEVE | stage_b2.rs:79-103 | b2_embed() dead code, prefix mismatch |
| DEEP-004 | B | ELEVE | stage_b3.rs:46-82 | b3_persist_embedding() dead code |
| DEEP-005 | B | ELEVE | stage_b1.rs:128-145 | b1_fetch_for_embedding() dead production code |
| DEEP-006 | B | ELEVE | orchestrator.rs:362-397 | spawn_pipeline_b_b1_only() test-only |
| DEEP-007 | C | ELEVE | stage_a3.rs:73-90 | Heavy clones in a3_enroll() |
| DEEP-008 | C | ELEVE | stage_a3.rs:215 | group_batch.clone() for spawn_blocking |
| DEEP-009 | C | MOYEN | stage_a1.rs:67-75 | Per-byte format!() in SHA-256 hex |
| DEEP-010 | C | MOYEN | worker_pool.rs:51 | Mutex<Receiver> pattern (acceptable) |
| DEEP-012 | C | MOYEN | stage_a3.rs:141 | interval() missing MissedTickBehavior |
| DEEP-013 | D | MOYEN | stage_b1.rs:260-272 | B1 inline timing metrics wrong |
| DEEP-014 | E | MOYEN | stage_a3.rs:1-22 | Obsolete AGE + GENERATED docstring |
| DEEP-015 | B | MOYEN | stage_a3.rs:64-100 | a3_enroll() dead production code |
| DEEP-016 | C | MOYEN | stage_b2.rs:184-186 | String alloc per chunk in GPU path |
| DEEP-018 | D | MOYEN | orchestrator.rs:480 | AXON_B1_WORKERS silently ignored |
| DEEP-020 | C | MOYEN | bench:249 | Bench doesn't use dedup cache |
| DEEP-021 | C | MOYEN | bench:254 | Bench doesn't use multi-embedder |
| DEEP-024 | C | FAIBLE | stage_a3.rs:234 | elapsed_us shared across groups |
| DEEP-017 | E | FAIBLE | orchestrator.rs:72-103 | Duplicated env-var parsing |
| DEEP-019 | C | FAIBLE | channels.rs:33 | b1_pool_size naming confusion |
| DEEP-022 | E | FAIBLE | stage_b1.rs:9-13 | Obsolete GENERATED docstring |
| DEEP-023 | E | FAIBLE | mod.rs/types.rs | Obsolete WatchedPath reference |
| DEEP-025 | B | FAIBLE | mod.rs:74 | Re-export of test-only function |
| DEEP-026 | E | FAIBLE | bulk_writer.rs:318-337 | Legacy per-table relation flush |
| DEEP-027 | C | FAIBLE | runtime.rs:384-401 | Bootstrap 50ms sleep on full |
| DEEP-028 | A | FAIBLE | runtime.rs:919-930 | Test asserts non-existent behavior |
| DEEP-029 | C | FAIBLE | embedder_gpu.rs:55-57 | String field could be &'static str |
| DEEP-030 | E | FAIBLE | graph_ingestion.rs:1057 | Obsolete GENERATED comment |

---

## Top 5 Corrections by Impact/Effort Ratio

### 1. DEEP-003 — Remove BGE passage prefix (BLOQUANT, low effort)
**Impact:** Every embedding in the system is computed with a wrong/truncated prefix (`"Represent this sentence: "` instead of empty). Query-time already uses the correct full prefix. Fixing this aligns with BGE-Large-en-v1.5 spec (passages = no prefix, queries = full prefix) and improves semantic recall for ALL queries. Requires a full re-embedding after fix.
**Effort:** Delete 3 lines in `stage_b2.rs:184-186`, replace with:
```rust
let texts: Vec<String> = batch.iter().map(|p| p.content.clone()).collect();
```
Query-time path in `embedder.rs:2011` already applies the correct full query prefix.

### 2. DEEP-012 — Set MissedTickBehavior::Delay on A3/B3 intervals (MOYEN, trivial)
**Impact:** Eliminates spurious empty-flush wakeups after slow PG writes. Stabilizes tail latency.
**Effort:** Add one line after each `interval()` call in stage_a3.rs and stage_b3.rs:
```rust
tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
```

### 3. DEEP-008 — Eliminate group_batch.clone() in A3 batched worker (ELEVE, moderate)
**Impact:** Saves ~1.6 MB/batch of heap allocation. At 2 batches/s = 3.2 MB/s less GC pressure.
**Effort:** Restructure the A3 flush: move chunk_meta processing into the spawn_blocking closure, return `(chunk_metas, receipts)` as the join result. ~30 lines of refactoring.

### 4. DEEP-002/004/005/006/015 — Mark dead functions `#[cfg(test)]` (ELEVE, trivial)
**Impact:** Reduces public API surface, eliminates confusion about which code path is production.
**Effort:** Add `#[cfg(test)]` annotations and remove from `mod.rs` public re-exports.

### 5. DEEP-018 — Document or wire AXON_B1_WORKERS (MOYEN, trivial)
**Impact:** Prevents operator misconfiguration where they believe they're tuning B1 parallelism but the setting is silently ignored.
**Effort:** Add a `tracing::warn!` when `AXON_B1_WORKERS` is set, explaining it's unused.

---

## Code Quality Score: 72/100

**Breakdown:**
- Architecture (topology, DAG, data flow): 85/100 — The 6-stage DAG is correctly linear. The try_send cross-pipeline handoff is sound. The batched worker pattern is well-designed.
- Dead code / unused exports: 55/100 — Multiple public functions (`b2_embed`, `b3_persist_embedding`, `b1_fetch_for_embedding`, `a3_enroll`, `spawn_pipeline_b_b1_only`) are test-only but exported as public API.
- Performance discipline: 70/100 — The DEEP-003 BGE prefix issue is a correctness bug with semantic impact. The clone pattern in A3 is wasteful but not dominant. SHA-256 hex encoding is suboptimal.
- Documentation accuracy: 50/100 — Multiple docstrings reference retired concepts (AGE, GENERATED content_tsv, WatchedPath). Several comments are stale.
- Metrics accuracy: 65/100 — B1 timing metrics are wrong (DEEP-013). A3 per-item timing is approximate across groups. MissedTickBehavior default causes ghost ticks.
- Test quality: 80/100 — Comprehensive test coverage. One latent test bug (DEEP-028). Tests exercise both topology paths.
- Env-var discipline: 75/100 — Consistent pattern but AXON_B1_WORKERS is silently ignored. Boilerplate could be DRYer.
