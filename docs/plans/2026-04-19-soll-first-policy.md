# SOLL-First Policy For Axon

## Purpose

Axon should use SOLL as its primary durable documentation system whenever practical.

This policy narrows the role of markdown documents so that durable truth stops drifting across surfaces.

## Canonical Rule

For the Axon project:

- durable truth belongs in SOLL first
- markdown is secondary unless explicitly marked otherwise

## Durable Truth That Must Prefer SOLL

The following categories should normally live in SOLL:

- target outcomes
- strategic principles
- testable capabilities
- architecture and engineering decisions
- durable engineering constraints
- domain concepts
- milestones
- proof and validation

In SOLL terms, that means preferential use of:

- `Vision`
- `Pillar`
- `Requirement`
- `Decision`
- `Guideline`
- `Concept`
- `Milestone`
- `Validation`

## What Markdown Is Still For

Markdown remains useful for:

- transient implementation plans
- operator notes
- temporary investigations
- review notes
- human-friendly derived views
- handoff notes during migration

## Mandatory Markdown Tags

Any markdown artifact that contains project truth must be explicitly tagged in intent, at least operationally, as one of:

- `canonical`
- `derived`
- `transitional`

### Meaning

`canonical`

- allowed only when the truth is intentionally kept outside SOLL for now
- must be rare

`derived`

- generated or explanatory surface
- never the source of truth

`transitional`

- temporarily carries truth during migration
- must declare how and when that truth moves into SOLL or is retired

## Minimal Enforcement Rule

Any new markdown artifact that carries durable project truth is non-compliant unless its role is explicit from creation time.

At minimum, the role must be verified:

- during review
- or during commit preparation for documentation-sensitive work

The minimum accepted roles are:

- `canonical`
- `derived`
- `transitional`

## Anti-Double-Truth Rule

No durable project truth may remain:

- implicitly canonical in markdown
- while also being separately maintained in SOLL

If both exist, one must be declared:

- canonical
- the other derived or transitional

## Promotion Rule

A markdown artifact should be promoted into SOLL when it contains any of the following and is expected to remain relevant beyond the immediate wave:

- a stable architecture decision
- a stable engineering rule
- a durable requirement
- a durable rationale
- a lasting milestone or validation

## Derived Docs Rule

Derived documentation such as generated navigable docs must remain clearly non-canonical.

For Axon, this includes:

- `docs/derived/...`
- generated HTML/Mermaid views
- explanatory exports

These are reading surfaces, not source-of-truth surfaces.

## Plans Rule

Implementation plans remain useful, but they are not automatically durable truth.

After delivery:

- either the durable parts are promoted into SOLL
- or the plan stays explicitly transitional/archive material

## Practical Operator Rule

When documenting Axon:

1. first ask whether the information is durable project truth
2. if yes, prefer SOLL
3. if markdown is used anyway, mark its role clearly
4. avoid maintaining the same durable truth independently in both places

## Immediate Application To Axon

The following areas should progressively become SOLL-first:

- runtime orchestration doctrine
- graph/value-first product rationale
- vectorization safety constraints
- quiescent-mode intent
- durable release/promotion doctrine where relevant

## Success Condition

A new engineer or an LLM should be able to recover Axon’s durable intent primarily from SOLL, with markdown acting as:

- a temporary working surface
- an operator surface
- or a derived reading surface

but not as an uncontrolled second canon.
