# Runtime Parameter Inventory

## Purpose

This inventory is the first bounded step toward runtime orchestrator unification.

It classifies important Axon runtime parameters as:

- `fixed`
- `bounded`
- `computed`

and identifies where they currently live.

## Classification Rules

### `fixed`

Use `fixed` when the parameter is part of the model contract or a hard product invariant and should not be tuned dynamically at runtime.

### `bounded`

Use `bounded` when the parameter may vary, but only inside explicit safety or contract limits.

### `computed`

Use `computed` when the parameter should be derived from:

- host capabilities
- runtime backlog
- pressure signals
- latency goals
- recovery state

## Inventory

| Parameter | Current Location | Class | Why |
| --- | --- | --- | --- |
| embedding dimension | `embedder` / model contract | `fixed` | dictated by the embedding model |
| model max input length | `embedder` | `fixed` | dictated by the model/runtime contract |
| query worker baseline | `runtime_profile` / `embedder` | `bounded` | usually small and stable, but may still be guarded by latency constraints |
| vector workers | `runtime_profile` + `runtime_tuning` + `embedder` | `computed` | must depend on host, VRAM, backlog, and interactive pressure |
| graph workers | `runtime_profile` + env | `computed` | must depend on backlog, graph value urgency, and available CPU |
| chunk batch size | `runtime_profile` + `runtime_tuning` + `vector_control` | `computed` | should adapt to tokens, GPU behavior, and pressure |
| file vectorization batch size | `runtime_profile` + `runtime_tuning` + `vector_control` | `computed` | should adapt to throughput and underfeed signals |
| graph batch size | `runtime_profile` + env | `bounded` | can be tuned, but under tighter CPU/latency constraints |
| max chunks per file | `embedder` / env | `bounded` | should stay explicit as a guard against runaway files |
| max embed batch bytes | `embedder` / `vector_control` | `bounded` | direct VRAM safety guard |
| vector ready queue depth | `runtime_tuning` | `computed` | should follow GPU feed and prepare/persist behavior |
| vector persist queue bound | `runtime_tuning` | `bounded` | bounded by memory and persist safety, but target may be orchestrated |
| vector max inflight persists | `runtime_tuning` | `bounded` | safety-oriented concurrency limit |
| embed micro batch max items | `runtime_tuning` | `computed` | should follow token coherence and GPU efficiency |
| embed micro batch max total tokens | `runtime_tuning` | `computed` | should track model constraints plus throughput behavior |
| RAM budget | `runtime_profile` | `bounded` | derived from host, but should remain under explicit headroom rules |
| ingestion memory budget | `runtime_profile` | `bounded` | derived from RAM budget, not freely tunable |
| queue capacity | `runtime_profile` | `bounded` | derived from host memory, but bounded for safety |
| utility-first scheduler state | `vector_control` | `computed` | directly derived from live runtime state |
| semantic ready reserve target | `vector_control` | `computed` | derived from backlog and service pressure |
| GPU worker admission | `vector_control` | `computed` | should follow service pressure and backlog, subject to hard guard |
| quiescent backoff intervals | `vector_control` / background loops | `computed` | should depend on true idle state, not remain static forever |
| watcher debounce | `main_background` / watcher | `bounded` | event-system contract plus responsiveness trade-off |
| optimizer action profile targets | `optimizer` | `computed` | should be proposals from a canonical controller |

## Current Observations

### Healthy structure already present

- Axon already distinguishes:
  - host detection
  - runtime tuning
  - scheduler logic
  - live pressure signals
- the correct product split already exists:
  - `graph_ready` first
  - `vector_ready` second

### Current contradictions

- `vector_workers` is the clearest example of authority fragmentation:
  - seeded in `runtime_profile`
  - mutable in runtime tuning
  - clamped in `embedder`
  - admitted again in `vector_control`
- `chunk_batch_size` and `file_vectorization_batch_size` are partly orchestrated, but their ultimate effective value still depends on multiple layers
- idle/quiescent behavior is not yet centralized; multiple background loops keep their own polling cadence

## Immediate Conclusion

The runtime already contains many of the right pieces.

The problem is not lack of signals.

The problem is that several parameters that should now be `computed` still behave as if they were partly hard-coded strategic choices.

That is the main unification target.

## Priority Unification Targets

The first parameters that should be unified under canonical orchestration are:

- `vector_workers`
- `chunk_batch_size`
- `file_vectorization_batch_size`
- idle/quiescent cadence

These are the highest-value targets because they currently combine:

- strong throughput impact
- visible runtime ambiguity
- real risk of hidden strategic overrides

## Required Traceability Upgrade

For every high-value runtime parameter, Axon should eventually expose four stages clearly:

- `seed`
  - initial boot assumption
- `target`
  - value computed by canonical orchestration
- `effective`
  - value actually applied at runtime
- `clamp_visible`
  - whether any divergence is surfaced with a reason

Without these four stages, the inventory remains descriptive but not yet operationally enforceable.
