# Runtime Authority Map

## Purpose

This document records who currently:

- computes a target
- applies a target
- may clamp a target

for the most important Axon runtime parameters.

It is meant to expose hidden strategic overrides before runtime unification work continues.

## Authority Fields

- `computed_by`
  - the layer that should decide the desired operating point
- `applied_by`
  - the layer that actually installs the live value
- `may_clamp_if`
  - the only valid reasons to diverge from the desired value

## Current Authority Map

| Parameter | computed_by | applied_by | may_clamp_if |
| --- | --- | --- | --- |
| `vector_workers` | currently split between `runtime_profile`, `optimizer`, `vector_control` | env bootstrap + `runtime_tuning` + `embedder` | explicit GPU/VRAM safety only |
| `graph_workers` | `runtime_profile` seed, later effectively static | env bootstrap | host worker budget, graph disabled canonically |
| `chunk_batch_size` | `runtime_profile` seed + `vector_control` + runtime tuning | runtime tuning snapshot + embedder lane config | model contract, max batch bytes, explicit VRAM pressure |
| `file_vectorization_batch_size` | `runtime_profile` seed + `vector_control` + runtime tuning | runtime tuning snapshot + embedder lane config | memory safety, queue pressure, persist backpressure |
| `vector_ready_queue_depth` | optimizer/runtime tuning | runtime tuning snapshot | queue memory safety |
| `vector_persist_queue_bound` | optimizer/runtime tuning | runtime tuning snapshot | memory safety, persist stability |
| `vector_max_inflight_persists` | optimizer/runtime tuning | runtime tuning snapshot | persist safety |
| `embed_micro_batch_max_items` | optimizer/runtime tuning | runtime tuning snapshot | model and memory safety |
| `embed_micro_batch_max_total_tokens` | optimizer/runtime tuning | runtime tuning snapshot | model token limit and memory safety |
| utility scheduler state | `vector_control` | `vector_control` | none except explicit recovery override |
| GPU worker admission | `vector_control` | worker admission gate in vector worker loop | service pressure, interactive priority, explicit GPU safety |
| idle/quiescent cadence | currently fragmented across background loops | individual loops | only liveness and wake-up responsiveness |

## Main Problems Exposed

### 1. Strategic decision and low-level clamp are mixed

The biggest current issue is not that Axon has clamps.

It is that some clamps are still behaving like strategic policy.

Example:

- `vector_workers` was effectively being decided in more than one place
- that made the orchestrator advisory instead of authoritative

### 2. `utility-first` is not yet the sole strategic scheduler

`vector_control` already carries the best current scheduling language:

- `graph_priority`
- `semantic_refill_protection`
- `balanced_drain`
- `recovery_override`

But the wider runtime still contains strategic influence elsewhere.

So today:

- `utility-first` is a strong scheduling component
- but not yet the single canonical controller

### 3. Idle behavior has no canonical authority

Background loops currently keep their own cadence and wake policy.

That means:

- quiescent behavior is emergent
- not intentional

## Canonical Target State

### Runtime orchestrator

Recommended target:

- one canonical runtime authority computes the target operating point
- this may initially be a façade above:
  - `optimizer`
  - `vector_control`
  - runtime tuning state

### Lower layers

Lower layers should:

- apply
- enforce hard local safety guards
- report when they clamp

but they should not silently redefine policy.

## Clamp Taxonomy

Not every divergence is the same.

Axon should distinguish explicitly between:

- `hard_safety_clamp`
  - local, non-negotiable, safety-driven
- `controller_requested_degrade`
  - strategic slowdown or reduction explicitly decided by canonical orchestration
- `recovery_override`
  - preemptive recovery behavior outside normal optimization policy

`recovery_override` must not be treated as a normal clamp.
It is a higher-priority runtime safety/recovery mode.

## Required Rule

For every dynamic parameter, Axon should eventually expose:

- target value
- effective applied value
- clamp reason, if different

Without that, the system stays partially opaque even if its logic improves.
