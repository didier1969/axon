# Ingress Buffer Design

Date: 2026-04-02
Status: validated by discussion, pending implementation
Scope: absorb raw watcher/scanner discovery in memory before promoting canonical `File` changes into DuckDB

## Goal

Stop writing every raw filesystem discovery directly into DuckDB.

The new target is:

- filesystem discovery stays noisy and fast
- ingress reduction happens in memory
- only reduced, deduplicated, stage-worthy decisions are promoted to DuckDB in batches
- DuckDB remains the only canonical truth for file status and scheduling

## Problem

Current Axon still mixes two very different concerns:

- raw detection of filesystem events
- canonical decision that a file should become `pending` in `File`

This creates avoidable pressure:

- the scanner can call `bulk_insert_files` too eagerly
- the watcher can stage hot deltas too eagerly
- `File.status` is polluted by ingress churn
- DuckDB absorbs too many small writes from raw discovery instead of fewer canonical batch decisions

The `FileIngressGuard` already helps suppress some unchanged file churn, but it operates at file granularity and does not by itself provide a real ingress buffer between discovery and canonical state.

## Decisions Locked

### 1. The new component is memory-only in MVP

No durable ingress WAL or second database in MVP.

Reason:

- if Axon crashes, the watcher and scanner can rebuild discovery state after restart
- canonical truth still lives in DuckDB
- the first problem to solve is live ingress pressure, not crash replay of raw discovery events

### 2. DuckDB stays canonical

DuckDB remains authoritative for:

- `File.status`
- `File.priority`
- `status_reason`
- `pending -> indexing` claims
- scheduler ordering
- all structural truth already stored in IST

The ingress buffer is not a second truth source.

### 3. The new component is an ingress buffer, not a scheduler

The retained names are:

- `IngressBuffer` for the in-memory collapse buffer
- `IngressPromoter` for the background loop that flushes reduced decisions to DuckDB

They may:

- merge repeated discoveries for the same path
- preserve highest observed priority
- collapse bursts
- batch canonical updates

They may not:

- claim work
- decide canonical scheduler order
- finalize `indexing`
- become the execution queue

### 4. `FileIngressGuard` remains useful

The buffer does not replace `FileIngressGuard`.

The roles become:

- `FileIngressGuard`: cheap derived filter against already committed `File`
- `IngressBuffer`: absorbs raw discovery bursts and coalesces them in memory
- `IngressPromoter`: moves reduced decisions into DuckDB in controlled batches

### 5. Directory watcher events must not recursively restage whole subtrees directly

Directory-level watcher events should become hints, not immediate recursive hot staging.

The canonical behavior target is:

- file event -> enqueue file candidate
- missing path -> enqueue tombstone candidate
- directory event -> enqueue `subtree_hint`, then let the reducer/promoter decide whether and how to scan that subtree safely

### 6. Promotion to DuckDB must be batched

Raw ingress must not disturb DuckDB every few milliseconds.

The promoter should flush using a hybrid policy such as:

- short flush window for hot deltas
- larger batch windows for scan bulk
- immediate flush only when explicitly required by a high-priority interactive condition

Exact thresholds remain implementation detail, but batching is a design invariant.

### 7. `pending` is written only after promotion

A file should become canonical `pending` only when the promoter decides that the current reduced ingress state merits a DB update.

This means:

- watcher/scanner discovery alone is not enough
- promotion produces the canonical write
- scheduler and claims stay unchanged after that point

### 8. Crash model is acceptable without durable ingress replay

If Axon crashes:

- the in-memory ingress buffer is lost
- DuckDB still retains canonical truth
- a new startup scan and watcher rearm rebuild discovery pressure
- `FileIngressGuard` hydrates from DuckDB and starts filtering immediately again

This is acceptable for MVP.

### 9. The execution queue stays separate

The existing execution queue remains:

- `IngressBuffer` is not `QueueStore`
- `QueueStore` remains the memory queue for already-claimed executable work

The pipeline target becomes:

```text
Watcher/Scanner
  -> IngressBuffer
  -> IngressPromoter
  -> DuckDB File
  -> claim
  -> QueueStore
  -> workers
```

## Target Flow

```text
Filesystem
   |
   +--> Watcher
   |
   +--> Scanner
          |
          v
IngressBuffer (memory only)
  - keyed by path
  - last observed metadata
  - highest observed priority
  - cause/source
  - dirty flag
  - optional subtree hints
          |
          v
IngressPromoter
  - deduplicates
  - collapses bursts
  - consults FileIngressGuard
  - writes canonical batch to DuckDB
          |
          v
DuckDB File (canonical truth)
  - pending
  - indexing
  - indexed
  - degraded
  - deleted
          |
          v
Claim Scheduler -> QueueStore -> Workers
```

## MVP Data Model

Suggested in-memory event shadow:

```text
IngressEvent
  path
  mtime
  size
  priority
  source = watcher | scan
  cause = discovered | modified | deleted | subtree_hint
  dirty
  first_seen_at
  last_seen_at
```

Suggested buffer state:

```text
IngressBuffer
  by_path: HashMap<PathBuf, IngressEvent>
  subtree_hints: HashMap<PathBuf, IngressHint>
  dirty_paths: HashSet<PathBuf>
```

Priority rule in memory:

- keep the highest observed priority for a path
- do not mirror canonical scheduler order

## Non-Goals

This tranche does not attempt to:

- make ingress replay durable on disk
- remove `File.status` from DuckDB
- replace `QueueStore`
- redesign claim policy
- solve MCP quality problems unrelated to ingress
- solve memory release after peak
