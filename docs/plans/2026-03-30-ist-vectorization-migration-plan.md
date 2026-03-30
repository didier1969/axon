---
title: IST Vectorization Migration Plan
date: 2026-03-30
status: proposed
branch: feat/axon-stabilization-continuation
---

# Objective

Evolve Axon from symbol-only embeddings toward a derived, versioned vectorization stack built on trusted `IST`, while preserving the physical separation and governance of `SOLL`.

# Method

This plan follows:

- `reality-first-stabilization`
- `axon-digital-thread`

Principles:

- `IST` remains the structural source of truth
- all vectorized layers are derived and disposable
- `SOLL` remains protected and independent from `IST` resets
- schema evolution must be versioned and invalidation must be explicit

# Current State

Current strengths:

- `IST` / `SOLL` separation exists
- `RuntimeMetadata` now versions `IST` compatibility
- `Symbol.embedding FLOAT[384]` exists
- background semantic worker exists

Current limits:

- embeddings are attached only to `Symbol`
- synchronous MCP embedding is intentionally disabled
- no chunk model exists
- no graph projection model exists
- no dedicated invalidation layers exist beyond coarse `embedding_version`

# Target Architecture

## Canonical Truth

Keep current canonical `IST` entities:

- `File`
- `Symbol`
- `Project`
- `CONTAINS`
- `CALLS`
- `CALLS_NIF`
- `IMPACTS`
- `SUBSTANTIATES`

Keep current canonical `SOLL` entities in `soll.*`.

## New Derived Layers

Introduce these as derived `IST`-side tables:

- `Chunk`
- `ChunkEmbedding`
- `GraphProjection`
- `GraphEmbedding`
- `EmbeddingModel`

## Design Intent

### Chunk

Represents an LLM-usable unit richer than a bare symbol.

Minimum shape:

- `id`
- `source_type` (`file`, `symbol`, `requirement`, `decision`)
- `source_id`
- `project_slug`
- `content`
- `content_hash`
- `start_line`
- `end_line`
- `kind`

### ChunkEmbedding

Represents a versioned embedding of a chunk.

Minimum shape:

- `chunk_id`
- `model_id`
- `embedding`
- `source_hash`

### GraphProjection

Represents a materialized structural neighborhood for retrieval.

Minimum shape:

- `id`
- `root_type`
- `root_id`
- `project_slug`
- `radius`
- `projection_hash`
- `summary`

### GraphEmbedding

Represents an embedding of a graph projection.

Minimum shape:

- `projection_id`
- `model_id`
- `embedding`
- `source_hash`

### EmbeddingModel

Tracks embedding provenance.

Minimum shape:

- `id`
- `kind` (`symbol`, `chunk`, `graph`)
- `model_name`
- `dimension`
- `version`
- `created_at`

# FOSS Recommendation

## Short Term

Keep the current local embedding family as bootstrap:

- `BGE Small EN v1.5`

Reason:

- already integrated
- local
- cost-efficient
- acceptable for first derived layers

## Medium Term

The main gain should come from better units and projections, not first from a bigger model.

Priority of value:

1. `Chunk`
2. `ChunkEmbedding`
3. `GraphProjection`
4. `GraphEmbedding`

# Invalidation Policy

Do not use a single global version.

Track at least:

- `schema_version`
- `ingestion_version`
- `embedding_version`
- `chunk_projection_version`
- `graph_projection_version`

Rules:

- schema drift: rebuild affected `IST` tables
- ingestion drift: rebuild structural derived layers
- chunk projection drift: rebuild `Chunk` and `ChunkEmbedding`
- graph projection drift: rebuild `GraphProjection` and `GraphEmbedding`
- embedding model drift: rebuild only affected embedding tables

`SOLL` must remain outside these resets.

# Migration Phases

## Phase 1. Stabilize Canonical IST

Goal:

- make `IST` trustworthy before adding new derived layers

Actions:

- keep current `RuntimeMetadata` discipline
- confirm live persistence of `File`, `Symbol`, `CONTAINS`, `CALLS`
- ensure rebuilds of `IST` are safe and deterministic

Exit criteria:

- live Axon on Axon returns structurally correct symbol and call results

## Phase 2. Introduce Chunk

Goal:

- create the first LLM-oriented retrieval unit

Actions:

- add `Chunk`
- derive chunks from indexed files and/or symbols
- define chunk hashing and invalidation
- add tests for chunk regeneration

Exit criteria:

- chunks are reproducible and tied to stable source hashes

## Phase 3. Introduce ChunkEmbedding

Goal:

- make retrieval useful before graph vectorization

Actions:

- add `EmbeddingModel`
- add `ChunkEmbedding`
- reuse current local model first
- add background worker for chunk embeddings

Exit criteria:

- at least one retrieval workflow is more useful with chunk embeddings than with symbol-only lookup

## Phase 4. Introduce GraphProjection

Goal:

- materialize structural neighborhoods explicitly

Actions:

- define projection radius and serialization strategy
- build projection hashes from current graph neighborhoods
- store projection summaries

Exit criteria:

- projections are deterministic and invalidated correctly when source graph changes

## Phase 5. Introduce GraphEmbedding

Goal:

- improve context awareness for LLM-assisted development

Actions:

- embed graph projections
- use them only as optional retrieval boosters
- keep structural retrieval as the authority layer

Exit criteria:

- graph-aware retrieval improves at least one validation en conditions reelles scenario

# Data Model Constraints To Add

Recommended additions over time:

- primary keys on all new derived entities
- unique constraints on `(chunk_id, model_id)` and `(projection_id, model_id)`
- indexes on:
  - `Chunk.source_id`
  - `Chunk.project_slug`
  - `GraphProjection.root_id`
  - `EmbeddingModel.kind`

# Validation En Conditions Reelles

The migration is only successful if it improves real use.

Priority scenarios:

1. find relevant code context faster than structural symbol search alone
2. recover neighboring implementation context for a target symbol
3. recover useful conceptual context around a `Requirement` or `Decision`
4. keep results explainable through structural fallback

Success condition:

- vectorized retrieval helps
- but structural truth remains the final arbitration layer

# Recommended Order

1. stabilize live `IST`
2. add `Chunk`
3. add `ChunkEmbedding`
4. validate value in real use
5. only then add `GraphProjection` and `GraphEmbedding`

# Non-Goals

Do not:

- replace structural search with pure vector search
- make `SOLL` depend on embeddings
- attach all semantics to `Symbol.embedding` forever
- introduce non-versioned derived layers
