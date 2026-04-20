# Triage And Gates

## Triage Quick Guide

### `light`

Use when:

- the idea is still fuzzy
- the blast radius is low
- the main value is concept clarification

Stop after:

- concept convergence
- explicit summary to the user

### `standard`

Use when:

- the work is meaningful but bounded
- concept, plan, and implementation are all needed
- independent critique is useful but not mission-critical

Required:

- concept doc
- plan doc
- implementation validation

### `full`

Use when:

- runtime, migration, operator workflow, live data, or large surface area is involved
- wrong sequencing or weak validation could create material damage

Required:

- concept doc
- plan doc
- explicit validation matrix
- strongest review discipline

## Phase Gate Checklist

### Concept Gate

Pass only if:

- constraints are explicit
- non-goals are explicit
- reuse vs change is explicit
- two reviews found no blocker

### Plan Gate

Pass only if:

- dependency ordering is explicit
- risky migrations are identified
- validation path is explicit
- rollback or containment is explicit when needed
- two reviews found no blocker

### Execution Gate

Pass only if:

- implementation matches the approved plan or deviations are documented
- validation evidence exists
- docs are updated where needed
- findings are resolved or explicitly accepted

### Completion Gate

Pass only if:

- evidence exists
- residual risks are stated
- integration state is clear
