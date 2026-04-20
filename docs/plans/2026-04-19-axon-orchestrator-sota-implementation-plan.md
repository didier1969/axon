# Axon Orchestrator To SOTA Implementation Plan

Date: 2026-04-19
Project: AXO
Status: plan draft

## Goal

Bring Axon's orchestrator from `partially unified and credible` to `measurably state of the art` for its mission profile.

## Validation Matrix

The work is only complete if it passes:
- single-instance GPU qualification
- dual-instance shared-host qualification
- CPU-only qualification
- cold rebuild qualification
- idle/quiescent qualification
- recovery qualification after staged interruption

## Phase 1: Finish Runtime Authority Unification

### Objective

Make the canonical orchestrator the strategic source of truth for all critical runtime actuators.

### Tasks

1. Inventory remaining strategic clamps
- scan runtime_profile, embedder, optimizer, vector_control, main_background
- classify each decision as fixed, bounded, or computed

2. Remove hidden strategic caps
- keep only explicit safety clamps locally
- move strategic target computation to the canonical runtime tuning path

3. Extend authority contract
- expose `seed`
- expose `target`
- expose `effective`
- expose `clamp_reason`
- expose `target_source`
- expose `effective_source`
- expose `authority_state`

4. Finish actuator coverage
- graph workers
- vector workers
- queue depths
- persist bounds
- micro-batch sizing
- quiescent cadence

5. Add actuator semantics taxonomy
- live-adjustable without restart
- restart-bound
- safety-clamped locally
- measured-effective
- controller-effective only

### Exit Criteria

- no critical strategic actuator remains locally fixed without explicit declaration
- runtime truth can explain every major target/effective divergence
- every critical actuator is classified by live/restart/clamp/observation semantics

## Phase 1b: Actuator Semantics And Live-Apply Taxonomy

### Objective

Prevent the orchestrator from overclaiming authority or applied truth.

### Tasks

1. For each critical actuator, declare:
- `computed_by`
- `applied_by`
- `may_clamp_if`
- `observed_effective_from`

2. Mark actuators explicitly as:
- live-adjustable without restart
- restart-bound
- local safety-clamped
- measured-effective
- controller-effective only

3. Expose this taxonomy in operator truth where practical.

### Exit Criteria

- no critical actuator remains ambiguous about whether it is truly live-adjustable
- no `effective` field is reported without an explicit semantics class

## Phase 2: Make Stage Autonomy Explicit

### Objective

Ensure each major stage is independently recoverable and refermable.

### Scope

Stages:
- watcher ingress
- graph ingestion
- graph projection
- vector preparation
- ready queue
- GPU embed lane
- persist/finalize lane

### Tasks

1. Define stage invariants
- ownership
- durable state
- stale detection
- requeue path
- success condition

2. Define failure windows
- before claim
- after claim before write
- after write before acknowledgment
- after partial completion

3. Add reconciliation checks
- orphan detection
- stale inflight detection
- missing queue ownership
- duplicate or contradictory ownership

4. Add stage-level repair paths
- deterministic
- idempotent
- bounded

### Exit Criteria

- every stage has documented invariants
- every stage has a provable repair path
- no known orphan state remains without explicit reconciliation coverage

## Phase 3: Implement True Quiescent Mode

### Objective

Reduce idle heat, wakeups, and noise without harming useful wake-up responsiveness.

### Tasks

1. Inventory background loops
- identify polling intervals
- identify always-on work
- classify hot vs idle necessity

2. Centralize idle cadence policy
- one quiescent policy
- backoff tiers
- wake-up triggers

3. Add quiescent observability
- current profile
- wakeup source
- idle dwell time
- recent hot-path resumes

4. Add explicit wake-up guarantees
- graph work arrival
- semantic work arrival
- operator interaction
- recovery override

5. Add hard quiescent metrics
- idle CPU %
- wakeups per second
- background loop iterations per minute
- resume latency p50/p95
- GPU idle utilization or power proxy at idle

### Exit Criteria

- host idle behavior is materially quieter/cooler
- wake-up time remains within declared bounds

## Phase 4: Throughput Optimization Under Safety

### Objective

Maximize useful throughput without violating memory or interactivity constraints.

### Tasks

1. Prove GPU feed quality
- ready reserve sufficiency
- low idle GPU time under backlog
- coherent batch composition
 - bounded `ready_queue == 0` dwell time

2. Refine graph-first policy
- fast graph utility
- bounded starvation of semantic completion

3. Refine adaptive batch sizing
- use model and host signals
- preserve memory safety
- expose clamp causes

4. Validate host-exclusive GPU behavior
- one instance owns GPU vector lane
- other instance remains operational without false throughput assumptions

5. Add host-class throughput targets
- minimum semantic chunks/sec by host class
- max `graph_ready` latency by host class
- minimum GPU utilization floor under sustained backlog where GPU is present

### Exit Criteria

- throughput improves or remains stable across target host profiles
- no unsafe VRAM or RAM regressions appear

## Phase 5: Qualification And Evidence

### Objective

Replace intuition with proof.

### Tasks

1. Build repeatable qualification suites
- single-instance GPU
- dual-instance GPU host
- CPU-only
- cold rebuild
- idle soak
- recovery interruption matrix

2. Split dual-instance qualification into explicit modes
- `live idle + dev hot`
- `live interactive + dev rebuild`
- `live mcp_only + dev full`
- GPU owner flip / non-owner fallback

3. Define pass/fail metrics
- graph time-to-useful-state
- semantic chunks per second
- files to vector-ready
- GPU utilization floor under backlog
- RAM/VRAM safety margins
- MCP latency guard
- idle CPU and thermal proxy indicators
 - wakeups/sec at quiescence
 - ready queue starvation dwell time
 - recovery convergence time
 - duplicate work rate
 - multi-instance interference deltas

4. Publish qualification artifacts
- summary
- regressions
- host metadata
- operator interpretation

### Exit Criteria

- each suite produces machine-readable evidence
- regressions are visible before promotion

## Phase 6: Decide Whether SOTA Is Proven

### Objective

Make the `SOTA` claim evidence-based.

### Gate

The label is justified only if:
- authority unification is effectively complete
- stage autonomy is demonstrated
- quiescent mode is proven
- throughput and safety metrics are strong
- multi-instance behavior is controlled
- qualification is green and repeatable

Otherwise:
- keep the label as `near-SOTA target`
- do not overclaim

## Risks

- over-unification that removes needed local safety clamps
- quiescent mode hurting wake-up responsiveness
- benchmark wins on one host but regressions on another
- hidden coupling between recovery and scheduling decisions
- operator truth overstating what is actually applied

## Recommended Execution Order

1. finish authority unification
2. finish stage invariants and repair paths
3. implement quiescent mode
4. optimize throughput only after the above
5. run full qualification matrix
6. decide if the SOTA claim is earned
