# IST Vectorization Stall Concept

## Problem

Some project documents, including `NTO` files under `docs/plans/`, are:

- discovered by watchers
- indexed structurally
- marked `graph_ready = true`
- still stuck at `vector_ready = false`

for hours, with no observable progress.

This creates a false operator impression that docs are "not watched" while the real defect is deeper in the semantic completion pipeline.

## What Is Already Proven

### Not a watch / discovery defect

Runtime `status` on `live` reports:

- `watcher_policy = full`
- `runtime_mode = full`
- `runtime_profile = full_autonomous`

`diagnose_indexing(NTO)` reports:

- `known files = 94`
- `completed files = 94`
- `pending = 0`
- `indexing = 0`
- `no_blocker_detected`

The affected `NTO` plan docs exist in `File` with:

- `status = indexed`
- `file_stage = graph_indexed`
- `graph_ready = true`
- `vector_ready = false`

### Not just "the backlog is big"

Global vector backlog is indeed large on `live`, but that alone does not explain the defect.

Observed facts:

- the affected `NTO` docs have chunks
- those chunks have zero `ChunkEmbedding`
- the affected docs have no `FileVectorizationQueue` entry
- `resume_vectorization` returns `queued_files = 0`

This means the files are not merely waiting behind a queue. They are missing from the file-level vectorization queue entirely.

## Most Likely Defect Class

The highest-confidence defect is:

- queue repair / backfill does not re-enqueue some graph-indexed, non-vector-ready files even though they still have unembedded chunks

Observed contradiction:

- the SQL condition equivalent to `backfill_file_vectorization_queue()` matches the `NTO` plan docs
- but the tool `resume_vectorization` still reports no missing backlog

That points to a real implementation bug in one of:

1. `backfill_file_vectorization_queue()` result parsing
2. backfill query execution path under runtime conditions
3. queue eligibility logic drifting from the actual stored state

## Secondary Risk

The current ingestion contract appears able to produce this invalid state:

- `File.graph_ready = true`
- `File.vector_ready = false`
- chunks exist
- no queue row exists

That state should be considered invalid for vectorizable files.

So the system likely needs:

- a stronger invariant at ingestion/finalization time
- and a reliable reconciliation path afterward

## Non-Goals

- do not redesign the whole watcher system
- do not redesign retrieval ranking in this wave
- do not rely on operator-only manual reindex as the normal fix

## Target Outcome

For any vectorizable file:

- if `graph_ready = true` and `vector_ready = false`
- and at least one chunk lacks a current embedding

then one of these must be true:

1. the file has a valid `FileVectorizationQueue` row
2. the file is currently owned by the persist/finalize path with an equivalent tracked state

Otherwise it must be considered a runtime defect and be recoverable automatically.

## Proposed Direction

1. Audit and fix the reconciliation path:
   - `resume_vectorization`
   - startup backfill
   - any periodic repair path

2. Introduce an explicit invariant checker for orphaned vectorization state:
   - `graph_ready && !vector_ready && chunks exist && no queue row`

3. Ensure repair is:
   - deterministic
   - idempotent
   - safe under load

4. Add a qualification check so this regression cannot silently return.

## Broader Resilience Position

The goal is not just to fix one stuck queue case.

The real target is a resilient end-to-end IST contract:

- watcher sees a change
- ingress promotes it durably
- pending work can always be reclaimed
- indexing can always be resumed
- vectorization can always be resumed
- projection can always be resumed

Current audit conclusion:

- watcher/discovery is mostly sound
- structural indexing restart is mostly sound
- semantic completion is not yet guaranteed

So the right solution is a layered repair model, not a one-off patch.

### Canonical Recovery Invariant

For every file, exactly one of these must be true after graph indexing:

1. `vector_ready = true`
2. the file has a durable queue/outbox ownership state proving semantic completion is in progress
3. the file is explicitly non-vectorizable and marked as such

Anything else is an orphan state and must be repairable automatically.

### Canonical Ownership Model

This wave must define one canonical semantic-completion ownership model.

For a vectorizable file, "in progress" must mean exactly one durable ownership state:

1. **queued ownership**
   - a row exists in `FileVectorizationQueue`
   - status is one of:
     - `queued`
     - `paused_for_interactive_priority`
     - `inflight`

2. **persist/finalize ownership**
   - must still remain canonically represented through the same `FileVectorizationQueue` row
   - via ownership fields such as lease owner / lease epoch / claim token
   - `VectorPersistOutbox` is adjunct execution state, not an ownership substitute

Therefore:

- the canonical ownership truth for semantic file completion is `FileVectorizationQueue`
- outbox state may support execution and recovery
- but it must not be treated as an alternative ownership model

The orphan predicate must read only from this ownership truth.

The system must never allow the same file to be simultaneously interpreted as:

- orphaned
- and in-progress

by two different surfaces.

### Canonical Non-Vectorizable Model

The invariant also requires an explicit definition of files that should not be semantically completed.

That state must not be heuristic-only.

It must be represented through canonical file truth such as:

- `status = skipped`
- `status = oversized_for_current_budget`
- another explicit terminal/non-vectorizable state

so repair logic can exclude those files deterministically.

### Stale Inflight Reclaim Rule

An `inflight` row is not healthy forever.

It becomes reclaimable when its canonical lease/heartbeat contract is stale.

This wave should treat the reclaim rule as:

- ownership remains canonical while heartbeat is fresh
- ownership becomes reclaimable when the configured stale-lease threshold is exceeded
- reclaim must be idempotent and safe under restart

This means operator truth must distinguish:

1. **orphaned files**
   - no canonical ownership row at all

2. **stale in-progress files**
   - canonical ownership exists
   - but lease freshness has expired and reclaim is required

### Solution Shape

The best solution is a three-layer correction:

1. **Invariant hardening at write time**
   - never leave a vectorizable file in `graph_ready && !vector_ready` without durable progress state

2. **Deterministic reconciliation**
   - one shared canonical reconciliation helper used by:
     - startup repair
     - `resume_vectorization`
     - periodic stale recovery

3. **Operator truth**
   - expose orphan counts explicitly so a hidden stall cannot look like healthy completion

4. **Bounded repair**
   - reconciliation must reclaim in bounded batches
   - with deterministic ordering
   - and idempotent claim semantics under concurrent load

`repair-needed` is diagnostic state only.

It can help operators understand why repair happened or is needed, but it must never count as ownership truth by itself.

### Why This Is Better Than A Narrow Patch

A narrow fix to `resume_vectorization` alone would still leave:

- silent orphan creation
- no explicit visibility
- drift between write path and repair path

The broader solution closes all three.
