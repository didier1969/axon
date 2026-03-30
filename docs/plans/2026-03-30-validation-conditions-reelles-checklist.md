---
title: Validation En Conditions Reelles Checklist
date: 2026-03-30
status: active
branch: feat/axon-stabilization-continuation
---

# Purpose

This checklist turns the validation en conditions reelles plan into an operational judge.

Each scenario should be executed on a real codebase, compared against a manual baseline, and recorded with evidence.

# How To Use This File

For each scenario:

1. run the baseline with manual navigation or `rg`
2. run the Axon workflow
3. record whether Axon was:
   - faster
   - clearer
   - more complete
   - more trustworthy
4. note what should be improved next

# Scoring

- `0`: broken or misleading
- `1`: works but weak
- `2`: useful
- `3`: clearly superior to manual baseline

# Execution Log Template

For every run, capture:

- date
- repo
- scenario id
- prompt
- baseline result
- Axon result
- score
- follow-up action

Suggested template:

```md
## Run

- Date:
- Repo:
- Scenario:
- Prompt:
- Baseline:
- Axon:
- Score:
- Follow-up:
```

# Checklist

## VCR-1 Symbol Discovery

### Target

- find the correct symbol/file from a natural-language need

### Prompts

- [ ] "Where is the scan trigger wired?"
- [ ] "Which modules handle MCP tool dispatch?"
- [ ] "Where is batch acknowledgement correlated?"
- [ ] "Where is SOLL restore implemented?"

### Expected Axon Path

- `axon_query`
- `axon_inspect`
- `axon_fs_read`

### Acceptance

- [x] identifies the correct file or symbol quickly
- [ ] reduces broad manual traversal
- [x] points to a useful next reading target

### Last Score

- `2`
- executable MCP coverage exists in `src/axon-core/src/mcp/tests.rs`

## VCR-2 Impact Before Change

### Target

- estimate blast radius before editing

### Prompts

- [ ] "What breaks if parse_batch ACK semantics change?"
- [ ] "Who depends on this public API?"
- [ ] "What would this mutation impact?"

### Expected Axon Path

- `axon_impact`
- `axon_bidi_trace`
- `axon_api_break_check`
- `axon_simulate_mutation`

### Acceptance

- [x] shows direct and indirect effects
- [x] highlights meaningful callers or consumers
- [ ] helps decide whether the change is local or architectural

### Last Score

- `2`
- executable MCP coverage exists in `src/axon-core/src/mcp/tests.rs`

## VCR-3 Architecture Rule Validation

### Target

- detect real layer violations or coupling drift

### Prompts

- [ ] "Does ui call db directly?"
- [ ] "Does dashboard bypass the intended bridge?"
- [ ] "Do watcher paths cut across intended layers?"

### Expected Axon Path

- `axon_architectural_drift`
- `axon_query`
- `axon_inspect`

### Acceptance

- [ ] finds real violations
- [ ] provides concrete paths and symbols
- [ ] avoids empty or misleading reports

### Last Score

- `unrun`

## VCR-4 SOLL Continuity

### Target

- preserve conceptual intent through export and restore

### Workflow

- [x] create or update SOLL entities
- [x] export SOLL
- [x] restore SOLL from export
- [x] verify restored entities are usable

### Expected Axon Path

- `axon_soll_manager`
- `axon_export_soll`
- `axon_restore_soll`

### Acceptance

- [x] no destructive surprise
- [x] restore is merge-oriented
- [x] conceptual continuity is preserved

### Last Score

- `2`
- executable MCP coverage exists in `src/axon-core/src/mcp/tests.rs`

## VCR-5 Operator Truthfulness

### Target

- ensure visible actions map to real runtime behavior

### Workflow

- [x] trigger manual scan from the operator path
- [x] observe indexing progress
- [x] confirm Rust-side work occurs
- [x] confirm state returns to live

### Acceptance

- [ ] no fake action
- [ ] no stale operator state
- [ ] no silent no-op

### Last Score

- `2`

## VCR-6 Audit And Health Honesty

### Target

- ensure quality outputs are useful without overstating confidence

### Expected Axon Path

- `axon_audit`
- `axon_health`
- `axon_debug`

### Acceptance

- [ ] findings are traceable
- [ ] wording matches signal strength
- [ ] output helps prioritization

### Last Score

- `unrun`

## VCR-7 Axon On Axon

### Target

- use Axon to understand and improve Axon itself

### Tasks

- [ ] locate protocol boundaries
- [ ] trace ingestion flow
- [ ] inspect IST/SOLL boundaries
- [ ] identify watcher to Rust control edges
- [ ] assess refactor impact before change

### Acceptance

- [ ] Axon clearly beats raw manual navigation on at least 3 tasks
- [ ] answers are stable enough to support real editing work

### Last Score

- `unrun`

# Priority Order

Recommended run order:

1. `VCR-1 Symbol Discovery`
2. `VCR-2 Impact Before Change`
3. `VCR-5 Operator Truthfulness`
4. `VCR-4 SOLL Continuity`
5. `VCR-3 Architecture Rule Validation`
6. `VCR-6 Audit And Health Honesty`
7. `VCR-7 Axon On Axon`

# Current Next Step

Run the first two scenarios on Axon itself and record evidence in this file or a companion log.
