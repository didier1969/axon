# Axon MCP Guidance: Concept And Vision

## Purpose

This document defines the concept we want for guided MCP responses in Axon.

It focuses on:

- what Axon should help an LLM understand when an MCP tool succeeds, degrades, or fails
- how SOLL should be treated as a maintained strategic layer, not passive documentation
- how to introduce a declarative guidance classifier without turning MCP into a second protocol

This is a concept and direction document, not yet an implementation plan.

## Product Intent

Axon should not only answer a query.
It should help an LLM:

- understand what is true
- understand what is missing
- understand what is probably wrong in the current request, scope, or assumptions
- understand what to do next

The goal is to reduce dead ends, false certainty, tool thrashing, and context waste.

The response from Axon should remain compact.
It should orient the LLM when orientation is needed, not narrate everything all the time.

## Strategic Position

### SOLL Must Be Maintained As A Living Strategic Layer

For Axon, SOLL is not optional contextual garnish.
It is the strategic and intentional truth layer.

An LLM coding against Axon should treat missing, stale, ambiguous, or contradictory SOLL as an operational issue.

Strong rule:

- if an agent discovers that strategic intent, rationale, constraints, or validation intent are materially missing, ambiguous, stale, or contradictory, it must surface that gap explicitly
- if the agent is authorized to update intention, and the evidence is sufficient, it should update SOLL in the same wave
- if authorization is absent, it should recommend the needed SOLL action rather than mutating intention on its own
- if code changes the meaning of the system, SOLL should be updated in the same wave, subject to the same authorization and evidence gates

### Vision Is The Highest-Order SOLL Entity

`Vision` should be treated as the strategic reason the project exists.

It should answer:

- why this project exists
- what human, organizational, or systemic value it creates
- what outcome it is trying to bring into the world

`Vision` is not an implementation detail.
It is the top-level meaning anchor that lets an LLM reason correctly about tradeoffs.

Important precision:

- `Vision` is read-mostly
- it is the right repair target for missing or drifting project purpose
- it is not the default target for every missing intent or traceability gap

## Desired MCP Behavior

We want important MCP tools to produce an appropriate response across these situations:

- positive result with strong evidence
- positive result with partial or degraded evidence
- empty result
- unavailable tool
- invalid scope
- ambiguous input
- incomplete index or incomplete vectorization
- missing SOLL, missing traceability, or missing rationale
- temporary backend pressure or runtime unavailability

The server should avoid exposing raw errors as the primary agent experience unless the error is truly internal and unrecoverable.

Instead, it should classify the situation and orient the LLM.

Important precision:

- guidance should be conditional by default
- a clean success path should stay compact
- extra guidance should appear mainly for degraded, ambiguous, empty, invalid-scope, unavailable, or materially incomplete cases
- a successful response may still include guidance if there is a clear next best action or a material SOLL gap

## Desired Guided Response Contract

The guidance contract should stay small enough that an LLM can use it reliably.

The frozen phase-1 contract, initial `problem_class` set, precedence rules, and examples are defined in:

- [2026-04-17-mcp-guidance-taxonomy.md](/home/dstadel/projects/axon/docs/plans/2026-04-17-mcp-guidance-taxonomy.md)

This concept document keeps the product intent and architectural rules.
The taxonomy document owns the exact phase-1 envelope.

## Examples Of Guided Outcomes

Representative phase-1 examples are frozen in the taxonomy document.

The concept-level point is:

- clean success should usually carry no guidance
- successful-but-actionable responses are allowed when a material SOLL gap exists
- degraded, ambiguous, empty, unavailable, or invalid-scope cases should orient the LLM toward the next best action

## Why Declarative Guidance

We do not want a large imperative maze of tool-specific `if/else` trees spread through the MCP code.

That style tends to become:

- hard to audit
- hard to extend
- hard to test coherently
- hard to explain

Guidance classification is mostly rule composition over normalized facts.
That is a strong fit for a declarative layer.

Important precision:

- the concept is a declarative guidance classifier
- Datalog is the leading implementation candidate, not the conceptual centerpiece

## Recommended Architecture

### Principle

Use a hybrid architecture:

- runtime facts come from the existing Axon Rust + DuckDB world
- a declarative classifier derives guidance semantics from normalized facts
- Rust remains responsible for MCP contract assembly, wording, policy gating, and final response rendering

### Split Of Responsibilities

#### Rust / MCP Layer

Responsible for:

- tool execution
- extracting normalized facts from runtime and tool results
- invoking the guidance classifier
- assembling the final MCP response
- timeout behavior
- wording and response compactness
- authorization checks for any suggested SOLL mutation

#### Declarative Guidance Classifier

Responsible for:

- classifying problem situations
- determining likely causes
- determining best next actions as stable recommendation keys
- determining whether a SOLL update should be recommended

Important constraint:

- the classifier should emit stable semantic keys or codes
- it should not become the renderer, the policy engine, or a second runtime authority

### Datalog Role

Datalog is the preferred candidate for the classifier because:

- the rules are relation-centric
- the logic should stay inspectable and auditable
- the domain is close to graph reasoning already

However:

- Datalog should not replace the MCP transport or rendering layer
- Datalog should not replace DuckDB as runtime truth in this phase
- Datalog should not become a full second runtime authority
- if phase 1 does not actually need recursive reasoning, we should re-evaluate whether a lighter declarative substrate is enough

## Fact Model

The first draft fact model in this concept was too thin.
The guidance classifier needs facts rich enough to avoid guesswork.

It should consume normalized facts such as:

- `tool(tool_name)`
- `tool_available(tool_name)`
- `tool_unavailable(tool_name, reason)`
- `runtime_mode(mode)`
- `project_scope(project_code)`
- `project_scope_valid(project_code)`
- `project_scope_resolved(input, project_code)`
- `symbol_requested(symbol)`
- `symbol_found(symbol)`
- `symbol_ambiguous(symbol)`
- `candidate_symbol(symbol)`
- `result_source(tool_name, source_kind)`
- `result_empty(tool_name)`
- `result_degraded(tool_name)`
- `index_complete(project_code)`
- `vectorization_complete(project_code)`
- `service_pressure(level)`
- `load_state(metric, value)`
- `traceability_present(target)`
- `soll_intent_present(project_code)`
- `soll_vision_present(project_code)`
- `canonical_id(kind, id)`

The classifier must operate on evidence-rich facts, not vague proxies.

## Rule Output Model

The classifier should produce derived facts like:

- `problem_class(tool_name, class)`
- `likely_cause(tool_name, cause)`
- `next_action(tool_name, action_key)`
- `soll_action(tool_name, action_kind)`
- `soll_update_kind(tool_name, update_kind)`
- `guidance_priority(tool_name, priority)`

Rust can then map these semantic keys to the final compact MCP response.

## Alternatives Considered

### Imperative Rules In Rust

Pros:

- easy to start

Cons:

- scales poorly
- hard to maintain consistently across many MCP tools

### SQL-Only Guidance

Pros:

- no new reasoning substrate

Cons:

- poor fit for compositional guidance logic
- mixes fact retrieval and classification too tightly

### YAML / JSON Decision Tables

Pros:

- more maintainable than raw `if/else`

Cons:

- weaker than Datalog for relational reasoning
- likely to become a custom half-rule engine anyway

### Rego / OPA

Pros:

- strong for policy

Cons:

- less natural than Datalog for graph-oriented MCP guidance in Axon

## Rollout Direction

Recommended incremental rollout:

1. Define the stable phase-1 taxonomy and contract in `docs/plans/2026-04-17-mcp-guidance-taxonomy.md`.
2. Define the minimal evidence-rich fact schema.
3. Implement a first declarative classifier only for:
   - `query`
   - `inspect`
4. Keep rendering and response assembly in Rust.
5. Run the classifier in shadow mode against a golden corpus before making it authoritative.
6. Extend later only if phase 1 proves useful and low-noise, likely to:
   - `retrieve_context`
   - `why`
   - `impact`

We should not start with a broad multi-tool rollout.

## Validation Strategy

The first version must be observable and reversible.

Required validation elements:

- golden examples for positive, degraded, empty, ambiguous, invalid-scope, and SOLL-gap cases
- shadow-mode comparison against current MCP behavior
- false-positive budget for guidance recommendations
- explicit rollback criteria if the classifier adds confusion, verbosity, or bad SOLL recommendations

## Success Criteria

We should consider this concept successful if:

- an LLM gets oriented responses instead of raw failure states
- guidance is consistent across tools where it is enabled
- the response contract stays compact enough for repeated use
- missing SOLL is surfaced as a first-class operational issue
- `Vision` is treated as a strategic anchor, not an optional note
- rule maintenance becomes easier than today’s imperative spread

## Non-Goals

- replacing DuckDB as runtime truth
- replacing the MCP response renderer with a rule engine
- building a second canonical source of truth
- auto-mutating SOLL by default
- rewriting the whole MCP layer before proving value on a first tool subset

## Open Questions

- do we actually need recursion in phase 1, or is a lighter declarative substrate sufficient
- should action wording stay entirely in Rust, with the classifier emitting semantic keys only
- should `soll_action` be limited to recommendation semantics in phase 1
- should successful-but-actionable guidance appear only for material SOLL gaps, or also for certain high-confidence next steps

## Current Recommendation

Proceed with a declarative guidance classifier as a focused derivation layer.

Datalog remains the preferred implementation candidate, but only if it stays:

- narrow
- evidence-rich
- observable
- easy to roll back

Keep:

- runtime truth in existing Axon data stores
- response rendering in Rust
- SOLL mutation behind authorization and evidence gates
- the first rollout narrow and observable

Do not begin with a large infrastructural replacement.
