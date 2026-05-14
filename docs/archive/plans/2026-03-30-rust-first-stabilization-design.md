# Rust-First Stabilization Design

> **Context:** validated after architecture discussion. Rust is the canonical runtime and ingestion authority. Elixir/Phoenix remains the visualization and operator plane only.

## Goal

Provide the safest execution sequence to turn Axon into:

1. a tool Didier can use daily on real projects,
2. a technically coherent product,
3. a future commercializable system without double control-plane debt.

## Design Choice

The execution model is **risk-first by waves**, not a flat backlog.

Each wave has:

- one dominant objective,
- one canonical validation signal,
- one doc update,
- one clean commit boundary.

Side work is allowed only if it does not threaten the current runtime truth.

## Why This Sequencing

Axon is no longer in rescue mode, but it is still in architectural convergence.

The largest remaining risks are:

- split runtime authority between Rust and Elixir,
- incorrect incremental recovery under restart or hot deltas,
- overloading `IST` before the planner/backpressure model is fully stabilized,
- shipping LLM-facing semantic features before the structural substrate is complete,
- treating `SOLL` as conceptually rich but operationally under-governed.

Because of that, the sequence must be:

1. canonical runtime first,
2. recovery correctness second,
3. conceptual governance third,
4. semantic enrichment fourth,
5. product polish last.

## Execution Waves

### Wave 1. Canonical Rust Runtime

Rust becomes the only authority for:

- file discovery,
- `Axon Ignore`,
- eligibility,
- staging into `IST`,
- claiming,
- scheduling,
- backpressure,
- indexing,
- embeddings,
- MCP / SQL truth.

Elixir remains present but non-authoritative for ingestion.

### Wave 2. Delta Restart And Recovery Correctness

Axon must restart on truth, not on habit.

The restart model must become:

- delta replay by default,
- additive `IST` repair when possible,
- full `IST` rebuild only when compatibility truly requires it,
- no destructive path toward `SOLL`.

### Wave 3. Adaptive Ingestion Completion

The ingestion engine must become fully adaptive:

- hot set first,
- cold universe second,
- semantic work only on slack,
- live service protected before throughput vanity.

### Wave 4. SOLL Governance Completion

`SOLL` becomes operationally trustworthy through:

- executable invariants,
- fuller restore of links and metadata,
- better reviewability and versionable projections,
- explicit traceability to `IST`.

### Wave 5. LLM Value Layers

Once structure is trustworthy:

- `Chunk`,
- `ChunkEmbedding`,
- `GraphProjection`,
- `GraphEmbedding`.

The rule remains:

- graph semantics are derived,
- never primary truth.

### Wave 6. Product Consolidation

Only after the above:

- retire obsolete Python paths,
- finish Elixir de-authoring,
- improve operator UX,
- tighten security,
- prepare packaging and commercialization.

## Operational Rules

- No new ingestion authority in Elixir.
- No semantic feature lands without clear invalidation semantics.
- No `SOLL` mutation path outside the governed manager/export/restore model.
- No success claim without a runtime proof and a test proof.
- No milestone remains undocumented or uncommitted.

## Exit Criteria

The sequence is complete when:

- Rust is the sole ingestion/runtime authority,
- restart and delta replay are trustworthy,
- `SOLL` has executable governance,
- Axon gives measurable LLM value on real projects,
- Elixir acts only as visualization/operator surface,
- the remaining work is product polish, not architectural correction.
