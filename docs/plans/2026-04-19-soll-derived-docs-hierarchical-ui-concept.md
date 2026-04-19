# SOLL Derived Docs Hierarchical UI Concept

Date: 2026-04-19
Status: concept
Method: idea-to-delivery

## Goal

Replace the current derived SOLL HTML pages with a human-first navigation model that matches the intended macro-to-micro hierarchy:

- `GLO` portfolio root
- project root
- vision
- pillar
- requirement
- decision / validation / guideline / concept / milestone / stakeholder

The generated documentation must feel like a navigable project cockpit, not a set of disconnected pages.

## Problem

The current derived site is useful but incomplete for human navigation:

- overview pages are too shallow
- node pages are local, not hierarchy-first
- navigation is link-rich but not structurally obvious
- the current layout does not dedicate stable space to:
  - tree navigation
  - focused graph view
  - structured detail panel
- Mermaid graphs are present, but they are not yet the center of a consistent top-down browsing experience

The user expectation is stricter:

- parents must appear on the left
- children must appear on the right
- clicking a child opens its own page where it becomes the new focus
- clicking a parent must reopen the parent focus page
- the left side must expose a collapsible full tree
- the left and right panels must be collapsible and resizable down to zero width
- the center panel must expand to the full available browser width when side panels are closed

## Non-Goals

- do not turn derived docs into canonical truth
- do not require a web server for reading
- do not introduce live editing in this wave
- do not redesign SOLL semantics or relation policy
- do not make Mermaid the only navigation surface

## Architectural Decision

Keep the output as a static HTML/CSS/JS site.

No server is required for this wave because:

- the user needs local reading, not collaborative editing
- collapsible trees, resizable panels, stateful UI, and inter-page navigation work fine client-side
- static generation preserves the existing incremental refresh model

The server-side Axon responsibility remains:

- generate the static site from canonical live SOLL
- keep it incrementally synchronized after successful SOLL mutations

## Canonical Reading Model

Every page becomes a focused hierarchy page composed of three panes:

1. left pane
- full tree navigation
- expandable/collapsible
- clickable hierarchy links
- resizable
- fully closable

2. center pane
- focused Mermaid graph
- left-to-right orientation
- current focus node on the left side of the local graph
- direct children on the right side
- direct parent context still visible when relevant
- occupies the remaining width

3. right pane
- structured details
- metadata
- canonical relations
- quick links
- resizable
- fully closable

## Page Types

### 1. Global root page

Path:
- `docs/derived/soll/index.html`

Identity:
- conceptual root label = `GLO`

Role:
- list all known projects
- show the first hierarchy step from `GLO` to projects
- anchor the global left tree

### 2. Project root page

Path:
- `docs/derived/soll/<PROJECT_CODE>/index.html`

Role:
- show the project as focus node
- show attached visions as immediate children
- serve as the project-local root for the left tree

### 3. Hierarchy focus page

Path:
- `docs/derived/soll/<PROJECT_CODE>/nodes/<NODE_ID>.html`

Role:
- current node becomes the focus
- direct parent context remains visible
- direct children are the main rightward expansion
- clicking parent/child opens the corresponding focus page

### 4. Optional aggregate pages

Existing subtree pages may remain, but they are no longer the primary reading model.

The primary reading model becomes:
- root page
- project root page
- focus node page

## Navigation Contract

Every page must guarantee:

- breadcrumb back to `GLO`
- breadcrumb back to project root
- left-tree direct navigation
- parent links in the center graph and in HTML
- child links in the center graph and in HTML
- sidebar state persistence client-side when possible

The reading path must work even if Mermaid clicks fail.

HTML links and the left tree are the canonical interaction path.

Mermaid click bindings are enhancement only and must never be the only way to move between pages.

## Data Semantics

The hierarchy should privilege canonical structural parent/child relations.

Primary hierarchy:

- `GLO -> Project`
- `Project -> Vision`
- `Vision -> Pillar`
- `Pillar -> Requirement`
- `Requirement -> Decision`
- `Requirement -> Validation`

Supporting entities:

- `Guideline`
- `Concept`
- `Milestone`
- `Stakeholder`

These may appear:

- as right-side children where canonically attached
- or in the details pane if they are lateral/supporting

The key rule is:
- primary hierarchy first
- lateral relations second

There must be exactly one hierarchy-preferred relation selector shared by:

- left tree generation
- center focus graph generation
- right-pane hierarchy summaries

This selector is a derived projection of canonical existing relations, not a second ontology.

If a node has multiple canonical parents, the UI must apply one deterministic focus rule:

- the center pane shows all canonical parents on the left
- one parent may be marked as the preferred breadcrumb parent by stable sort order
- no parent may be hidden silently

## UX Constraints

- full-width responsive layout
- no artificial max-width cap on the central reading area
- split panes with drag handles
- pane widths persisted client-side
- pane collapse/expand controls
- keyboard-operable tree expand/collapse and pane toggles
- accessible fallback links outside Mermaid
- conservative default tree expansion; large projects must not open fully expanded by default
- if client-side persistence is unavailable under `file://`, the layout must still render and remain operable with default widths/states

## Incremental Generation Impact

The existing incremental generator stays valid in principle, but page invalidation must become stronger because:

- the left tree is shared across all pages in a project
- project root pages depend on project-level hierarchy
- global root depends on project registry and project summaries

So the invalidation model becomes:

- one changed node updates:
  - its own focus page
  - ancestors’ focus pages
  - affected project root page
  - left tree payload for that project
  - global root only if the project summary changed

For this wave, the promise must remain truthful:

- project output may still be regenerated as a full project render with differential writes
- true page-level ancestor-closure invalidation is desirable, but must not be claimed unless implemented explicitly

## Risks

### Risk 1: hierarchy ambiguity

Some node types have lateral relations, not strict parent/child hierarchy.

Mitigation:
- define a single hierarchy-preferred edge selection rule for center graphs
- keep non-hierarchical relations in the right pane

### Risk 2: oversized left tree payload

Large projects could make the left pane heavy.

Mitigation:
- generate a compact nested HTML tree
- defer virtualized tree or search to a later wave

### Risk 3: file:// behavior differences

Browser behavior for module loading or persisted state may vary.

Mitigation:
- keep JS inline and self-contained
- avoid server-only assumptions

## Decision

Proceed with a static, hierarchical, three-pane derived SOLL site where:

- `GLO` is the conceptual portfolio root
- `GLO` is a derived reading root only, not a new canonical SOLL entity
- project root pages become true hierarchy roots
- node pages become focused parent/child navigation pages
- left and right panes are resizable and collapsible
- Mermaid is centered in the experience but not the only navigation surface
