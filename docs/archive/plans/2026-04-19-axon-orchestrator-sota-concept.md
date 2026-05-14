# Axon Orchestrator To SOTA Concept

Date: 2026-04-19
Project: AXO
Status: concept draft

## Thesis

Axon's orchestrator is no longer naive, but it is not yet SOTA.

It already has:
- meaningful runtime observability
- partial authority unification over critical parameters
- utility-first scheduling
- GPU exclusivity on shared hosts
- stronger recovery semantics than before

It still lacks the properties required to call it state of the art:
- one canonical runtime authority across the full pipeline
- fully autonomous, restart-safe, independently recoverable stages
- true quiescent idle behavior
- end-to-end measured optimality under realistic host contention
- proof that decisions are applied, not only computed and reported

## What SOTA Means For Axon

For Axon, `SOTA` does not mean novelty for its own sake.

It means:
- the orchestrator computes nearly all runtime decisions from explicit bounds and live signals
- every stage can stop, restart, and converge back to a stable pipeline state
- graph utility arrives quickly
- semantic throughput remains high without violating VRAM/RAM safety
- idle behavior is quiet and cool
- multi-instance coordination is explicit and safe
- operator truth exposes target, effective value, and clamp reason for every meaningful actuator
- performance claims are backed by repeatable qualification

It also means Axon can explicitly classify every actuator as one of:
- live-adjustable without restart
- restart-bound
- safety-clamped locally
- measured-effective
- controller-effective only

## Fixed, Bounded, Computed

### Fixed

These are not free tuning variables:
- model input and output contract
- embedding dimension
- model-specific max length
- storage representation

### Bounded

These must stay inside declared safety envelopes:
- max VRAM pressure
- max RAM pressure
- max embed batch bytes
- max vector workers per host class
- max persist pressure
- max interactive latency budget

### Computed

These should converge toward orchestrator ownership:
- vector workers
- graph workers
- chunk batch size
- file vectorization batch size
- ready queue depth
- persist queue bound
- max inflight persists
- semantic cadence
- quiescent cadence
- graph-first vs semantic-refill vs balanced mode

Each computed actuator must also declare:
- `computed_by`
- `applied_by`
- `may_clamp_if`
- `observed_effective_from`

## Current Reality

Current Axon is best described as:
- architecturally aligned with the right direction
- partially unified in runtime authority
- not yet fully autonomous per stage
- not yet fully proven under hostile or mixed conditions

The main gaps are:

1. Authority is still only partially unified
- some values are computed centrally
- some are still clamped or effectively fixed in local components

2. Applied truth is still incomplete
- several values show runtime tuning truth rather than independently observed effective truth

3. Idle behavior is not yet a first-class contract
- the runtime still wakes more often than it should
- operator comfort and host thermals are not yet treated as hard success criteria

4. Stage autonomy is not yet proven enough
- recovery has improved
- but full per-stage refermability is not yet certified

5. Optimality is not yet demonstrated
- coherence is stronger
- SOTA requires measured comparative qualification

## Non-Goals

This wave should not:
- replace the orchestrator wholesale
- introduce a second strategic scheduler
- chase theoretical optimality without runtime proof
- optimize only for one laptop class
- hide clamps or safety degradations from operators

## Target End State

The target system has these properties:
- one canonical orchestrator computes all strategic runtime targets
- local components only apply or safety-clamp
- every clamp is visible
- every stage has explicit recovery and reconciliation invariants
- GPU vectorization is host-exclusive where required
- graph utility is prioritized early
- semantic throughput is maximized safely
- idle mode becomes truly quiescent
- qualification can prove behavior across host classes and instance modes

## Success Criteria

The orchestrator can be called SOTA only if all are true:

1. Runtime authority
- every critical actuator exposes `seed`, `target`, `effective`, `clamp_reason`
- no hidden strategic caps remain in local code paths

2. Stage autonomy
- each major stage can crash or pause independently and converge back without manual repair

3. Idle quality
- background heat/noise drops materially when no work is present
- wake-up latency remains acceptable
 - idle CPU and wakeups stay below declared thresholds

4. Throughput quality
- graph utility remains fast
- GPU stays sufficiently fed under backlog
- memory safety remains inside declared bounds
 - queue starvation dwell time stays bounded

5. Multi-instance safety
- concurrent runtimes cannot silently corrupt throughput assumptions

6. Qualification
- repeatable runtime suites prove the above on at least:
  - single-instance GPU
  - dual-instance shared host
  - CPU-only
  - rebuild from backlog

## Recommended Strategy

Do not claim SOTA early.

Use a phased proof strategy:
- finish authority unification
- formalize actuator semantics and live-apply taxonomy
- finish stage recovery invariants
- add true quiescent mode
- qualify under realistic runtime matrices
- only then decide whether the `SOTA` label is justified
