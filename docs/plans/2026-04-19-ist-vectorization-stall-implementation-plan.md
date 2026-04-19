# IST Vectorization Stall Implementation Plan

## Objective

Fix the runtime defect where files can remain:

- `graph_ready = true`
- `vector_ready = false`
- with chunks present
- but with no file vectorization queue entry

and therefore never complete semantic indexing.

## Validation Matrix

### Runtime truth to preserve

- watcher discovery still works
- structural indexing still works
- no destructive resets of healthy queue entries
- no duplicate queue rows

### Runtime truth to restore

- orphaned vectorizable files are re-enqueued automatically or via explicit repair
- `resume_vectorization` reflects real backlog truth
- affected docs eventually transition to `vector_ready = true`

## Plan

### 1. Reproduce in code

Add a regression test for the invalid orphan state:

- insert a `File` with:
  - `status = indexed`
  - `file_stage = graph_indexed`
  - `graph_ready = true`
  - `vector_ready = false`
- insert matching `Chunk` rows without `ChunkEmbedding`
- ensure no `FileVectorizationQueue` row exists
- assert that `backfill_file_vectorization_queue()` re-enqueues it

Also add a server-level regression test for:

- `resume_vectorization`
- returning a non-zero `queued_files` count for that state

Add a resilience-level regression test for:

- restart with files left in:
  - `indexing`
  - vector `inflight`
  - projection `inflight`
  - orphaned `graph_ready && !vector_ready && no queue`
- after startup repair, the system must return to a stable claimable state

### 2. Audit backfill implementation

Inspect:

- `GraphStore::backfill_file_vectorization_queue()`
- any parsing of writer-query results
- queue upsert path
- differences between startup backfill and MCP-triggered backfill

Expected outcome:

- determine why the live-equivalent state returns `0` even when SQL eligibility matches

### 3. Fix reconciliation path

Preferred fix order:

1. correct the backfill query / parsing / queue-upsert path
2. if needed, add a stronger eligibility helper shared by:
   - startup backfill
   - `resume_vectorization`
   - any future reconciliation probe

Concrete design:

- introduce one canonical helper on `GraphStore`, conceptually:
  - `reconcile_orphaned_file_vectorization_state()`
- it should detect vectorizable files that are:
  - `graph_ready = true`
  - `vector_ready = false`
  - with missing current chunk embeddings
  - with no queue row
  - and no equivalent outbox-owned completion state
- and enqueue them idempotently

Required model tightening:

- define the exact ownership truth this helper reads:
  - queue ownership states only, through `FileVectorizationQueue`
  - persist/finalize remains represented by lease ownership on that same row
  - `VectorPersistOutbox` must not become a second ownership truth
- define the exact exclusion truth it reads:
  - skipped
  - oversized
  - deleted
  - other explicit non-vectorizable states
- do not let the helper infer ownership from soft heuristics

### 4. Harden ingestion invariant

Where a file becomes `graph_indexed` and `vector_ready = false`, ensure:

- vectorizable files are queued in the same wave
- or a repair event is emitted immediately

If a file cannot be queued, the runtime should not silently leave it in an orphan vectorization state.

Concrete requirement:

- if queue upsert fails or is skipped for a vectorizable file, the file must carry an explicit repair-needed status reason
- no silent success path is allowed
- `repair-needed` is diagnostic only; it is not a substitute for queue ownership

Crash-window requirement:

- test and, where needed, harden the following windows:
  1. file marked `graph_ready` before queue/outbox durability
  2. queue/outbox durability before file-state transition
  3. worker crash after claim but before finalize
  4. finalize handoff crash between vector worker and persist/finalize ownership

Stale-ownership requirement:

- the reclaim rule for stale `inflight` ownership must be explicit and shared with runtime recovery
- it must use the durable lease freshness contract already tracked on queue rows
- tests must prove reclaim is safe, bounded, and idempotent

### 5. Add observability

Expose a precise count for orphaned vectorization files in operator truth, preferably via:

- `status`
- or `health` / `diagnose_indexing`

Recommended metric:

- orphan_vectorization_files

Definition:

- `graph_ready = true`
- `vector_ready = false`
- missing current chunk embeddings
- no queue entry

Also expose, if possible:

- orphan_vectorization_examples
- last_orphan_repair_at_ms
- last_orphan_repair_count

And the health/status contract must distinguish:

- healthy progressing backlog
- repairable orphan backlog
- stale in-progress backlog pending reclaim

Precise metric split:

- `orphan_vectorization_files`
  - `graph_ready = true`
  - `vector_ready = false`
  - missing current chunk embeddings
  - no canonical `FileVectorizationQueue` ownership row

- `stale_vector_inflight_files`
  - canonical queue ownership exists
  - stale lease/heartbeat threshold exceeded

### 6. Re-qualify on live-like data

Validation steps:

1. run targeted Rust tests
2. run the repair path on `dev`
3. verify affected docs gain queue entries
4. verify they become `vector_ready = true`
5. verify retrieval improves on those docs after semantic completion
6. restart `dev` mid-pipeline and verify the system converges back to stable progress

Additional mandatory proofs:

7. repeated reconciliation is idempotent
8. no duplicate queue rows are emitted under concurrent repair + normal enqueue
9. no duplicate embeddings are emitted for the same chunk/hash/model
10. non-vectorizable files are not falsely reclaimed
11. stale inflight files are reclaimed after the configured threshold and not before

## Risks

- duplicate requeue if ownership detection is incomplete
- masking a deeper finalize/outbox bug if repair is too broad
- load spike if reconciliation suddenly requeues too much backlog at once

## Rollout Strategy

1. implement test-first
2. validate on `dev`
3. promote with release preflight
4. verify on `live`:
   - orphan count decreases
   - target docs gain semantic readiness

## Done Criteria

- regression tests cover the orphan state
- `resume_vectorization` re-enqueues real orphaned files
- target `NTO` docs no longer remain indefinitely at `graph_ready=true` / `vector_ready=false`
- operator surfaces can distinguish:
  - watch/index healthy
  - vectorization stalled
- startup recovery and explicit repair use the same canonical reconciliation logic
- no silent orphan state remains reachable from the normal write path
- bounded repair behavior is defined and tested
- the same file cannot be simultaneously reported as orphaned and in-progress
- canonical semantic ownership is represented only by `FileVectorizationQueue`

## Scope Discipline

This wave is centered on file semantic completion.

It may reuse existing projection recovery mechanisms, but it must not broaden into a projection redesign unless a direct causal dependency is proven.
