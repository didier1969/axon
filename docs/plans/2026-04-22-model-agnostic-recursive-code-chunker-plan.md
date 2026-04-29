# Model-Agnostic Recursive Code Chunker Plan

> ID to Delivery strict. Holistic-first. No local symptom patching.

**Goal:** Replace the current one-symbol-one-chunk + tokenizer truncation behavior with a model-agnostic code chunker that preserves whole symbols when they fit, and recursively sub-chunks only oversized symbols for vectorization quality.

**Current Reality**

- Today, code chunking is symbol-granular, not token-budget-aware.
- [graph_ingestion.rs](/home/dstadel/projects/axon/src/axon-core/src/graph_ingestion.rs:1154) builds a single chunk from `start_line..end_line`.
- [graph_ingestion.rs](/home/dstadel/projects/axon/src/axon-core/src/graph_ingestion.rs:2629) inserts one `Chunk` row per symbol.
- [embedding_contract.rs](/home/dstadel/projects/axon/src/axon-core/src/embedding_contract.rs:6) sets `MAX_LENGTH = 512` for the current model.
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:814) truncates tokenizer input to the active max length.

**Architecture Target**

`file -> parser symbol -> chunk policy -> fast-path accept if clearly small -> token-aware verification near threshold -> recursive structural split only if oversized -> emit one or many vector chunks for the same symbol -> embed`

**Key Principles**

1. The chunker must not be hard-coded to `512`.
2. Symbol extraction stays canonical and whole.
3. Vector chunks become a derived representation of a symbol, not the canonical symbol itself.
4. Small symbols must stay cheap:
   - no recursive split
   - no heavy tokenization if clearly under budget
5. Large symbols must avoid blind truncation.
6. Subchunks must preserve retrieval context:
   - qualified symbol name
   - kind
   - signature/docstring when available
   - part `i/n`

**External References**

- BGE large EN v1.5 model card:
  - https://huggingface.co/BAAI/bge-large-en-v1.5
- LlamaIndex `CodeSplitter`:
  - https://docs.llamaindex.ai/en/stable/api_reference/node_parsers/code/
- LangChain recursive/code splitters:
  - https://reference.langchain.com/python/langchain-text-splitters/python/PythonCodeTextSplitter
- Cohere chunking guidance:
  - https://docs.cohere.com/page/chunking-strategies

## Tranche A: Establish Model-Aware Chunking Contract

**Purpose:** Create the reusable policy surface that any embedding model can drive.

**Files**
- Add: `src/axon-core/src/code_chunker.rs`
- Modify: `src/axon-core/src/embedding_contract.rs`
- Modify: `src/axon-core/src/lib.rs`

**Implementation**

1. Introduce an embedding chunk profile abstraction:
   - `model_max_tokens`
   - `target_chunk_tokens`
   - `overlap_tokens`
   - `small_symbol_char_fast_path`
   - `gray_zone_char_threshold`
2. Default the current profile from the active embedding contract.
3. Keep the profile model-agnostic so future models only change the profile values, not the algorithm.

**Exit criteria**

- A single API exists to ask:
  - what the hard token cap is
  - what the target chunk size is
  - what fast-path thresholds apply

## Tranche B: Add Recursive Symbol Chunker

**Purpose:** Replace single-snippet chunk building with a symbol-aware chunk emission pipeline.

**Files**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Add if needed: `src/axon-core/src/parser/chunk_support.rs`
- Modify if needed: parser symbol/property helpers in language parsers

**Implementation**

1. Add a chunk builder that emits `Vec<DerivedCodeChunk>` instead of a single chunk string.
2. Fast path:
   - if symbol is clearly below the fast-path threshold, emit one chunk directly.
3. Gray zone:
   - estimate or measure token length with the active tokenizer-aware policy.
4. Oversized path:
   - recursively split by structure when possible
   - fallback by line windows with light overlap when structural subdivision is unavailable
5. Emit chunk ids as stable derived ids, e.g.:
   - `symbol_id::chunk`
   - `symbol_id::chunk::part-01`
   - `symbol_id::chunk::part-02`
6. Keep `source_type='symbol'` and `source_id=symbol_id` so retrieval and graph relations still resolve to the canonical symbol.
7. Prefix each emitted subchunk with retrieval context:
   - symbol
   - kind
   - optional docstring
   - `part i/n`

**Exit criteria**

- A too-large method no longer becomes a single truncation-prone chunk.
- Multiple chunk rows can be emitted for a single symbol without breaking retrieval joins.

## Tranche C: Verification And Retrieval Safety

**Purpose:** Freeze the behavior with tests and verify retrieval surfaces remain coherent.

**Files**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`
- Modify if needed: `src/axon-core/src/tests/pipeline_test.rs`

**Tests**

1. Small symbol:
   - one emitted chunk
   - no recursive split
2. Oversized symbol:
   - multiple emitted chunks
   - no chunk exceeds target policy
3. Oversized symbol with no structural split points:
   - line-window fallback with overlap
4. Retrieval:
   - chunk rows still anchor to the same symbol/file
5. Model profile:
   - changing the active max token budget changes chunking behavior without code changes

**Exit criteria**

- Chunking behavior is deterministic and covered.
- Retrieval surfaces still work with multi-chunk-per-symbol input.

## Definition Of Done

- No oversized symbol relies on blind tokenizer truncation as its only handling path.
- The chunker is driven by a model profile, not hard-coded `512`.
- Small symbols remain cheap through a fast path.
- Oversized symbols are recursively split only when necessary.
- Axon can change embedding model window size without rewriting the chunking algorithm.
