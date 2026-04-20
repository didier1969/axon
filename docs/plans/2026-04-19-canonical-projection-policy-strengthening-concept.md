# Canonical Projection Policy Strengthening Concept

Date: 2026-04-19
Status: concept
Method: idea-to-delivery

## Intent

Preserve the ability to generate very high-quality SOLL-derived documentation immediately, while progressively removing UI-local structure assumptions by strengthening the canonical structural policy itself.

This is not a docs-only change.

It is a structural product change so that:

- `soll_relation_schema`
- mutation guidance
- validation guidance
- derived docs

all consume the same enriched canonical structural truth.

## Current Reality

Today, Axon already has:

- a canonical relation policy for valid source/target pairs
- default relations
- guidance payloads derived from that policy
- a high-quality derived docs generator

But the current relation policy is still thinner than the needs of a hierarchy-aware documentation projection.

The derived docs generator therefore still contains UI-local hierarchy knowledge.

This is acceptable for quality now, but not the healthiest long-term architecture.

## Product Goal

Reach a state where the docs generator no longer owns hierarchy semantics.

Instead:

- canonical relation policy defines structural truth
- projection metadata defines hierarchy semantics
- the docs generator only renders the derived reading surface

This keeps the output:

- high quality for humans
- stable across regenerations
- more agnostic across projects
- less prone to structural drift

## Non-Goals

- do not block current derived docs quality while the canonical model is being strengthened
- do not rewrite all docs generation in one risky wave
- do not turn derived docs into canonical truth
- do not introduce a second policy file that can drift from the relation policy
- do not require a server-rendered UI

## Core Decision

The target architecture is:

- canonical relation policy
  + explicit projection metadata
  + consumed by all structural surfaces

not:

- hardcoded UI hierarchy
- nor a separate docs-only projection config

## What Must Become Canonical

The canonical structural policy should be enriched with projection semantics such as:

- hierarchy role
  - primary
  - lateral
  - supporting
- parent preference rank
- breadcrumb eligibility
- root eligibility
- child ordering hints
- visibility class for human reading

This does not replace validity semantics.

It extends them.

The policy must continue to answer:

- is this relation valid?
- what is the default relation?

And must additionally answer:

- is this relation hierarchy-primary for reading?
- if multiple parents exist, which one is preferred for breadcrumbing?
- should this node appear in the main tree or in supporting detail space?

## Transitional Product Stance

The current docs generator can already produce high-quality documentation.

Therefore the roadmap should be:

1. keep current generator quality intact
2. enrich canonical structural policy
3. add a schema-driven projection layer behind the same docs UI
4. delete the UI-local hierarchy helper only after parity is proven

This means:

- immediate quality is preserved
- structural health improves without blocking delivery

## Why This Is Better

Compared with staying hardcoded:

- fewer hidden assumptions
- less drift
- easier evolution of SOLL semantics

Compared with a separate projection config:

- one structural truth instead of two
- better coherence with MCP tools
- lower maintenance burden

Compared with fully heuristic projection from data:

- much more stable
- much more readable
- less chance of oscillating parent/child interpretations

## Immediate Practical Outcome

We do not need to wait for the canonical strengthening to keep producing excellent docs.

So the practical doctrine is:

- short term: continue using the current generator, but treat it as transitional
- medium term: introduce canonical projection metadata and consume it
- long term: remove UI-local hierarchy assumptions completely

## Risks

### 1. Canonical policy enrichment can sprawl

If projection metadata is added carelessly, the relation policy may become overloaded.

Mitigation:

- keep projection metadata small and explicit
- separate validity and projection concerns in the same canonical model

### 2. Migration can silently diverge

If the old generator and the new schema-driven projection differ, the docs may change unexpectedly.

Mitigation:

- run parity tests during migration
- compare outputs page by page on representative projects

### 3. Not every valid relation should shape the main tree

A valid relation can still be lateral for human reading.

Mitigation:

- make “hierarchy-primary vs lateral/supporting” canonical, not heuristic

## Success Criteria

This theme is successful when:

- docs quality remains high throughout the migration
- derived docs no longer need hardcoded hierarchy rules
- `soll_relation_schema` and derived docs tell the same structural story
- parent selection remains stable across regenerations
- new SOLL types or relations can be introduced without patching a docs-only ontology

## Final Position

Yes:

- we can already work now and create very high-quality documentation

Also yes:

- the healthiest structural future is to enrich the canonical relation policy so docs projection is derived from it

Therefore:

- immediate quality work and structural reinforcement are compatible
- they should be handled as one phased roadmap, not as competing choices
