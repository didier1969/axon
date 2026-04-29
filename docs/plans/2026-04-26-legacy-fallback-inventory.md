# Axon Legacy and Fallback Inventory

## Purpose

This inventory isolates the remaining `legacy_*`, `compatibility_shim`, and `fallback_*` surfaces so cleanup can remove the right branches in the right order.

It is not a deletion list by itself.
It distinguishes:
- topology compatibility that still affects public/runtime truth
- product fallbacks that may still be intentional
- purely operational/script residue
- low-level defensive fallbacks that are not legacy debt

## Current Signal

Repository-wide coarse count:
- `187` matches for `legacy_monolith|compatibility_shim|fallback_`

This count is intentionally noisy.
It includes:
- tests
- metrics
- runtime truth strings
- real compatibility paths
- legitimate algorithmic fallback behavior

## Buckets

### 1. Topology Compatibility Debt

These are the highest-value cleanup targets because they keep old monolith topology alive in public reasoning.

Primary files:
- [runtime_topology.rs](/home/dstadel/projects/axon/src/axon-core/src/runtime_topology.rs:80)
- [runtime_mode.rs](/home/dstadel/projects/axon/src/axon-core/src/runtime_mode.rs:49)
- [tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs:268)
- [main_telemetry.rs](/home/dstadel/projects/axon/src/axon-core/src/main_telemetry.rs:118)
- [tools_system.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_system.rs:127)

Observed patterns:
- `legacy_monolith`
- `legacy_compatibility_shim`
- compatibility truth in runtime topology payloads
- default process role fallback to `legacy_monolith`

Assessment:
- this cluster is real cleanup debt
- it should be burned down after the current watcher pass, but only once runtime truth and scripts can move together

### 2. Script and Operator Residue

Primary files:
- [start.sh](/home/dstadel/projects/axon/scripts/start.sh:109)
- [status.sh](/home/dstadel/projects/axon/scripts/status.sh:16)
- [stop.sh](/home/dstadel/projects/axon/scripts/stop.sh:16)
- [lib/axon-role-layout.sh](/home/dstadel/projects/axon/scripts/lib/axon-role-layout.sh:5)
- [qualify_runtime.py](/home/dstadel/projects/axon/scripts/qualify_runtime.py:570)
- [qualify_ingestion_run.py](/home/dstadel/projects/axon/scripts/qualify_ingestion_run.py:91)

Observed patterns:
- default role fallback to `legacy_monolith`
- qualification logic that still treats monolith truth as a canonical success path
- TMUX fallback kill logic mixed with core stop flow

Assessment:
- this is likely the next major cleanup wave
- it is risky because these scripts are still operationally important
- it should be done as one coordinated pass, not piecemeal

### 3. Runtime Product Fallbacks

Primary files:
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:2079)
- [service_guard.rs](/home/dstadel/projects/axon/src/axon-core/src/service_guard.rs:1104)
- [main_background.rs](/home/dstadel/projects/axon/src/axon-core/src/main_background.rs:227)
- [bridge.rs](/home/dstadel/projects/axon/src/axon-core/src/bridge.rs:177)

Observed patterns:
- `mixed_fallback_batches_total`
- `prepare_fallback_inline_total`
- `finalize_fallback_inline_total`
- CPU fallback ORT environment helpers

Assessment:
- not all of this is legacy debt
- some of it is explicit runtime recovery behavior
- cleanup here must separate:
  - true historical fallback
  - current recovery semantics that still serve production

### 4. Retrieval and Semantic Fallbacks

Primary files:
- [tools_context.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs:966)
- [tools_dx.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_dx.rs:419)
- [tools_soll.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs:7370)

Observed patterns:
- repo literal fallback
- semantic fallback reason
- fallback guidance

Assessment:
- mostly not legacy debt
- these are product behaviors and diagnostics
- they should not be removed under a “legacy cleanup” label

### 5. Low-level Defensive Fallbacks

Primary files:
- [code_chunker.rs](/home/dstadel/projects/axon/src/axon-core/src/code_chunker.rs:34)
- [queue.rs](/home/dstadel/projects/axon/src/axon-core/src/queue.rs:311)
- [graph.rs](/home/dstadel/projects/axon/src/axon-core/src/graph.rs:51)
- [graph_bootstrap.rs](/home/dstadel/projects/axon/src/axon-core/src/graph_bootstrap.rs:704)

Assessment:
- these are engineering fallbacks, not product topology residue
- they are not primary cleanup targets in this campaign

## Recommended Removal Order

1. watcher/dashboard wrappers and dead assigns
2. script/operator residue around `legacy_monolith`
3. runtime topology compatibility strings and MCP projections
4. only then reassess whether runtime recovery fallbacks can be simplified

## Explicit Non-Targets For Now

- semantic retrieval fallback behavior
- SOLL fallback guidance wording
- queue/memory defensive fallbacks
- mixed batch fallback itself as a vector runtime concept

## Next Cut Candidates

Most credible next removal targets after the current watcher pass:
- `legacy_monolith` defaulting in shell/runtime role resolution
- duplicated monolith success logic in qualification scripts
- compatibility projection shaping in runtime topology MCP output
