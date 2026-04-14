# Traceability And Shadow Optimizer Plan

## Topology
1. Canonical state
2. Append-only analytics
3. Derived hourly rollups
4. Shadow optimizer decisions
5. Future actuator application

## Canonical State
`File` stores:
- `first_seen_at_ms`
- `indexing_started_at_ms`
- `graph_ready_at_ms`
- `vectorization_started_at_ms`
- `vector_ready_at_ms`
- `last_state_change_at_ms`
- `last_error_at_ms`

`ChunkEmbedding` stores:
- `embedded_at_ms`

Semantics:
- first-transition timestamps are written once
- `last_state_change_at_ms` is rewritten on canonical state transitions
- `embedded_at_ms` reflects the last effective persistence of `(chunk_id, model_id)`

## Append-Only Analytics
Tables:
- `FileLifecycleEvent`
- `VectorBatchRun`
- `OptimizerDecisionLog`
- `RewardObservationLog`

Rules:
- file lifecycle events are file-level and stable-stage only
- vector batch runs are batch-level and append-only
- optimizer decisions and rewards are append-only and reproducible from stored snapshots

## Derived Analytics
Table:
- `HourlyVectorizationRollup`

Purpose:
- exact hourly `chunks/hour`
- exact hourly `files/hour`
- best-hour queries
- recent analytics windows for optimizer input

## Host-Aware Shadow Optimizer
The optimizer is Axon-native and deterministic in v1.

Inputs:
- `HostSnapshot`
- `RuntimeSignalsWindow`
- `OperatorPolicySnapshot`
- `RecentAnalyticsWindow`

Output:
- `OptimizerDecision`

Rules:
- shadow mode only in v1
- hard constraints remain outside the policy engine
- one actuator at a time
- open slowly, close quickly
- never exceed host physical limits

## Hard Constraints
Configurable operator policy:
- `max_cpu_ratio`
- `min_ram_available_ratio`
- `max_mcp_p95_ms`
- `max_vram_used_ratio`
- `max_vram_used_mb`
- `max_io_wait_ratio`
- `backlog_priority_weight`
- `interactive_priority_weight`
- `shadow_mode_enabled`
- `allowed_actuators`
- `evaluation_window_ms`

Detected host facts:
- platform
- WSL or not
- CPU cores
- total RAM
- GPU presence
- GPU name
- total VRAM

## Implementation Order
1. Migrate canonical state
2. Add append-only event tables
3. Wire stable write points
4. Add derived hourly rollups
5. Add host and policy snapshots
6. Add shadow optimizer logging
7. Expose truth through MCP `status`
8. Validate with targeted TDD
