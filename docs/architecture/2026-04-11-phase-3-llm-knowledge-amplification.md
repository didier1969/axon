# Phase 3: LLM Knowledge Amplification

Date: 2026-04-11

## Purpose

Phase 3 extends Axon from a set of useful retrieval tools into a query-time
knowledge amplification layer for LLMs.

The goal is not to maximize raw retrieval volume. The goal is to maximize
grounded answerability under bounded latency and token budgets.

Axon already had the right substrate:

- canonical IST truth
- structural graph
- chunk layer
- deferred semantic enrichment
- SOLL / traceability intent layer
- shared-server MCP contract

The missing piece was orchestration at query time: deciding what to retrieve,
how to join it, how to bound graph expansion, when to inject SOLL, and how to
package evidence for LLM consumption.

## Design Principles

- Preserve the existing system. Add a new retrieval path instead of rewriting
  existing MCP tools.
- Keep graph embeddings secondary. Chunk/file semantic retrieval is the primary
  semantic path.
- Keep graph expansion bounded and explainable.
- Inject SOLL only when it materially improves the answer.
- Return an evidence packet shaped for LLM consumption, not just human
  inspection.
- Preserve shared-server behavior and existing MCP mutation semantics.

## Runtime Shape

Phase 3 introduces a new additive MCP tool:

- `retrieve_context`

This tool does not replace `query`, `inspect`, or `impact`. It builds on the
same canonical data and retrieval primitives, but adds a planner and an
evidence packet contract.

The current implementation keeps `retrieve_context` hidden by default from the
public tool list. It is intended as an expert/internal retrieval surface until
it has enough runtime evidence to become a broader default.

## Query Planner

`retrieve_context` starts with an explicit route selection step. The planner is
inspectable and returns its selected route in the structured payload.

Current planner routes:

- `exact_lookup`
- `wiring`
- `impact`
- `soll_hybrid`
- `hybrid`

These routes are chosen from the question text and optional project scope.
Examples:

- exact symbol/file/config questions route to `exact_lookup`
- wiring questions route to `wiring`
- blast-radius / breakage questions route to `impact`
- rationale / requirement / intent questions route to `soll_hybrid`
- broader mixed questions route to `hybrid`

The planner is intentionally explicit. We do not hide strategy selection behind
opaque heuristics with no operator visibility.

### Scope normalization

Phase 3.1 hardens project-scope routing at query time. Retrieval now accepts
canonical project codes, slugs, and repo-derived aliases without rewriting the
stored graph rows. This closes the real runtime mismatch where a project could
be queried as `AXO` while symbol rows still used `axon`.

The planner exposes:

- `project_scope_variants`
- `semantic_search_used`
- `degraded_reason`
- live queue depths and service pressure

## Hybrid Retrieval Pipeline

The Phase 3 retrieval path is:

1. exact / lexical retrieval
2. semantic chunk retrieval
3. candidate union and dedup
4. bounded graph neighborhood expansion around strong candidates
5. conditional SOLL / Traceability join
6. global reranking
7. evidence packet assembly under a token budget

### Exact and lexical retrieval

Exact and lexical retrieval remains important because many Axon questions are
anchored on stable entities:

- symbols
- file paths
- modules
- config names
- services

This path is fast, deterministic, and a strong source of high-confidence entry
points.

Phase 3.1 also treats explicit file/document path questions as first-class
anchors. A path-like query can now resolve directly to a canonical file entry
before broader semantic search is attempted.

### Chunk retrieval

Chunk retrieval is the primary semantic input surface for Phase 3.

If query-time embeddings are available, Axon retrieves chunk candidates via
semantic similarity. If the semantic worker is paused or unavailable, the
system falls back to lexical chunk retrieval instead of failing the request.

This preserves the two-speed model:

- fast lane: structural truth remains available early
- slow lane: semantic enrichment improves retrieval quality asynchronously

Phase 3.1 adds an anchor-first chunk policy:

- chunks anchored to the selected symbol are preferred first
- same-file chunks come next
- graph-neighbor or broader semantic chunks are only kept if they add
  non-redundant support

Phase 3.2 hardens this further for operational code questions:

- implementation chunks beat doc, test, fixture, and example chunks when a
  strong local anchor exists
- broader semantic fallbacks are capped to one item
- exclusion reasons are preserved explicitly, including:
  - `docs_file_penalty`
  - `test_file_penalty`
  - `same_file_preferred`
  - `broader_semantic_dropped_due_to_anchor`
  - `non_operational_chunk_penalized`

This reduces the previous failure mode where a correct entrypoint was found but
supporting chunks were semantically loose.

### Pressure-aware degradation

`retrieve_context` now consumes existing runtime pressure signals instead of
pretending semantic search is always available.

Current behavior:

- `Healthy`: exact, lexical, semantic chunk search, bounded graph, conditional SOLL
- `Recovering`: semantic search only when a strong anchor exists and vector backlog is bounded
- `Degraded` / `Critical`: exact, lexical, bounded graph only; semantic search is skipped explicitly

Phase 3.2 adds MCP-first interactive priority on top of this model:

- live MCP requests enter an interactive priority state
- new slow-lane vectorization and non-requested graph projection admissions are
  suppressed while interactive priority is active
- already-started critical writer sections are allowed to drain safely
- runtime telemetry now exposes:
  - `interactive_priority_active`
  - `interactive_priority_level`
  - `interactive_requests_in_flight`
  - `background_launches_suppressed_total`
  - `vectorization_suppressed_due_to_interactive`
  - `projection_suppressed_due_to_interactive`

The packet reports honest partial truth through:

- planner `degraded_reason`
- packet `missing_evidence`
- packet `excluded_because`

### Graph expansion

Graph expansion is bounded. It is used to enrich strong anchors, not to flood
the response with graph noise.

Current policy:

- most routes expand to one hop
- `impact` may expand to two hops

The graph branch answers questions such as:

- who calls this symbol?
- what contains this file?
- what are the immediate structural neighbors?
- what may be affected if this changes?

### SOLL injection

SOLL is treated as a precise why-layer.

It is joined only when it materially improves the answer, especially for:

- architectural intent
- design rationale
- requirement constraints
- traceability links

It is not dumped indiscriminately into every answer. The retrieval output keeps
structural truth and intentional truth distinct.

Phase 3.2 upgrades SOLL joining to be anchor-first:

- direct symbol traceability outranks broader intent
- direct file traceability is now considered alongside symbol traceability
- rationale joins remain capped to 1-2 items
- missing rationale states are more precise, including:
  - `anchor_found_but_no_traceability`
  - `rationale_requested_but_no_intent_evidence`

## Evidence Packet Contract

Phase 3 replaces mostly flat hits with a canonical evidence packet for LLM
consumption.

Current packet shape includes:

- `answer_sketch`
- `direct_evidence`
- `supporting_chunks`
- `structural_neighbors`
- `relevant_soll_entities`
- `confidence`
- `missing_evidence`
- `why_these_items`
- `excluded_because`
- `token_budget_estimate`
- `retrieval_diagnostics`

### Semantics

- `answer_sketch`: compact retrieval-grounded summary of the most likely answer
  shape
- `direct_evidence`: strongest exact or high-confidence anchor items
- `supporting_chunks`: chunk-level evidence selected for semantic support and
  coverage
- `structural_neighbors`: bounded graph context around strong anchors
- `relevant_soll_entities`: intent/rationale items joined only when useful
- `confidence`: retrieval-time confidence, not a claim of full correctness
- `missing_evidence`: what the system could not prove from current knowledge
- `why_these_items`: routing and ranking rationale for observability
- `excluded_because`: candidates dropped due to redundancy, low confidence, or
  budget pressure
- `token_budget_estimate`: approximate packaging budget used to assemble the
  result
- `retrieval_diagnostics`: candidate counts and selected evidence counts for
  observability

### Selection policy

The packet favors compact diversity over large dumps:

- 1-2 strong entrypoints
- 2-4 supporting chunks
- 1-2 structural neighbors
- 1 precise SOLL signal when relevant

It also makes different evidence classes explicit:

- canonical current truth
- intentional / rationale truth
- derived diagnostics
- advanced or lower-confidence supporting signals

## Reranking

Phase 3 adds a practical reranking layer above raw candidate retrieval.

The initial reranker is intentionally bounded and explainable. It improves top-k
quality without introducing an expensive or opaque dependency.

Current ranking signals include:

- exactness of the lexical match
- planner route intent
- semantic distance when available
- anchoring strength
- graph relevance
- duplication and redundancy pressure

Phase 3.2 also unifies canonical scope handling across historical MCP tools:

- `retrieve_context`, `impact`, and `inspect` now accept canonical project
  code, slug, and repo-derived aliases through the same normalization path
- this removes the previous divergence where exact retrieval succeeded under a
  canonical code but historical tools still reported scope misses
- file affinity and anchored chunk preference
- project-scope normalization

The immediate objective is better top-k usefulness, not maximal model
complexity. More expensive rerankers remain optional future work only if they
produce measured gains.

## Why Graph Embeddings Remain Secondary

Graph embeddings are not a primary KPI in the product surface and are not the
center of Phase 3.

Reasons:

- chunk/file semantic retrieval improves answerability more directly
- structural graph truth is already available without graph embeddings
- bounded graph projection provides most of the useful local topology cheaply
- graph embeddings add complexity and cost before they have proven query-time
  value

Graph embeddings may still exist as an advanced signal, but they are not the
main retrieval contract.

## Evaluation

Phase 3 requires executable evaluation artifacts instead of subjective claims.

The current evaluation harness exercises representative Axon tasks:

- symbol discovery
- wiring questions
- impact / breakage questions
- rationale questions requiring SOLL

Phase 3.1 adds executable retrieval qualification:

- `scripts/qualify_retrieval_context.py`
- fixed corpus in `scripts/retrieval_context_cases.json`
- integration into `qualify_runtime.py` as `retrieval_qualify`
- explicit hidden-tool probes in `mcp_validate.py`
- `retrieve_context` load coverage in `qualify_mcp_robustness.py`

Required evaluation layers:

1. Offline retrieval quality
   - Recall@k
   - MRR
   - nDCG
   - citation precision
   - good-file / good-symbol hit rate
2. Task-level answer quality
   - exactness
   - groundedness
   - completeness
   - useless context ratio
   - blast-radius accuracy
   - architecture understanding quality
3. Runtime behavior
   - p50/p95 latency
   - token budget used
   - SQL/MCP responsiveness under load
   - `graph_only` vs `full` behavior
   - degraded behavior under partial semantic readiness

The initial implementation includes test-level benchmark coverage for planner
routing, grounded evidence presence, and conditional SOLL retrieval. This is a
base layer, not the final evaluation program.

## Operational Notes

- `retrieve_context` is additive and does not regress the current MCP tool
  model.
- Shared-server behavior is preserved.
- No destructive SOLL mutations are required.
- If semantic workers are unavailable, retrieval degrades to lexical evidence
  instead of failing outright.
- Planner output and evidence packet structure provide operator-visible
  observability for why a given context pack was assembled.

## Out of Scope for This Slice

- replacing `query` with `retrieve_context`
- making graph embeddings primary again
- unbounded graph exploration
- destructive SOLL evolution
- expensive judge-model reranking without measured gains
- dashboard mutation controls

## Result

Phase 3 makes Axon more useful to LLMs by turning existing knowledge into a
bounded, explainable, and answer-oriented retrieval product.

The main change is not more stored knowledge. The main change is better query
time assembly of the knowledge Axon already has.
