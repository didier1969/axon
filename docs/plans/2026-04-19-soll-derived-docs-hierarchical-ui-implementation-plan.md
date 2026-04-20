# SOLL Derived Docs Hierarchical UI Implementation Plan

Date: 2026-04-19
Status: plan
Method: idea-to-delivery

## Scope

Implement the next generation of derived SOLL docs as a static hierarchical UI with:

- `GLO` portfolio root
- project root pages
- focused node pages
- left tree navigation
- center hierarchy graph
- right details pane
- collapsible and resizable side panes

## Out of Scope

- server-hosted UI
- full-text search
- live editing
- collaborative state
- non-static client storage beyond lightweight local persistence

## Phase 1: Normalize Page Model

### 1. Replace page semantics

Refactor the derived docs generator so that:

- project `index.html` is a real project focus page
- node pages are hierarchy-focus pages
- subtree pages become secondary/compatibility pages if retained

### 2. Introduce a global root model

Generate:

- `docs/derived/soll/index.html`

This page must render:

- conceptual `GLO` root
- known projects as children
- stable ordering of projects

### 3. Define hierarchy-preferred relation selection

Create a helper that determines:

- preferred parent(s)
- preferred child(ren)
- lateral/supporting relations

This helper becomes the shared source for:

- center graph
- left tree
- right-pane hierarchy summaries

Constraints:

- it must remain a derived projection of canonical existing relations
- it must not become a second implicit ontology
- if multiple parents exist, the helper must return:
  - all visible canonical parents
  - one deterministic preferred breadcrumb parent by stable ordering

## Phase 2: Build the Three-Pane Shell

### 4. Replace current page template

Upgrade `render_site_page(...)` into a real app shell layout:

- left pane for tree
- center pane for graph
- right pane for details
- top toolbar for pane toggles and context

### 5. Add pane controls

Implement client-side JS for:

- collapse left pane
- collapse right pane
- resize left pane
- resize right pane
- persist pane widths and collapsed state in browser storage

Constraints:

- pane toggles must be keyboard-operable
- if persistence fails under `file://`, default widths/states must still render cleanly

Validation:

- all pages must still work over `file://`

## Phase 3: Build Tree Navigation

### 6. Generate project tree payload

For each project, generate a canonical nested tree structure derived from:

- project root
- visions
- pillars
- requirements
- lower-level descendants

### 7. Render left tree HTML

The left tree must:

- support open/close per branch
- include direct links to focus pages
- highlight current page
- allow full collapse of the pane itself
- default to conservative expansion, not fully expanded on large projects

### 8. Render global tree

The global root page must render:

- `GLO`
- project entries beneath it

## Phase 4: Reframe Center Graphs

### 9. Enforce top-down left-to-right focus graphs

Use `flowchart LR`.

Each focus page graph must show:

- parent context on the left
- current node as focus anchor
- direct children on the right

If there are multiple parents, represent them explicitly but keep the current node central in the local slice.

HTML parent/child links outside Mermaid remain the canonical navigation path.

### 10. Keep HTML fallback links

The center pane must be complemented by:

- parent links
- child links

outside Mermaid, so inter-page navigation is reliable even if Mermaid click binding fails.

## Phase 5: Reframe Right Pane

### 11. Move all structured detail here

The right pane must contain:

- title
- type
- status
- description
- metadata
- canonical hierarchy links
- lateral relations
- supporting entities

### 12. Separate hierarchy from lateral relations

Do not overload the center graph with every relation.

Rules:

- hierarchy edges go center
- lateral/supporting edges go right pane

## Phase 6: Incremental Regeneration

### 13. Expand invalidation rules

When a node changes, regenerate:

- its own focus page
- parent focus pages up the hierarchy chain
- project root page
- tree payload/pages affected by ancestry

Regenerate global root only if:

- project roster changes
- project summary inputs change

Delivery truth for this wave:

- if true page-level invalidation is not implemented cleanly, fall back to full per-project regeneration with differential writes
- do not claim finer-grained incremental invalidation than the runtime really performs

### 14. Keep full-build fallback

If manifests are missing or incompatible:

- perform full rebuild

## Phase 7: Tests

### 15. Add/replace tests for UI contract

Required tests:

- project root page renders project focus with child visions
- node page renders parent-left / child-right navigation contract
- left tree contains expected nested links
- page shell contains left/center/right panes
- pane controls exist
- pane resize/collapse JS is emitted
- pane controls are keyboard-addressable
- global root page uses `GLO` wording, not `ALL`
- incremental build still avoids rewriting unchanged pages

### 16. Preserve existing guarantees

Still verify:

- manifest generation
- stale page deletion
- root generation
- project-only null root fields

## Phase 8: Validation

### 17. Runtime validation on `dev`

Generate docs for `AXO` and manually inspect:

- root page
- project root page
- several node focus pages

### 18. Human validation checklist

Must succeed:

- click parent -> opens parent focus page
- click child -> opens child focus page
- collapse left pane
- collapse right pane
- drag left pane width
- drag right pane width
- center pane expands to full width when side panes are closed

## Risks and Mitigations

### 1. Existing tests assume older page semantics

Mitigation:
- update tests in one wave
- keep assertions focused on contract, not exact markup trivia

### 2. Mixed relation semantics can confuse the hierarchy

Mitigation:
- keep one explicit hierarchy-preferred relation selector

### 3. Incremental invalidation may miss ancestor pages

Mitigation:
- compute ancestor closure for changed nodes
- include project root in every node-affecting rebuild set

## Delivery Criteria

This wave is complete only when:

- the generated docs use the three-pane shell
- global root is `GLO`
- project root pages are top-down hierarchy roots
- node pages support parent/child inter-page navigation in both directions
- left and right panes are collapsible and resizable
- center pane expands correctly
- tests pass
- docs are regenerated successfully for `AXO` on `dev`
