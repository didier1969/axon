---
title: FOSS Vectorization Migration Plan
date: 2026-03-30
status: draft
branch: feat/axon-stabilization-continuation
---

# Intent

Move Axon from symbol-only semantic decoration toward a derived, graph-aware retrieval stack that stays fully FOSS, preserves `IST` as structural truth, and keeps `SOLL` protected.

# Decision Summary

The target architecture is:

1. `IST` remains the canonical structural truth.
2. `SOLL` remains separate and protected.
3. semantic/vector layers become derived projections, not source truth.
4. the first useful vector layer is `Chunk`, not `GraphEmbedding`.
5. graph-aware embeddings come only after chunk retrieval is stable and valuable in validation en conditions reelles.

# Current State

Today Axon has:

- `Symbol.embedding FLOAT[384]` in `IST`
- a background semantic worker using `BGE Small EN v1.5`
- synchronous MCP embedding disabled in safe mode
- structural search still carrying most of the actual value
- no dedicated chunk layer
- no dedicated graph projection layer
- no explicit embedding model registry beyond runtime code assumptions

This is acceptable as a bootstrap, but not as the target design.

# Principles

## Structural Truth First

Never let embeddings become the canonical answer.

- `File`, `Symbol`, `CONTAINS`, `CALLS`, `CALLS_NIF` remain the base truth.
- vector similarity is a retrieval aid, not a replacement for structure.

## Derived Layers Are Disposable

The following layers must be rebuildable:

- `SymbolEmbedding`
- `Chunk`
- `ChunkEmbedding`
- `GraphProjection`
- `GraphEmbedding`

If compatibility drifts, Axon must invalidate and recompute them without threatening `SOLL`.

## Validation Before Sophistication

Do not ship graph-aware vectorization because it sounds strong.

Ship it only if validation en conditions reelles shows that it improves:

- symbol discovery
- impact reasoning
- context assembly for LLM work
- project steering against raw `rg` + file reading

# FOSS Recommendation

## Base Model Strategy

Keep the first implementation fully local and FOSS.

Recommended progression:

1. keep `BAAI/bge-small-en-v1.5` as the low-risk bootstrap
2. validate pipeline quality before changing model class
3. consider a stronger FOSS model only after chunk retrieval is proven useful

The main expected gain is not a larger model.
The main expected gain is better units of representation.

## Retrieval Units

Priority order:

1. `Chunk`
2. `Symbol`
3. `GraphProjection`
4. optional `SOLL` conceptual embeddings

This order is intentional:

- symbol names alone are too poor
- chunks are much closer to what an LLM actually needs
- graph projections only become useful once chunk-level retrieval is already grounded

# Target Data Model

## Keep Existing Core

Keep:

- `File`
- `Symbol`
- `Project`
- `CONTAINS`
- `CALLS`
- `CALLS_NIF`
- `IMPACTS`
- `SUBSTANTIATES`
- `RuntimeMetadata`
- `soll.*`

## Add Derived Projection Tables

### `EmbeddingModel`

Purpose:

- register the active embedding families and their compatibility surface

Suggested columns:

- `id`
- `kind` (`symbol`, `chunk`, `graph`, `soll`)
- `model_name`
- `dimension`
- `version`
- `created_at`
- `metadata`

### `Chunk`

Purpose:

- store LLM-meaningful textual units derived from `IST`

Suggested columns:

- `id`
- `source_type` (`file`, `symbol`, `requirement`, `decision`)
- `source_id`
- `project_slug`
- `kind`
- `content`
- `content_hash`
- `start_line`
- `end_line`
- `metadata`

### `SymbolEmbedding`

Purpose:

- version symbol-level embeddings without overloading `Symbol`

Suggested columns:

- `symbol_id`
- `model_id`
- `source_hash`
- `embedding`
- `updated_at`

### `ChunkEmbedding`

Purpose:

- become the main retrieval surface for LLM context assembly

Suggested columns:

- `chunk_id`
- `model_id`
- `source_hash`
- `embedding`
- `updated_at`

### `GraphProjection`

Purpose:

- store a stable derived representation of local graph neighbourhoods

Suggested columns:

- `id`
- `root_type`
- `root_id`
- `project_slug`
- `radius`
- `projection_hash`
- `summary`
- `metadata`

### `GraphEmbedding`

Purpose:

- enable graph-aware semantic retrieval once structural graph quality is good enough

Suggested columns:

- `projection_id`
- `model_id`
- `source_hash`
- `embedding`
- `updated_at`

# Versioning And Invalidation

Use explicit runtime compatibility keys.

## Structural Compatibility

- `schema_version`
- `ingestion_version`

## Derived Projection Compatibility

- `chunk_projection_version`
- `graph_projection_version`

## Semantic Compatibility

- `embedding_version`
- `embedding_model_version`

## Invalidation Rules

### Rebuild `IST`

Required when:

- schema drift changes canonical tables
- ingestion semantics change canonical graph truth

### Rebuild `Chunk`

Required when:

- chunk segmentation logic changes
- file-to-symbol mapping changes
- source content hash changes

### Rebuild `ChunkEmbedding`

Required when:

- chunk content changes
- embedding model or dimension changes
- chunk projection version changes

### Rebuild `GraphProjection`

Required when:

- graph neighbourhood construction changes
- relevant `CALLS` / `CONTAINS` semantics change
- graph radius or projection rules change

### Rebuild `GraphEmbedding`

Required when:

- graph projection changes
- embedding model changes

# Migration Phases

## Phase A. Stabilize Current Semantic Layer

Goal:

- stop treating `Symbol.embedding` as the long-term design

Actions:

- keep current background semantic worker operational
- document it as bootstrap-only
- add explicit metadata/versioning for semantic compatibility

Exit:

- no ambiguity about current semantic layer status

## Phase B. Introduce `Chunk`

Goal:

- create useful retrieval units from trusted `IST`

Actions:

- define chunking rules per language and entity type
- start with code chunks derived from symbols and line ranges
- guarantee stable IDs and hashes

Exit:

- chunk table populated deterministically
- chunk regeneration works from `IST`

## Phase C. Introduce `ChunkEmbedding`

Goal:

- make chunk retrieval the primary semantic recall path

Actions:

- register active model in `EmbeddingModel`
- embed chunks asynchronously
- expose chunk retrieval to MCP behind truthful wording

Exit:

- chunk retrieval is measurable and useful in validation en conditions reelles

## Phase D. Introduce `GraphProjection`

Goal:

- build graph-aware local context packets without making them canonical truth

Actions:

- define projection roots:
  - symbol
  - file
  - requirement
  - decision
- define radius and projection serialization

Exit:

- graph projections are deterministic and rebuildable

## Phase E. Introduce `GraphEmbedding`

Goal:

- improve context awareness for LLM workflows beyond textual chunk similarity

Actions:

- embed graph projections
- compare graph-aware retrieval against chunk-only retrieval
- keep it behind explicit capability boundaries

Exit:

- graph embeddings produce measurable gain over chunk-only retrieval

## Phase F. Optional Conceptual Embeddings For `SOLL`

Goal:

- help retrieve related `Requirement`, `Decision`, `Concept`, and `Validation`

Actions:

- only after `SOLL` invariants and restore coverage are stronger
- embed conceptual items as a derived layer, never as source truth

Exit:

- improved project steering without weakening `SOLL` governance

# Validation En Conditions Reelles

The migration is only successful if it wins on real scenarios.

## Required Scenarios

### `VCR-1`

Chunk retrieval must beat symbol-only retrieval for natural navigation prompts.

### `VCR-2`

Impact reasoning must improve context assembly, not just return more text.

### `VCR-3`

Graph projections must help detect architectural drift better than structural-only browsing.

### `VCR-6`

Any semantic/audit output must remain explicit about confidence and derivation.

## Success Threshold

Do not keep a new vector layer unless it is at least:

- correct
- repeatable
- faster or more complete than structural-only fallback

# Recommended Execution Order

1. finalize `IST` stability and compatibility handling
2. add `Chunk`
3. add `ChunkEmbedding`
4. validate chunk retrieval on Axon itself
5. only then add `GraphProjection`
6. only then add `GraphEmbedding`

# Recommendation

For Axon, the optimal FOSS direction is not “use a bigger embedding model”.

It is:

1. trusted structural graph
2. derived chunk layer
3. chunk embeddings as primary semantic retrieval
4. graph projections
5. graph embeddings only when they prove added value
