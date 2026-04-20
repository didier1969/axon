# SOLL Derived Docs Auto-Sync Concept

Date: 2026-04-19
Status: draft for CDD review

## Goal

Extend the newly introduced `soll_generate_docs` capability into an always-aligned human-readable documentation surface with two levels:

1. one global root that lists all known projects
2. one isolated derived site per project containing only that project's own structure

The update model must be:

- full generation when the derived docs do not exist yet
- incremental update when they already exist
- canonical truth remains live SOLL

## Problem

The current derived-doc generator is explicit and manual:

- it can generate a static HTML+Mermaid site for one project
- it writes only changed files
- it keeps a local manifest

But it is not yet maintained automatically after SOLL mutations, and it has no global index across projects.

## Core Requirements

1. Derived docs must auto-refresh after successful SOLL mutations.
2. The first generation for a project may be full.
3. Later generations must be incremental and file-stable.
4. The root site must expose all known projects.
5. Each project site must remain scoped to that project's own graph.
6. Derived docs remain non-canonical and non-restorable.

## Canonical Boundary

Canonical:

- live SOLL graph
- `soll_export`

Derived only:

- global project index
- project overview pages
- subtree pages
- node detail pages

No human-facing page becomes an import source.

## Proposed Structure

Root:

- `docs/derived/soll/index.html`
  - all known projects
  - links to each per-project site
  - summary counts

Per project:

- `docs/derived/soll/<PROJECT_CODE>/index.html`
- `docs/derived/soll/<PROJECT_CODE>/subtrees/<ROOT_ID>.html`
- `docs/derived/soll/<PROJECT_CODE>/nodes/<NODE_ID>.html`
- `docs/derived/soll/<PROJECT_CODE>/_manifest.json`

## Project Discovery

Use the same canonical discovery path already used elsewhere in Axon:

- `.axon/meta.json`
- `soll.ProjectCodeRegistry`

The global root should never infer project identity from generated docs.

## Update Model

### Full build

Run a full build when:

- the project output root does not exist
- the manifest is missing
- the generator version changes
- dependency data is incomplete or inconsistent

### Incremental build

Run an incremental build when:

- project output exists
- manifest exists
- changed SOLL entity set is known

Minimum invalidation set:

- changed node pages
- direct neighbors
- affected subtree roots
- project overview page
- global root index only when project metadata or project-level counts changed

## Trigger Model

Auto-sync should trigger only after successful mutating SOLL operations.

Candidates:

- `soll_manager`
- `soll_apply_plan`
- `soll_commit_revision`
- `soll_rollback_revision`
- `soll_attach_evidence`
- `axon_init_project`
- `axon_apply_guidelines` if it changes SOLL

Non-triggers:

- read-only tools
- `soll_validate`
- `soll_query_context`
- `soll_relation_schema`

There must be one canonical refresh boundary per execution class:

- one shared sync post-mutation hook
- one shared async completion hook

Do not duplicate refresh logic ad hoc across individual tools.

## Execution Strategy

Do not block every mutating response on a full rebuild.

Recommended approach:

- synchronous mutation result first
- then a best-effort derived-doc refresh in the same process for small updates
- or a coalesced background refresh if the mutation is heavy or batched

But the first implementation should bias toward simplicity:

- project-local regeneration in-process after successful sync mutations
- project-local regeneration at async job completion for async mutations

If refresh fails:

- the mutation result remains authoritative and successful
- the response must expose machine-readable refresh metadata
- stale derived docs must be visible, not silent

## Global Root Policy

The root page should contain:

- all known projects
- project name
- project code
- path or origin metadata if useful
- node counts by major SOLL type
- last derived-doc generation timestamp
- generation mode: `full` or `incremental`
- link to project site

The root page should never inline the full graph of all projects together by default.
It must visually state that it is derived and non-canonical.

## Why This Matches The User Request

This gives:

- one root node for all projects
- one per-project documentation tree containing only that project's structure
- incremental behavior by default
- full generation only when needed
- a human-readable surface with minimal tooling

## Risks

1. Over-eager triggers could make SOLL mutations slower.
2. Under-specified invalidation could miss pages and create stale docs.
3. A global root page can look canonical if its derived status is not explicit.
4. Async mutation completion hooks must not drift from sync mutation hooks.
5. Obsolete derived pages could survive unless manifest-driven cleanup is explicit.

## Recommended Minimal Solution

Wave 1:

- add global root generation
- add full-or-incremental decision per project
- add automatic trigger after successful SOLL mutations
- update root only when project registry or project summary changes
- add manifest-driven deletion of obsolete derived pages
- add visible freshness and derived-status markers on root and project pages

Wave 2:

- add coalescing/debouncing for bursty mutation flows
- add richer freshness/status reporting

## Initial Verdict

Proposed for CDD review.

## CDD Review Outcome

Reviewer A, architecture/update/versioning:

- approved the direction as a natural extension of the current generator
- requested one shared hook boundary, manifest-driven deletion, machine-readable refresh metadata, and visible derived status
- verdict: `approved_with_reservations`

Reviewer B, UX/navigation/publication:

- approved the global-root plus project-scoped-site model
- requested visible freshness badges, stable project ordering, and explicit stale-doc signaling on refresh failure
- verdict: `approved_with_reservations`

## Final Concept Verdict

`approved_with_reservations`

Reservations are integrated into the implementation plan.
