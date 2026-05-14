# Axon Constrained Runtime Optimizer Concept

Date: 2026-04-20
Project: AXO
Status: concept draft

## Thesis

Axon should not adopt a monolithic global optimizer that continuously chases a single scalar objective.

Axon should adopt a constrained hierarchical runtime optimizer:
- a hard safety envelope
- a mode supervisor
- a fast constrained controller
- a slow calibration layer

This architecture matches Axon's real mission profile:
- graph utility must arrive early
- semantic throughput must stay high
- VRAM and RAM must remain safe
- interactive work must stay protected
- idle behavior must remain quiet
- host contention must remain explicit

## Why A Single Global Optimizer Is Not The Right First Step

The runtime is not stable enough for a single global maximizing solver to be trusted yet.

Reasons:
- workload shape changes continuously
- backlog depth changes continuously
- file density and token distribution change continuously
- dual-instance contention may exist on the same host
- some resource truths are still partial
- the objective is multi-dimensional, not scalar

Today Axon does not have one clean value to maximize.
It has a constrained objective:
- maximize useful semantic burn rate
- while preserving graph-first value
- while staying inside host safety bounds
- while protecting quiescent and interactive quality

## Target Objective

The optimizer should maximize:
- useful semantic progress per unit time

Subject to:
- VRAM soft and hard ceilings
- RAM soft and hard ceilings
- no unsafe spill behavior
- graph utility latency target
- interactive latency guard
- quiescent wake and noise budget
- multi-instance GPU exclusivity rules

This means the optimizer is not free to choose any setting that increases throughput.
It must maximize under explicit constraints.

## Proposed Architecture

### 1. Safety Envelope

This layer owns non-negotiable bounds:
- model contract
- max embed batch bytes
- VRAM soft limit
- VRAM hard limit
- RAM soft limit
- RAM hard limit
- max live GPU workers by host class
- max persist pressure
- GPU lease policy

This layer is declarative and fail-closed.

### 2. Runtime Mode Supervisor

This layer classifies the runtime into operational modes:
- `quiescent`
- `graph_priority`
- `semantic_refill`
- `balanced_drain`
- `interactive_guarded`
- `gpu_memory_guarded`
- `recovery_guarded`

The supervisor does not directly tune every actuator.
It narrows the allowed region for the controller.

### 3. Fast Constrained Controller

This layer adjusts live runtime targets inside the active safety and mode envelope.

Primary actuators:
- `target_embed_batch_chunks`
- `target_files_per_cycle`
- `ready_reserve_target`
- `prepare_depth`
- `prepare_prefetch_aggressiveness`
- `semantic_sleep_scale_pct`
- `semantic_idle_sleep_scale_pct`
- selected queue and persist bounds

The controller must use:
- hysteresis
- cooldown windows
- rollback on degraded density or throughput
- explicit clamp visibility

### 4. Slow Calibration Layer

This layer optimizes seeds and policy defaults more slowly:
- per host class
- per runtime mode
- per provider class

It should not run every loop.
It should learn from qualification and stable runtime traces.

Candidate techniques:
- bounded search
- Bayesian optimization offline
- contextual bandit for narrow actuator proposals

## Multi-Environment Strategy

The optimizer must be hardware-aware by construction.

It should adapt across:
- CPU-only hosts
- small-VRAM GPU hosts
- larger-VRAM GPU hosts
- low-core laptops
- high-core workstations

The method is:
- detect host capabilities
- derive a hardware seed
- adapt dynamically inside runtime constraints
- recalibrate slowly from evidence

This is not a single-machine optimizer.
It is a host-class-aware control system.

## Required Runtime Truth

The optimizer can start now, but it still needs stronger truth over time.

Already sufficient to begin:
- CPU utilization
- RAM headroom
- GPU utilization
- VRAM usage
- vector lane runtime metrics
- ready/prepare/persist depths
- burn-rate probe
- quiescent wake telemetry

Still needed for a stronger final system:
- GPU temperature inside canonical runtime truth
- shared/system GPU memory or spill proxy if accessible
- stage timings with cleaner attribution
- clearer effective semantics for a few remaining actuators

## Canonical Runtime Concepts

The optimizer should make explicit the following concepts:
- constrained runtime objective
- safety envelope
- runtime mode supervisor
- fast constrained controller
- slow calibration layer
- host-class seed
- useful semantic burn rate
- stage density collapse

## What Must Be Visible To Operators

For each critical actuator, runtime truth should expose:
- `seed`
- `target`
- `effective`
- `clamp_reason`
- `authority_state`
- `target_source`
- `effective_source`

For the optimizer itself, operators should also see:
- current mode
- current objective focus
- dominant limiting factor
- safety clamp cause when active
- underfeed diagnosis when active
- whether the runtime is throughput-limited, memory-limited, or policy-limited

## Non-Goals

This wave should not:
- replace the existing orchestrator wholesale
- introduce hidden strategic caps
- optimize only for one GPU model
- hide degraded throughput behind averaged status fields
- claim mathematical optimality before runtime proof exists

## Success Criteria

This wave is successful if:
- the optimizer architecture is canonically described in SOLL
- the control objective is explicit
- the safety envelope is explicit
- the fast controller scope is explicit
- the slow calibration scope is explicit
- the design remains valid across CPU-only and GPU hosts
- implementation can proceed incrementally without architectural contradiction

## Recommended Delivery Order

1. document the optimizer canonically in SOLL
2. define the objective and constraint model in code-facing docs
3. finish missing observability needed by the controller
4. implement the fast constrained controller first
5. add slow calibration only after fast-loop truth is stable
