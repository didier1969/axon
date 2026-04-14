# Phase 2 Pipeline Semantics And Dashboard

## Purpose

This document captures the Phase 2 operator model that now exists in the runtime and dashboard.

It is intentionally concrete:

- Rust remains the canonical runtime and mutation plane
- MCP mutations remain job-based
- the dashboard remains read-only
- `SOLL` remains protected
- `IST` remains rebuildable

## Primary Product Path

The primary product path is:

1. file discovery
2. pending backlog
3. indexing
4. structural graph readiness
5. semantic coverage
6. SOLL alignment

`Graph Embeddings` are now explicitly secondary.
They may remain useful for advanced analysis, but they are not a primary operator KPI and they are not on the critical path for time-to-usefulness.

## Fast Lane And Slow Lane

Axon keeps a two-speed design.

### Fast lane

The fast lane is what should become available early:

- file discovery
- structural extraction
- chunk derivation
- graph-ready truth

This is the lane operators should use to answer:

- Is the file known?
- Is it still waiting?
- Is it actively indexing?
- Did structural truth land?

### Slow lane

The slow lane enriches already-usable structure:

- chunk embeddings
- derived semantic coverage
- advanced graph embedding signals

Slow-lane lag must not be confused with ingestion failure.

## Canonical File Lifecycle

Per-file truth is carried by:

- `status`
- `file_stage`
- `status_reason`
- `last_error_reason`
- `graph_ready`
- `vector_ready`

### Active non-terminal states

- `pending`
- `indexing`

### Terminal states

- `indexed`
- `indexed_degraded`
- `skipped`
- `deleted`
- `oversized_for_current_budget`

`oversized_for_current_budget` is terminal for the current runtime envelope and therefore counts as completed in operator progress.
It may still reopen later if a new scan or a different budget envelope makes the file admissible.

## Meaning Of Key Flags

### `graph_ready`

`graph_ready=true` means the structural graph for the file is available and queryable.
This is the main fast-lane readiness signal.

### Semantic coverage

The dashboard uses `Files With Semantic Coverage (Derived)` as the main semantic KPI.
It is derived from graph-ready files whose current chunks have no missing embeddings for the active chunk model.

This is deliberately different from the raw `File.vector_ready` flag.

### `vector_ready`

`File.vector_ready` remains in the schema for compatibility and low-level state carry.
It is not the main operator metric.
The dashboard shows it only as an advanced raw flag.

## Requeue Semantics

Pending/indexing oscillation must not happen silently.

Phase 2 keeps requeue causes explicit through `status_reason`.
The concrete pathological loop observed as `requeued_after_writer_batch_failure` was traced to NUL-byte content crossing the SQL `CString` boundary during chunk persistence.

The runtime now canonicalizes embedded NUL bytes before SQL serialization, which prevents valid files from requeueing forever due to query encoding failure.

## Dashboard Contract

The dashboard is observational only.

It now prioritizes:

- a flow-oriented pipeline view
- compact summary counters
- active backlog reasons
- clear separation between terminal truth, structural truth, derived semantic truth, and advanced signals

The main visualization is an ECharts flow map centered on:

- Known Files
- Terminal
- Indexing
- Pending
- Indexed / Indexed Degraded / Skipped / Deleted / Oversized
- Files Graph Ready
- Nodes / Links
- Files With Semantic Coverage (Derived)
- Chunk Embeddings
- Graph Embeddings (Advanced)

## Out Of Scope For This Phase

- making graph embeddings a first-class KPI again
- dashboard mutation controls
- destructive `SOLL` changes
- GPU graph parsing work
- broad runtime rewrites unrelated to the verified bottlenecks
