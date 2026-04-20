# IST Utility-First Scheduling Implementation Plan

## Objective

Refine Axon's existing scheduler so that:

- structural indexing reaches `graph_ready` as quickly as possible for client utility
- the semantic/GPU lane remains continuously fed through a bounded reserve
- fairness prevents indefinite starvation of already graph-ready files
- correctness and recovery overrides remain authoritative

## Scope

This wave is a scheduling-policy refinement.

It is distinct from, but compatible with, the orphaned semantic ownership repair wave on `feat/ist-resilience-recovery`.

## Validation Matrix

### Preserve

- existing governor and optimizer stack
- interactive safety guards
- vector worker admission controls
- bounded prepare / ready / persist lanes

### Improve

- time to `graph_ready`
- avoidance of GPU underfeed
- fairness for old semantic backlog
- operator visibility into scheduling intent

## Plan

### 1. Formalize scheduler states

Introduce one explicit policy surface over the existing runtime:

- `graph_priority`
- `semantic_refill_protection`
- `balanced_drain`

These states should be derived from existing signals, not from a new ownership model.

### 2. Define canonical entry and exit thresholds

Use existing runtime metrics to drive transitions:

- `file_vectorization_queue_depth`
- `ready_queue_depth_current`
- `prepare_inflight_current`
- `persist_queue_depth_current`
- `canonical_chunks_embedded_last_minute`
- `gpu_utilization_ratio`
- `interactive_priority`
- service pressure

Initial decision rules:

- enter `semantic_refill_protection` when:
  - `file_vectorization_queue_depth >= 16`
  - ready reserve is empty or materially below target
  - prepare inflight is low
  - and one or more underfeed symptoms are true:
    - semantic throughput is stalled
    - GPU is underfed
    - vector workers are starved

Backlog depth alone must not trigger the mode.

- exit `semantic_refill_protection` only when:
  - ready reserve is back above target
  - persist lane is not congested
  - the healthy condition holds for a full evaluation window

- stay or return to `graph_priority` when:
  - graph backlog exists
  - semantic feed floor remains satisfied
  - service pressure remains healthy

### 3. Add hysteresis and fairness

Do not transition on single-sample thresholds.

Implement:

- low-watermark / high-watermark hysteresis
- explicit minimum hold window per state
- fairness counters or age-based pressure for:
  - oldest graph-pending file
  - oldest semantic-pending file

Required done condition:

- no indefinite starvation of semantic backlog while new graph work keeps arriving
- fairness ages actively influence scheduling decisions

### 4. Bind policy to existing actuators

Prefer refining existing controls over adding new machinery.

Likely actuator surface:

- vector worker admission
- file vectorization batch size
- vector ready queue target depth
- maybe graph projection suppression policy only if directly needed

Thresholds and targets should remain profile-aware and hardware-aware where possible.

Do not create a second independent scheduler.

### 5. Publish operator truth

Expose the scheduling policy explicitly through runtime truth surfaces such as `status`:

- active scheduler state
- semantic underfeed detected or not
- reserve target
- fairness ages
- override cause when recovery supersedes optimization
- hold-window status
- active threshold/profile basis

### 6. Re-qualify behavior

Validation must prove more than raw throughput.

Required proofs:

1. new files reach `graph_ready` faster or at least not worse under load
2. GPU/vector workers do not idle unnecessarily when semantic backlog exists
3. semantic backlog still completes over time
4. interactive pressure still dominates when needed
5. scheduler does not oscillate rapidly between modes
6. recovery/correctness overrides still take precedence
7. persist congestion is not worsened by refill protection
8. fairness ages remain bounded in steady state

## Risks

- over-biasing graph work and starving semantic completion
- overreactive thresholds causing mode thrash
- hiding persist bottlenecks by only filling prepare/ready buffers harder
- conflating correctness bugs with scheduler decisions
- using static thresholds that fail across different hardware/runtime profiles

## Rollout Strategy

1. add tests for state selection and hysteresis
2. bind policy to existing actuators behind explicit state reporting
3. validate on `dev` with:
   - fresh ingestion
   - rebuild-like backlog
   - interactive pressure
4. compare:
   - time to `graph_ready`
   - semantic underfeed rate
   - vector worker idle behavior
5. only then promote

## Done Criteria

- scheduler states are explicit and operator-visible
- graph-first usefulness is implemented as a measured policy, not a slogan
- semantic feed floor prevents GPU starvation
- fairness prevents indefinite semantic starvation
- hysteresis prevents rapid oscillation
- the policy uses the current scheduling foundation rather than bypassing it
