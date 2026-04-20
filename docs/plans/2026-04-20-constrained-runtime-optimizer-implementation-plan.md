# Axon Constrained Runtime Optimizer Implementation Plan

Date: 2026-04-20
Project: AXO
Status: plan draft

## Goal

Implement a constrained hierarchical optimizer for Axon's runtime that:
- increases useful semantic throughput
- preserves graph-first utility
- respects RAM and VRAM safety
- remains valid across heterogeneous host classes

## Canonical Delivery Principle

This wave is `SOLL-first`.

The optimizer is not considered real once it exists only in code.
It must exist in:
- live SOLL
- derived SOLL docs
- qualification artifacts
- runtime truth surfaces

## Phase 1: Canonical Intent And Constraint Model

### Objective

Create canonical doctrine for the optimizer before broad implementation.

### Tasks

1. extend the orchestration pillar with optimizer doctrine
2. define the constrained objective canonically
3. define the optimizer architecture canonically
4. define multi-environment scope canonically
5. define validation obligations canonically

### Exit Criteria

- live SOLL includes optimizer doctrine
- derived docs reflect it
- exported SOLL snapshot archives it

## Phase 2: Observable Limiting-Factor Model

### Objective

Make the optimizer reason from explicit bottlenecks rather than intuition.

### Tasks

1. classify runtime limiting factors
- `cpu_prepare_underfeed`
- `gpu_compute_bound`
- `vram_bound`
- `ram_bound`
- `persist_congested`
- `reader_refresh_dominated`
- `interactive_guarded`

2. strengthen runtime truth for:
- GPU temperature
- VRAM pressure
- shared/system GPU memory proxy if accessible
- stage timings
- batch density
- burn-rate quality

3. expose a machine-readable limiting-factor diagnosis

### Exit Criteria

- runtime truth can state what is currently limiting the pipeline
- qualification can archive the diagnosis

## Phase 3: Fast Constrained Controller

### Objective

Implement the fast adaptive loop that operates inside declared safety and mode bounds.

### Tasks

1. define controller inputs
- queue depth
- ready reserve
- prepare inflight
- persist depth
- CPU headroom
- RAM headroom
- GPU utilization
- VRAM pressure
- burn-rate slope
- batch density

2. define controller outputs
- `target_embed_batch_chunks`
- `target_files_per_cycle`
- `ready_reserve_target`
- `prepare_depth`
- `semantic_sleep_scale_pct`
- `semantic_idle_sleep_scale_pct`

3. implement control rules
- bounded adjustments
- hysteresis
- cooldown
- rollback on density collapse
- rollback on memory guard

4. define controller mode contracts
- graph priority
- semantic refill
- balanced drain
- memory guarded
- interactive guarded

### Exit Criteria

- controller decisions are visible in runtime truth
- throughput improves under safe conditions
- density collapse does not trigger runaway over-targeting

## Phase 4: Safety Envelope Hardening

### Objective

Ensure optimization cannot exceed machine safety.

### Tasks

1. centralize hard safety bounds
2. surface hard vs soft clamp reasons
3. integrate GPU thermal pressure later as a new bound class
4. verify CPU-only behavior remains correct

### Exit Criteria

- no optimized path can silently exceed declared machine bounds
- clamps are visible and attributable

## Phase 5: Slow Calibration Layer

### Objective

Add slower adaptation based on evidence rather than every-loop reaction.

### Tasks

1. store host-class calibration seeds
2. define stable per-host tuning candidates
3. evaluate bounded search or offline BO
4. keep live-loop adaptation separate from slow calibration

### Exit Criteria

- host-class seeds become better over time
- fast-loop safety remains stable

## Phase 6: Qualification Matrix

### Objective

Prove the optimizer rather than merely describing it.

### Scenarios

- CPU-only host
- 8 GB VRAM host
- larger VRAM GPU host
- dual-instance host
- graph-only mode
- full mode with active semantic drain
- quiescent idle mode

### Required Artifacts

- runtime status snapshot
- runtime quiescent summary
- resource samples
- resource diagnosis
- burn-rate probe
- controller verdict summary

### Pass Conditions

- useful semantic throughput improves or remains stable
- graph utility remains protected
- VRAM and RAM remain inside safety envelope
- quiescent quality does not regress
- limiting-factor diagnosis is honest

## Immediate Next Slice

1. create optimizer doctrine in live SOLL
2. attach it to `PIL-AXO-007`
3. add minimal requirements and decisions only
4. regenerate derived docs and export
5. then start implementation with limiting-factor truth and fast-controller guardrails
