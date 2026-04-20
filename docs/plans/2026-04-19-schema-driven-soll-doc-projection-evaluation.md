# Schema-Driven SOLL Doc Projection Evaluation

Date: 2026-04-19
Status: evaluation
Method: idea-to-delivery

## Question

Should the derived SOLL documentation stop relying on UI-local hierarchy rules and instead project from the canonical structural schema/policy itself?

The target property is:

- more abstract
- less hardcoded
- less drift-prone
- still stable and readable for humans

## Options

### Option A: hardcoded hierarchy in the generator

Example:

- `Project -> Vision`
- `Vision -> Pillar`
- `Pillar -> Requirement`
- `Requirement -> Decision/Validation/...`

Pros:

- simple
- deterministic
- fast to ship

Cons:

- generator owns structure knowledge
- guaranteed drift risk when SOLL evolves
- not truly agnostic
- duplicates structure outside the canonical model

Verdict:

- acceptable as a bootstrap
- not the healthiest long-term architecture

### Option B: separate UI policy/config

The generator would read a dedicated projection policy file/table that declares:

- parent/child type pairs
- hierarchy priority
- lateral relations
- breadcrumb preference

Pros:

- less hardcoded in the generator
- easier to evolve than pure code
- deterministic

Cons:

- still a second truth if it is not derived from canonical schema
- can drift from real SOLL relation policy
- improves maintainability, but not model integrity

Verdict:

- better than A
- still inferior to using the canonical structural source directly

### Option C: schema/policy-driven projection

The generator reads the canonical structural relation policy and derives the reading hierarchy from it.

The projection combines:

- canonical schema / relation policy
- actual SOLL instances

Pros:

- best structural integrity
- generator stops owning ontology
- when the canonical model evolves, docs can evolve with it
- better reuse across all projects
- reduces architectural duplication

Cons:

- schema alone is not enough unless it carries projection metadata
- some relations are valid but not hierarchy-primary
- multi-parent and lateral relations still need deterministic projection rules

Verdict:

- strongest target architecture
- but only if canonical policy is rich enough for projection

## Core Finding

The best architecture is not:

- hardcoded generator logic
- nor free heuristic inference from raw graph data

The healthiest architecture is:

- schema/policy-driven projection

More precisely:

- the canonical relation policy becomes structural truth
- the generator derives hierarchy from that truth
- actual nodes/edges provide the instance data
- a small projection layer resolves:
  - hierarchy-primary vs lateral/supporting
  - parent preference ordering
  - root semantics such as `GLO`

## Important Nuance

“Read the schema” is only sufficient if the schema/policy exposes enough semantics.

For the doc generator to be truly clean, the canonical structural source must provide at least:

- allowed endpoint types
- allowed relations
- default relation
- hierarchy relevance
- lateral/supporting classification where needed
- deterministic parent preference if multiple canonical parents are possible

If the current canonical source does not expose those distinctions, then:

- the generator would still need local guesses
- which would reintroduce hidden ontology

Therefore the real best option is:

- schema-driven projection with explicit projection metadata inside the canonical policy

Minimum canonical projection metadata should include:

- hierarchy-primary flag or role
- parent preference rank
- breadcrumb eligibility
- root eligibility
- lateral/supporting classification

## Why This Is Better Than the Current State

Compared with the current UI-local hierarchy helper:

- fewer structural assumptions live in the renderer
- lower drift risk
- easier long-term evolution
- more project-agnostic
- stronger conceptual integrity

Compared with a separate projection config:

- fewer truths to maintain
- less chance of policy mismatch
- better alignment with MCP surfaces like `soll_relation_schema`

## Transitional Strategy

The best target architecture is not automatically the best immediate rewrite strategy.

The healthy migration path is:

1. keep the current hardcoded hierarchy as bootstrap
2. enrich the canonical relation policy with projection metadata
3. switch the generator to consume canonical projection data
4. remove the UI-local hierarchy helper only once parity is proven

So the right conclusion is:

- best target architecture: schema-driven projection
- best immediate transition: hybrid staged migration

## Major Risks

### Risk 1: current canonical relation policy may be too thin

The current relation policy mostly exposes valid pairings and default relations.

That is not yet the same as a full projection model.

So the idea is sound, but may require canonical policy enrichment first.

### Risk 2: hierarchy is not identical to validity

A valid relation is not automatically the best tree edge for human reading.

Therefore the canonical source must distinguish:

- valid relation
- default relation
- hierarchy-primary projection role

And the projection must stay stable:

- the same node must not oscillate between parents across regenerations

### Risk 3: over-generalization can damage readability

A fully generic graph-to-tree projection can become unstable or confusing if it tries to infer too much.

So “dynamic” must still be constrained by explicit canonical semantics.

## Conclusion

This idea is the healthiest architectural direction among the available options.

But the honest conclusion is not:

- “it has no defect”

The honest conclusion is:

- it is the best target architecture
- provided the canonical schema/policy is enriched enough to drive projection
- and provided the generator is not asked to invent hierarchy heuristically

In short:

- best option: yes
- no better option currently identified: yes
- no major defect: no

The remaining major condition is:

- enrich the canonical structural policy so the projection can truly be derived from it

Additional clarification:

- `GLO` should remain a derived reading root unless and until the canonical model explicitly represents a portfolio layer
