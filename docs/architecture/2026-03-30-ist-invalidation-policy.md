# IST Invalidation Policy

## Purpose

This document defines how Axon decides between:

- additive repair
- soft invalidation
- hard rebuild

for `IST`.

`SOLL` is never affected by these operations unless explicitly requested.

## Principles

### 1. Preserve restart speed when truth can be preserved

`File` and `Project` are the restart anchors.
If Axon can preserve them safely, it should.

### 2. Purge derived truth before purging canonical file backlog

`Symbol`, structural relations, `Chunk`, and embedding layers are derived.
They should be invalidated before considering a full `IST` rebuild.

### 3. Reserve hard rebuild for real incompatibility

Hard rebuild is justified only when the base `File` schema or global runtime invariants are no longer safely readable by the current runtime.

## Dimensions

### Schema version

`schema_version` governs base storage compatibility.

- If the drift is repairable through additive migration:
  - apply additive repair
  - preserve `File` rows
- If the base `File` schema is incompatible after additive repair:
  - perform hard rebuild

### Ingestion version

`ingestion_version` governs structural derivation truth.

When it drifts:

- preserve `File` and `Project`
- purge derived structural layers:
  - `Symbol`
  - `CONTAINS`
  - `CALLS`
  - `CALLS_NIF`
  - `IMPACTS`
  - `SUBSTANTIATES`
  - `Chunk`
  - `ChunkEmbedding`
  - `EmbeddingModel`
- requeue files by setting `File.status = 'pending'`

This is a soft invalidation, not a hard rebuild.

### Embedding version

`embedding_version` governs semantic/vector compatibility only.

When it drifts:

- preserve `File`
- preserve `Project`
- preserve `Symbol`
- preserve `Chunk`
- purge only:
  - `ChunkEmbedding`
  - `EmbeddingModel`
- clear `Symbol.embedding`

This is also a soft invalidation.

## Current Operational Rule

At boot:

1. run additive schema repair first
2. inspect runtime metadata
3. if the base `File` schema is incompatible, hard rebuild `IST`
4. else if `ingestion_version` drift exists, soft-invalidate derived structural layers
5. else if `embedding_version` drift exists, soft-invalidate embedding layers only
6. rewrite `RuntimeMetadata` to current expected values

## Expected Outcomes

- compatible restart preserves existing `File` rows
- additive drift does not replay the whole universe
- ingestion drift requeues files without erasing the durable file backlog
- embedding drift does not destroy structural truth
- hard rebuild remains exceptional and explicit
