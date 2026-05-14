# Axon Orchestrator SOTA Phase 1 Gap Inventory

Date: 2026-04-19
Project: AXO
Status: working inventory

## Scope

This document captures the concrete phase-1 gaps between:
- the current runtime/orchestrator reality
- and the authority model required by the SOTA plan

It is intentionally implementation-facing.

## Summary

Current reality is best described as:
- several critical actuators are now partially unified
- runtime truth is much more visible than before
- some strategic authority is still fragmented
- some `effective` values are still reported from tuning truth rather than independently observed applied truth
- idle/quiescent behavior is still distributed across multiple loops

## Current State By Area

### 1. Partially Unified Now

These areas have materially improved and already pass through canonical runtime tuning or equivalent central policy:
- `vector_workers`
- `graph_workers`
- `chunk_batch_size`
- `file_vectorization_batch_size`
- `semantic_cadence`
- `vector_ready_queue_depth`
- `vector_persist_queue_bound`
- `vector_max_inflight_persists`
- host-exclusive `gpu_vector_lease`

### 2. Still Fragmented

These areas are still not fully unified:
- actual queue/persist application truth vs tuning truth
- quiescent behavior across background loops
- watcher/scanner/promoter cadence
- graph worker admission vs fully orchestrated graph worker lifecycle
- optimizer vs local component clamping boundaries

## Concrete Code Findings

### A. `runtime_authority` still overstates some effective truth

In [tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs):
- `vector_ready_queue_depth`
- `vector_persist_queue_bound`
- `vector_max_inflight_persists`

are currently emitted as:
- `target`
- `effective = target`

This is better than opaque state, but still not the same as independently observed application truth.

Implication:
- operator truth is stronger
- but not yet end-to-end authoritative

### B. `graph_workers` is partially unified, not fully unified

Relevant files:
- [runtime_tuning.rs](/home/dstadel/projects/axon/src/axon-core/src/runtime_tuning.rs)
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs)
- [main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)

Current behavior:
- bootstrap seeds a graph worker pool
- runtime tuning can reduce admission
- runtime cannot exceed the bootstrapped worker pool without restart

Implication:
- strategic reduction works
- strategic expansion remains restart-bounded

This is acceptable for `partially_unified`
but not enough for a final SOTA claim.

### C. Semantic cadence is centralized, but quiescent behavior is not

Relevant files:
- [vector_control.rs](/home/dstadel/projects/axon/src/axon-core/src/vector_control.rs)
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs)
- [main_background.rs](/home/dstadel/projects/axon/src/axon-core/src/main_background.rs)
- [scanner.rs](/home/dstadel/projects/axon/src/axon-core/src/scanner.rs)
- [worker.rs](/home/dstadel/projects/axon/src/axon-core/src/worker.rs)

What is already good:
- semantic cadence now has named profiles
- runtime tuning scales semantic sleep values

What still blocks SOTA:
- many non-semantic loops keep their own polling cadence
- background optimizer loop defaults remain coarse
- reader refresh loop remains independent
- promoter and watcher-related loops still maintain their own wake policy
- scanner has its own sleep policy ladder

Implication:
- semantic cadence is no longer opaque
- whole-system idle behavior is still emergent rather than canonical

### D. Hidden strategic boundaries still exist across layers

Relevant files:
- [runtime_profile.rs](/home/dstadel/projects/axon/src/axon-core/src/runtime_profile.rs)
- [runtime_tuning.rs](/home/dstadel/projects/axon/src/axon-core/src/runtime_tuning.rs)
- [optimizer.rs](/home/dstadel/projects/axon/src/axon-core/src/optimizer.rs)
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs)

Examples:
- bootstrap profile still defines the first hard shape of worker pools
- runtime tuning normalization clamps values centrally, which is good
- some local application remains restart-bounded or component-bounded

Implication:
- the architecture is converging
- but one can still find strategic influence in more than one layer

### E. Multi-instance GPU safety is improved, not fully global

Relevant files:
- [vector_control.rs](/home/dstadel/projects/axon/src/axon-core/src/vector_control.rs)
- [tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs)

Current gain:
- host-exclusive GPU vector lease is explicit
- operator truth exposes ownership

Remaining gap:
- the lease protects the critical vector lane
- but the full multi-instance qualification story is not yet complete
- contention behavior for non-vector background activity is not yet fully characterized

## Priority Gaps

### Priority 1

Make operator `effective` values real where they are still just tuning truth:
- ready queue depth
- persist queue bound
- max inflight persists

### Priority 2

Create a canonical whole-system quiescent policy:
- semantic lane
- graph lane
- watcher/promoter/scanner loops
- optimizer/refresh loops

### Priority 3

Make stage application boundaries explicit:
- computed by
- applied by
- may clamp if
- observed effective from

### Priority 4

Add recovery/ownership invariants per major stage:
- ingress
- graph
- vector prepare
- ready
- embed
- persist

## Recommended Next Slice

The best next slice is:

1. define `observed_effective` semantics for queue/persist parameters
2. build a single quiescent policy contract
3. attach each major loop to that contract
4. expose true idle state and wakeup reasons in `status`

## Exit Criteria For Phase 1

Phase 1 is complete only if:
- no critical actuator still lies about `effective`
- no major background loop has an undocumented private cadence
- quiescent behavior is explicitly modeled, not emergent
- operator truth can identify where every strategic runtime decision comes from
