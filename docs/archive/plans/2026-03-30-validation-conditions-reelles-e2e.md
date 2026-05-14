---
title: Validation En Conditions Reelles E2E
date: 2026-03-30
status: draft
branch: feat/axon-stabilization-continuation
---

# Intent

Axon must first prove its value in real LLM-assisted development before further commercialization work.

This document defines the end-to-end validation scenarios that should become the practical judge of progress.

# Principle

Do not ask only:

- does Axon start
- does Axon ingest
- does Axon answer something

Ask instead:

- does Axon make an LLM developer better than raw file reading
- does Axon reduce navigation cost
- does Axon preserve conceptual continuity through `SOLL`
- does Axon help steer change and risk in a live codebase

# Evaluation Axes

Each scenario should be judged on:

1. correctness
2. usefulness
3. time-to-answer
4. operator clarity
5. repeatability

Suggested scoring:

- `0`: broken or misleading
- `1`: partially usable but weak
- `2`: correct and useful
- `3`: clearly superior to raw grep + manual reading

# Baseline Comparison

For every scenario, compare Axon against a simple baseline:

- `rg`
- targeted file reads
- manual code traversal

Axon is valuable only if it produces one or more of these improvements:

- faster navigation
- less missed context
- better impact reasoning
- clearer project-state reasoning
- preservation of intention through `SOLL`

# Scenario Set

## VCR-1 Symbol Discovery

Goal:

- find the right symbol or file to inspect from a natural-language need

Prompt examples:

- "Where is the scan trigger wired?"
- "Which modules handle MCP tool dispatch?"
- "Where is batch acknowledgement correlated?"

Axon path:

- `axon_query`
- `axon_inspect`
- optional `axon_fs_read`

Expected value:

- returns the right symbol/file quickly
- identifies the right next reading target
- avoids broad manual repo traversal

Failure modes:

- wrong symbol family
- too many irrelevant hits
- opaque or misleading mode wording

## VCR-2 Impact Before Change

Goal:

- estimate the blast radius before modifying a symbol or protocol path

Prompt examples:

- "What breaks if I change parse_batch ACK semantics?"
- "Who calls this symbol?"
- "What is impacted if this public function changes?"

Axon path:

- `axon_impact`
- `axon_bidi_trace`
- `axon_api_break_check`
- `axon_simulate_mutation`

Expected value:

- shows direct and indirect effects
- highlights likely consumer paths
- helps decide whether a change is local or architectural

Failure modes:

- incomplete caller graph
- false confidence
- unusable formatting

## VCR-3 Architecture Rule Validation

Goal:

- detect architectural drift and layer violations early

Prompt examples:

- "Does ui call db directly?"
- "Do watcher and dashboard layers bypass the intended bridge?"

Axon path:

- `axon_architectural_drift`
- `axon_query`
- `axon_inspect`

Expected value:

- detects real violations
- surfaces concrete paths and symbols
- gives a useful operator/developer answer, not just a boolean

## VCR-4 Concept Preservation Through SOLL

Goal:

- prove that conceptual project state survives runtime churn

Workflow:

1. create or update SOLL entities
2. export `SOLL`
3. restore from export
4. verify the restored conceptual layer is usable

Axon path:

- `axon_soll_manager`
- `axon_export_soll`
- `axon_restore_soll`

Expected value:

- no destructive surprise
- merge-oriented restore
- stable conceptual continuity

Failure modes:

- missing entities after restore
- format drift
- restore that silently corrupts meaning

## VCR-5 Operator Truthfulness

Goal:

- ensure visible actions in the dashboard map to real runtime behavior

Workflow:

1. trigger manual scan
2. observe indexing progress
3. verify Rust-side work actually occurs
4. verify status returns to live state

Expected value:

- no fake buttons
- no stale operator states
- no mismatch between UI claim and runtime effect

## VCR-6 Audit And Health With Honest Confidence

Goal:

- ensure Axon helps an LLM reason about project quality without overstating certainty

Axon path:

- `axon_audit`
- `axon_health`
- `axon_debug`

Expected value:

- findings are traceable
- output is useful for prioritization
- uncertainty is not hidden

Failure modes:

- authoritative wording on weak signals
- metrics that look exact but are heuristic

## VCR-7 Axon On Axon

Goal:

- use Axon to understand and evolve Axon itself

Target tasks:

- locate protocol boundaries
- trace ingestion flow
- inspect `IST`/`SOLL` boundaries
- find watcher to Rust control edges
- assess impact before refactor

This scenario is the highest-value internal proving ground.

# Execution Order

Recommended order:

1. `VCR-1 Symbol Discovery`
2. `VCR-2 Impact Before Change`
3. `VCR-5 Operator Truthfulness`
4. `VCR-4 Concept Preservation Through SOLL`
5. `VCR-3 Architecture Rule Validation`
6. `VCR-6 Audit And Health With Honest Confidence`
7. `VCR-7 Axon On Axon`

# Exit Criteria For Phase 0

Validation en conditions reelles can be considered satisfactory when:

- the main scenarios are repeatable
- the answers are more useful than baseline manual navigation
- the operator workflow is truthful
- `SOLL` continuity is preserved
- Axon helps a real development task instead of adding ceremony

# Next Implementation Step

Turn these scenarios into a tracked checklist and bind each scenario to:

- concrete prompts
- expected outputs
- observed outputs
- regression notes
- decision to improve tool, UX, or data quality
