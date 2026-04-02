# File Ingress Guard Design

Date: 2026-04-02
Status: validated by discussion, pending implementation
Scope: ingress filtering before scanner and watcher write into `File`

## Goal

Reduce redundant `File` upserts and silent requeues to `pending` without introducing a second source of truth or moving scheduling authority out of DuckDB.

## Problem

The current runtime shows a strong mismatch between structural truth and `File.status` truth:

- the workspace graph is already rich
- many current files still appear as `pending`
- for project `axon`, most files currently on disk are `pending` even when they already have `Chunk`, `Symbol`, and `CONTAINS`

The most plausible cause is the ordinary rescan/upsert path:

- `Scanner` walks the filesystem and calls `bulk_insert_files`
- hot watcher paths call `upsert_hot_file`
- `upsert_file_queries` can reopen rows to `pending`
- the system currently uses indirect metadata (`path`, `mtime`, `size`, `priority`, status) and does not have an ingress-side filter to prevent redundant rewrites

The result is too much churn in `File`, too much pressure on DuckDB, and poor operator truth about actual progress.

## Decisions Locked

### 1. The new component is not a second index

The name `MemoryIndex` is rejected.

The retained name is `FileIngressGuard`.

Reason:

- its job is not to become a parallel index
- its job is to sit in front of scanner/watcher writes
- it protects DuckDB from redundant upserts and unnecessary requeues

`DiscoveryGuard` remains an acceptable fallback name if the implementation reveals that `FileIngressGuard` is too tied to filesystem ingress vocabulary, but `FileIngressGuard` is the current canonical name.

### 2. DuckDB remains the only canonical truth

DuckDB stays authoritative for:

- `File.status`
- `priority`
- `worker_id`
- `needs_reindex`
- `defer_count`
- claim ordering
- all `pending -> indexing` transitions

`FileIngressGuard` is strictly derived state.

It may accelerate decisions such as:

- ignore unchanged file
- stage changed file
- stage unknown file
- tombstone missing file

It may not:

- claim files
- decide priority canonically
- change `File.status` by itself
- become the scheduler

If guard and DB disagree, DB wins and the guard is rebuilt or discarded.

### 3. Prioritization stays in DuckDB

The guard does not need to mirror the full pending order.

Reason:

- claim order is already canonical in DuckDB
- duplicating ranking in memory would create an unnecessary second scheduler
- the immediate problem is ingress noise, not claim performance

The guard may keep lightweight counters or sets for observability, but not a canonical priority queue.

### 4. No project favoritism in the canonical system

Axon is a multi-project indexer.

The design target is:

- no canonical bias for the currently opened repo
- no product rule that `axon` itself outranks the rest of the workspace

Any current preferred-project runtime bias is treated as a separate concern, not as part of this design.

### 5. The first decision filter is metadata, not content hash

MVP decision signal:

- `path`
- `mtime`
- `size`

This is enough to suppress a large amount of redundant ingress cheaply.

File content hashing is explicitly deferred out of MVP because:

- it adds filesystem I/O cost
- it is not needed to prove the ingress-guard concept
- the current failure mode is already visible with metadata-only churn

Hashing remains a later optimization candidate for cases where `mtime/size` are noisy.

### 6. The guard must fail open

If the guard is absent, stale, partially hydrated, or fails to initialize:

- scanner and watcher fall back to current behavior
- ingestion must not block globally

This keeps the first rollout low risk.

### 7. Guard updates happen only after canonical success

The guard may hydrate from DuckDB at boot.

After boot, it may only learn durable state after successful canonical DB operations.

It must not be updated:

- before a `File` write commits
- from parse success alone
- from queue admission alone

This rule prevents the guard from becoming a phantom truth source.

### 8. Boot sequence must prefer fast truth, not brute-force churn

Target boot sequence:

1. open DuckDB
2. run recovery such as `recover_interrupted_indexing`
3. hydrate `FileIngressGuard` from `File`
4. expose MCP/SQL against already persisted truth
5. arm watcher
6. run scanner with the guard enabled

This does not eliminate scanning.

It changes scanning from:

- brute-force restaging

to:

- reconciliation against known persisted file stamps

### 9. `pending` must become explainable

The investigation confirmed that current `pending` is too ambiguous.

Implementation should move toward explicit transition causes for requeue, at least conceptually:

- newly discovered
- metadata changed on scan
- hot delta changed
- recovered interrupted claim
- invalidated by drift
- changed while indexing
- requeued after write failure

This causality is required for trustworthy operations, even if the first implementation only adds probes before schema changes.

### 10. MCP needs fast per-project completeness truth

When an MCP request targets a project, Axon should be able to answer quickly with a compact truth such as:

- files known
- files indexed
- files pending
- files degraded
- files oversized
- completion ratio
- partial truth warning

This does not require the guard to be canonical.

It requires:

- trustworthy `File` truth
- cheap aggregated counters
- optionally a mirrored read-side cache later

## Guard Responsibilities

`FileIngressGuard` is responsible for one thing only:

- deciding whether an observed filesystem object should be staged into DuckDB or ignored as already known and unchanged

Its minimal API should stay narrow:

- `hydrate_from_store(store) -> guard`
- `should_stage(path, mtime, size) -> decision`
- `record_file_commit(path, mtime, size, status)`
- `record_tombstone(path)`
- `invalidate_all()`

The likely decision enum is intentionally small:

- `StageNew`
- `StageChanged`
- `SkipUnchanged`
- `RetombstoneMissing`

## Non-Goals

This tranche does not attempt to:

- move scheduling into memory
- replace DuckDB claims
- fix all MCP quality issues
- introduce content hashing
- redesign project-level prioritization globally
- redesign the dashboard

## Recommended Data Shape

Minimal shadow entry:

- `path`
- `project_slug`
- `status`
- `size`
- `mtime`
- `priority`
- `needs_reindex`
- `worker_id`
- `defer_count`
- `last_deferred_at_ms`

Derived helper:

- `stat_sig = (mtime, size)`

That is enough for ingress filtering.

It is not a graph cache.

## Integration Points

Primary Rust files involved:

- `src/axon-core/src/scanner.rs`
- `src/axon-core/src/fs_watcher.rs`
- `src/axon-core/src/graph_ingestion.rs`
- `src/axon-core/src/graph_bootstrap.rs`
- `src/axon-core/src/main_background.rs`
- `src/axon-core/src/lib.rs`

Likely new module:

- `src/axon-core/src/file_ingress_guard.rs`

## Rollout Strategy

Low-risk rollout order:

1. introduce the guard as an isolated component with tests
2. hydrate it at boot, still unused on the hot path
3. branch the watcher through it
4. branch the scanner through it
5. add telemetry for hit/miss/hydration
6. only then consider schema changes for explicit requeue cause

## Open Follow-Up Decisions

These are intentionally not locked in this document:

- whether to add a persisted `last_transition_cause` column in `File`
- whether to add file content hash later
- whether to add a special “project understanding bootstrap” priority tier for `README`, manifests, architecture docs, and concept files

The last point is promising for early MCP usefulness, but should be validated as a separate prioritization policy once ingress truth is stable.
