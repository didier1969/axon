# SOLL Navigable Docs Implementation Plan

Date: 2026-04-19
Status: draft after concept convergence

## Objective

Add a human-readable, navigable documentation surface derived from SOLL without replacing the canonical full export or introducing a second source of truth.

## Phase 0: Guardrails

1. Freeze scope
- keep live SOLL and `SOLL_EXPORT_*.md` as canonical/archival artifacts
- define derived docs as read-only projection

2. Stop broad export pollution
- review where `docs/vision/` is auto-staged in commit flows
- isolate archival snapshot generation from ordinary developer commits

## Phase 1: Generator Contract

1. Define generator inputs
- project code
- selected root scope
- node rows
- edge rows
- generator version

2. Define output tree
- `docs/vision/site/<project_code>/index.html`
- `docs/vision/site/<project_code>/subtrees/<ROOT_ID>.html`
- `docs/vision/site/<project_code>/nodes/<NODE_ID>.html`
- manifest file with per-page hashes and dependencies

3. Define dependency invalidation
- changed node
- direct neighbors
- owning subtree
- ancestor indexes
- force full rebuild of any global overview page

## Phase 2: First Generator

1. Implement a deterministic generator
- stable ordering
- stable labels
- stable HTML structure
- stable Mermaid emission

2. Generate three page classes
- project overview
- subtree pages
- node detail pages

3. Embed human-readable summaries
- titles and types
- metadata summary
- related links
- bounded graph neighborhood

## Phase 3: Incremental Build

1. Write manifest
- page path
- node/edge dependency signature
- content hash
- generated timestamp
- source project code

2. Rebuild only changed pages when safe
- otherwise escalate to subtree or full project regeneration

3. Verify determinism
- no file change on identical source graph

## Phase 4: Publication

1. Add operator entrypoint
- generate site for one project
- optionally generate all

2. Add optional preview path
- static local serve only
- no new always-on service in this wave

## Phase 5: Validation

1. Structural validation
- every generated page resolves
- every link target exists
- every Mermaid block renders syntactically

2. Incremental validation
- single-node change only touches expected pages
- cross-subtree edge change invalidates expected ancestors

3. Human validation
- one real project walkthrough on `AXO`
- one greenfield walkthrough on `NTO`

## Risks

- hidden duplication of semantics between SOLL and generated docs
- invalid partial regeneration when graph neighborhood effects are underestimated
- accidental recommit of archival snapshots in ordinary workflows
- Mermaid readability limits on large pages

## Rollout Recommendation

1. ship generator behind a dedicated command
2. test on `AXO`
3. adopt as preferred human-reading surface
4. only later decide whether archival full exports should be reduced or retained at current frequency

## Expected Outcome

After this plan:

- humans browse SOLL as a navigable project doc
- Axon keeps one canonical intentional truth
- Git stores only changed derived pages where possible
- release workflows stop being polluted by repeated snapshot churn
