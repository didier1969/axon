# SOLL Navigable Docs Concept

Date: 2026-04-19
Status: concept converged
Method: CDD phase 1 with two independent expert reviews

## Problem

The current human-readable SOLL export is a monolithic Markdown snapshot written under `docs/vision/SOLL_EXPORT_*.md`.

This has three practical issues:

1. readability collapses as the graph grows
2. repeated timestamped exports pollute `docs/vision/` and can accidentally perturb Git `HEAD`
3. the export is useful as an archival snapshot, but not as a navigable project documentation surface for humans

## User Goal

Keep SOLL as the canonical source of intentional truth, but derive a human-readable documentation surface that:

- is easy to browse with minimal tooling
- exposes Mermaid graphs at a manageable scope
- lets readers navigate parent/child/subtree relationships
- avoids storing the same truth repeatedly
- versions only changed derived artifacts when possible
- can later be served locally with very low operational cost

## Non-Goals

- replacing live SOLL as the source of truth
- turning Mermaid pages into an import/restore source
- inventing a second documentation ontology independent of SOLL
- adding a heavy always-on server in the first wave
- implementing a Git replacement inside Axon

## Current Reality

Current `soll_export` in `src/axon-core/src/mcp/tools_soll.rs`:

- reads `soll.Node` and `soll.Edge`
- emits one Markdown file
- embeds one large Mermaid topology block
- appends a flat entity listing
- writes to canonical repo-root `docs/vision/`

Current `axon_commit_work` also stages `docs/vision/`, which means repeated exports can leak into normal commits and release work if not carefully controlled.

## Core Decision

Keep two distinct layers:

1. Canonical layer
- live SOLL graph
- existing archival full export `SOLL_EXPORT_*.md`

2. Derived human-doc layer
- deterministic, navigable, generated from SOLL
- optimized for reading, not for restore

This is a reuse-first decision, not a redesign of SOLL.

## Recommended Output Model

Preferred model:

- a generated static documentation tree, ideally HTML pages that embed Mermaid client-side
- stable links between overview pages, subtree pages, and node detail pages

Why HTML instead of Markdown-only:

- Mermaid link/click behavior is renderer-dependent in Markdown
- plain fenced Mermaid is not reliable enough for real navigation
- HTML allows stable anchors, controlled layout, and human-readable summaries around the graph

## Proposed Information Architecture

Per project, generate:

- `index.html`
  - project overview
  - top-level graph summary
  - entry links by vision, pillar, requirement, decision, validation
- `subtrees/<ROOT_ID>.html`
  - subtree graph centered on one vision or pillar
  - canonical outgoing/incoming context
  - links to related node detail pages
- `nodes/<NODE_ID>.html`
  - node detail
  - metadata
  - incoming/outgoing relations
  - nearby graph
  - links to parent/child/subtree pages
- optional `global.html`
  - only if useful
  - should be treated as a full-regenerated page, not a fine-grained incremental artifact

Every page should contain:

- title and node type labels
- canonical ID
- compact Mermaid graph
- short textual explanation
- clickable related entities
- relevant metadata/evidence summary when applicable

## Incremental Generation Model

Partial regeneration is realistic only with explicit dependency invalidation.

Safe invalidation scope:

- changed node page
- direct neighbor node pages
- owning subtree page
- ancestor index pages

Potentially unsafe as partial-only:

- any global overview page
- pages affected by cross-subtree or cross-project edges

So the generator should track page inputs explicitly:

- node IDs
- edge signatures
- generator version
- source snapshot/generation marker
- page content hash

## Versioning Decision

Do not version repeated full snapshots as the primary human-doc surface.

Instead:

- keep archival `SOLL_EXPORT_*.md` snapshots for restore/backup semantics
- version the derived navigable pages only when content actually changes
- maintain one machine manifest describing page inputs and hashes

This achieves the practical Git-like behavior the user wants without inventing a second version control system.

## Publication Decision

First publication mode should be static.

Recommended first shape:

- generated site under a dedicated subtree such as `docs/vision/site/<project_code>/`
- browsable directly as files or via a trivial local static server

Optional later addition:

- a lightweight local preview command or static file server

Do not start with a new always-on Axon runtime service for this.

## Relation To Current Export

The current full export should remain:

- canonical archival snapshot
- restore-compatible source
- operator fallback artifact

The new navigable docs should become:

- the default human-reading surface
- the project-friendly browsing surface
- the place where Mermaid is used intentionally and at controlled scope

## Minimal-Change Path

Wave 1:

- keep `soll_export` intact
- add a second generator for navigable derived docs
- generate per-project overview, subtree pages, and node detail pages
- add deterministic manifest + incremental invalidation
- stop auto-staging broad `docs/vision/` snapshots in normal delivery flows

Wave 2:

- add optional preview command / local static serve
- refine labels, summaries, and evidence presentation

Wave 3:

- only if justified, integrate tighter with a richer documentation UI

## Expert Review Summary

Reviewer A, architecture/export/versioning:

- technically sound as a derived layer
- warned against making Mermaid pages a second source of truth
- recommended content-addressed incremental generation and keeping full export as canonical restore artifact
- verdict: `approved_with_reservations`

Reviewer B, UX/navigation/publication:

- agreed the current monolithic Mermaid export is not human-scalable
- recommended HTML-based static docs rather than Markdown-only navigability
- warned that Mermaid hyperlink behavior is too renderer-dependent in plain Markdown
- verdict: `approved_with_reservations`

## Final Concept Verdict

`approved_with_reservations`

Reservations are non-blocking and now explicit:

- derived docs must remain non-canonical
- incremental regeneration must honor graph dependency invalidation
- static publication should come before any heavier server solution
