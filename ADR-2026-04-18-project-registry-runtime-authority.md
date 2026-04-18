# ADR-2026-04-18: Project Registry Runtime Authority

## Status

Accepted on 2026-04-18.

## Context

Axon needs a strict and durable contract for project identity across three phases:

- cold state, when Axon is not running
- bootstrap, when Axon imports project identities
- runtime, when Axon resolves project scope on every operation

The repository already contains per-project `.axon/meta.json` files.
SOLL also persists project identities in `soll.ProjectCodeRegistry`.
Runtime keeps an in-memory clone for fast resolution.

This layering is valid only if the roles are explicit.
Otherwise the system drifts into competing truths, legacy names, and partial migrations.

The concrete failure observed on 2026-04-18 was exactly of that kind:

- legacy `project_slug` still existed in live SOLL tables
- runtime mutators expected `project_code`
- reservation of SOLL IDs failed before mutation jobs could be accepted

The vocabulary itself also matters.
Keeping both `project_slug` and `project_code` encourages future confusion and invalid migrations.

## Decision

Axon adopts the following authority model for project identity:

- cold import truth: per-project `.axon/meta.json`
- active persistent runtime truth: `soll.ProjectCodeRegistry`
- active in-memory runtime truth: memory clone of `soll.ProjectCodeRegistry`
- technical SOLL sequence registry: `soll.Registry`

Operational meaning:

- `.axon/meta.json` defines which projects are admissible for import
- at bootstrap, Axon imports those identities into `soll.ProjectCodeRegistry`
- once Axon is running, `soll.ProjectCodeRegistry` is the active persisted authority
- runtime memory mirrors `soll.ProjectCodeRegistry` for low-latency lookups
- `soll.Registry` is not a project identity registry; it only stores per-project SOLL sequence counters

Vocabulary rules:

- `project_code` is the only canonical field name for project identity
- `project_slug` is forbidden
- no schema, code path, fixture, or MCP payload may reintroduce `project_slug`

## Consequences

Positive:

- the system has one clear authority at each lifecycle phase
- project identity remains explicit and queryable in SOLL during runtime
- memory acceleration stays legitimate because it mirrors the active persisted registry
- the technical sequence registry is separated from identity concerns
- legacy naming ambiguity is removed

Costs:

- bootstrap and migration logic must actively normalize legacy SOLL tables
- any remaining legacy schema using `project_slug` must be rebuilt or migrated
- tests and fixtures must be kept aligned with `project_code` only

## Resynchronization Rules

- if `.axon/meta.json` changes while Axon is stopped, the next bootstrap imports the new truth
- if `.axon/meta.json` changes while Axon is running, Axon must not silently invent or mutate runtime truth
- runtime resynchronization must happen only through an explicit import/reload path or restart
- memory must never diverge intentionally from `soll.ProjectCodeRegistry`

## Implementation Notes

The runtime must guarantee all of the following:

1. bootstrap imports project identities from `.axon/meta.json` into `soll.ProjectCodeRegistry`
2. runtime lookups resolve through the in-memory registry cloned from `soll.ProjectCodeRegistry`
3. `soll.Registry` stores sequence counters keyed by `project_code` only
4. legacy `project_slug` columns are rebuilt away, not preserved as long-term compatibility aliases
5. tests enforce that canonical runtime schemas expose `project_code` and not `project_slug`
