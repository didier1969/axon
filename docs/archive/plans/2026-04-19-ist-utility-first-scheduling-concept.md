# IST Utility-First Scheduling Concept

## Problem

Axon's current runtime already has a strong scheduling stack:

- interactive pressure guards
- vector drain states
- GPU worker admission
- prepare / ready / persist bounds
- optimizer profiles and a governor

But the current decision model still optimizes mostly for technical drain efficiency, not for the earliest product utility visible to clients.

For clients, the system becomes materially useful as soon as files are:

- discovered
- structurally indexed
- `graph_ready = true`

even if semantic vectorization is still incomplete.

The long pole remains the semantic/GPU lane. That lane must stay fed, but it should not dominate CPU usage so aggressively that graph usefulness arrives later than necessary.

## Core Position

We should **not** replace the current scheduler with a new system.

We should **refine** it by changing the governing objective:

- prioritize low time-to-`graph_ready`
- while preserving a bounded semantic feed floor for the GPU lane
- and preserving fairness so old semantic backlog cannot starve forever

This is an adjustment of policy and invariants, not a scheduler rewrite.

## Why This Is Better Than the Alternatives

### Better than "always drain semantics first"

That policy optimizes downstream completion too early and delays the point where Axon already becomes useful through graph truth.

### Better than "finish graph entirely, then do semantics"

That policy risks GPU starvation, idle vector workers, and unbounded semantic lag.

### Better than rewriting the scheduler

The existing runtime already exposes the right control surfaces:

- `file_vectorization_queue_depth`
- `ready_queue_depth_current`
- `prepare_inflight_current`
- `persist_queue_depth_current`
- interactive priority state
- service pressure
- governor profile adjustments

The healthier move is to align these existing signals with product utility.

## Target Scheduling Model

The target runtime policy should have three explicit product-facing modes:

1. `graph_priority`
   - dominant objective: minimize time to structural usefulness
   - CPU stays primarily available for new indexing / graph completion
   - semantic lane must still retain a protected minimum feed floor

2. `semantic_refill_protection`
   - dominant objective: prevent GPU underfeed
   - entered when semantic backlog exists but the CPU-side prepare/ready lane falls below a healthy reserve
   - this is a bounded refill mode, not a permanent semantic drain takeover

3. `balanced_drain`
   - dominant objective: preserve both freshness and completion
   - used when graph ingress is healthy and semantic buffers are healthy

## Required Invariants

### Product Utility Invariant

Newly discovered eligible files should reach `graph_ready` quickly and predictably.

### Semantic Feed Invariant

If semantic backlog exists, the semantic lane must never remain underfed long enough for GPU/vector workers to idle unnecessarily.

### Fairness Invariant

Already graph-ready files must still reach `vector_ready` within a bounded time.

New file freshness must not starve old semantic completion indefinitely.

These bounds must become active scheduling pressure, not only operator reporting.

### Recovery Override Invariant

Repair states override normal scheduling:

- orphaned vectorization ownership
- stale inflight semantic work
- semantic lane stalled despite backlog

Correctness beats optimization.

This override must remain a hard preemption path, not just another optimization signal.

## Control Law

This should be implemented with hysteresis, not single-threshold toggles.

It must also be age-bounded, not queue-depth-only.

### Enter `semantic_refill_protection` when all are true

- semantic backlog is meaningful
- ready reserve is empty or below a target floor
- prepare inflight is low
- one or more underfeed symptoms are visible:
  - GPU under-utilized
  - no recent semantic progress
  - vector workers effectively starved

Backlog depth alone is not sufficient.

### Exit `semantic_refill_protection` only when all are true

- ready reserve is back above target
- persist lane is not congested
- the healthy condition holds for a full evaluation window

### Stay or re-enter `graph_priority` when

- new graph work is present
- semantic feed floor is still satisfied
- service pressure remains healthy enough

This must be hysteresis-based to avoid oscillation.

Concrete implementation should use:

- low-watermark / high-watermark thresholds
- a minimum hold window before mode exit
- profile-aware and hardware-aware bounds where possible

## Non-Goals

- do not redesign watcher discovery
- do not merge this with the orphaned queue correctness fix
- do not invent a second scheduler beside the existing governor/optimizer stack
- do not optimize purely for benchmark throughput if it harms `graph_ready` latency

## Observability Requirements

The runtime must expose enough truth to show which objective is currently winning.

Recommended operator-visible state:

- `utility_first_scheduler_state`
- `semantic_underfeed`
- `semantic_ready_reserve_target`
- `oldest_graph_pending_age_ms`
- `oldest_semantic_pending_age_ms`
- `scheduler_override_reason`
- `scheduler_hold_window_ms`
- `scheduler_profile_basis`

This makes the policy auditable instead of heuristic and hidden.

## Decision

The current scheduler architecture is good enough to keep.

The correct next move is:

- strengthen the policy
- publish clearer invariants
- bind runtime transitions to measurable signals
- keep the semantic lane fed without sacrificing graph-first usefulness

Persist congestion must remain a first-class brake:

- if downstream persist pressure is rising, refill protection must not blindly push more CPU into prepare
- the scheduler must stabilize the downstream bottleneck before accelerating upstream feed

This is the healthiest path because it improves product behavior without discarding a capable scheduling foundation.
