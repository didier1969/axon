# Axon Cockpit Redesign Design

Date: 2026-04-02
Status: approved-for-implementation
Scope: Phoenix/LiveView cockpit refactor only, without changing Rust runtime authority

## Purpose

Replace the current machine-centric cockpit with a truthful operator cockpit that answers:

- where Axon stands right now
- what is already usable for development and LLM workflows
- what is still blocked or partial
- which projects are ready, partial, or lagging

The redesign must remain fully aligned with the Rust-first boundary:

- Rust owns runtime truth
- Phoenix displays and organizes truth
- the cockpit stays strictly read-only

## Why The Current Cockpit Is Not Enough

The current cockpit in `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex` is still shaped like a technical console:

- machine-oriented sections (`UNIT 01`, `UNIT 02`, etc.)
- weak project/workspace visibility
- weak backlog explanation
- weak developer-value signaling
- residual non-native asset loading in the watcher layout (`CDN` for Phoenix/LiveView and Google Fonts)

This no longer matches the real maturity of Axon. The runtime now exposes richer truth:

- canonical backlog states
- `status_reason`
- ingress buffer telemetry
- runtime pressure and claim mode
- process memory split (`RSS`, `RssAnon`, `RssFile`, `RssShmem`)
- DB/WAL size
- project completion and partial-truth signals

The cockpit must catch up with that reality.

## Non-Negotiable Constraints

### 1. LiveView Native First

The redesign must use native Phoenix/LiveView patterns as the default:

- assigns for scalar state
- periodic refresh for factual aggregates
- streams for live collections
- local JS only when LiveView alone is insufficient

No React-like client architecture should be recreated inside Phoenix.

### 2. No CDN

The cockpit must not rely on CDN-loaded runtime assets.

Specifically:

- no CDN for Phoenix JS
- no CDN for Phoenix LiveView JS
- no CDN for fonts

Assets must come from the local Phoenix asset pipeline or local static files only.

### 3. Read-Only Boundary

The cockpit must not:

- trigger runtime actions
- own backlog logic
- own scheduling
- own retry policy
- invent runtime truth

It is an operator and developer visibility surface only.

### 4. Operator Value Over Machine Vanity

Machine telemetry is useful, but secondary.

The cockpit should first answer:

- what progress exists
- what remains
- what is blocked
- what is partial
- what a developer or LLM can trust now

Only then should it surface low-level memory/runtime signals.

## Recommended UI Model

The cockpit becomes a single strong LiveView page with five information bands.

### Band 1. Global Workspace Header

This is the top situation room summary.

It shows:

- workspace-wide readiness state
- MCP/SQL runtime observed state
- current truth mode:
  - structural
  - hybrid
  - partial

This band is not decorative. It is the operator's first decision frame.

### Band 2. Hero Metrics

This band shows the canonical global ingestion counters:

- files known
- indexed
- indexing
- pending
- indexed degraded
- oversized
- skipped
- completion rate

This must be visually immediate and legible.

It is the primary answer to “where are we?”

### Band 3. Backlog And Causality

This band explains why the backlog exists.

It shows:

- dominant `status_reason`
- whether the current backlog is mostly normal flow or mostly pathological churn
- separation between:
  - `pending`
  - `indexing`
  - `indexed_degraded`
  - `oversized`

This is where Axon stops being a vague progress bar and becomes an explainer.

### Band 4. Projects

This band makes workspace truth usable.

It shows:

- project completion
- completed vs total files
- partial truth / degraded truth indicators
- most active or most delayed projects

This is the primary surface for a developer who wants to know:

- whether a project is ready
- whether answers will be partial

### Band 5. Runtime, Ingress, And Memory

This band keeps technical truth visible without dominating the page.

It shows:

- `claim_mode`
- `service_pressure`
- queue depth
- memory budget / reserved / exhaustion
- ingress buffer state
- `RSS`, `RssAnon`, `RssFile`, `RssShmem`
- DB/WAL size

This is operationally valuable, but intentionally lower in hierarchy than progress and project truth.

## Information Hierarchy

The hierarchy must be:

1. progress truth
2. blockage truth
3. project readiness
4. developer/LLM trust level
5. runtime telemetry

The current cockpit does this in almost the reverse order. The redesign corrects that.

## Visual Direction

The visual direction should be deliberate and high-signal:

- dark industrial surface
- restrained glass/surface layering
- compact cards
- strong typography hierarchy
- minimal chrome
- color reserved for state meaning, not decoration

Color semantics:

- green: healthy / complete / available
- amber: in progress / partial / guarded
- red: blocked / pathological / refused
- blue: neutral structural information

Typography:

- local sans for labels and layout framing
- local mono for counts, paths, and technical state

The page should feel like a real command surface, not a themed debug panel.

## LiveView Architecture

### Data Model

The LiveView should compose state from:

- `Progress.get_status/1`
- `Progress.get_directory_stats/1`
- `Axon.Watcher.Telemetry.get_stats/0`
- SQL-derived backlog summaries where needed

The page should prefer a small number of explicit derived view models over raw map rendering.

### Refresh Model

Use a controlled periodic tick for factual refresh.

Recommended pattern:

- keep a periodic refresh cadence
- derive compact view-state maps
- stream only the project/backlog collections

No unbounded growth in assigns.

### Streams

Use LiveView streams for:

- project rows
- top backlog reasons
- optionally recent files if retained as a narrow operator window

Do not use streams for scalar telemetry.

### JS Usage

JS should be minimal and local.

Allowed uses:

- tiny local hooks for progressive enhancement
- local asset bootstrapping
- optional chart micro-helpers only if HEEx/CSS is not enough

Not allowed:

- client-owned state architecture
- external CDN charting
- LiveView being reduced to a data API for a JS SPA

## Asset Governance

The watcher layout must be cleaned up.

Required changes:

- remove CDN `<script>` tags for Phoenix and LiveView
- rely on the existing local `assets/js/app.js`
- remove Google Fonts CDN links
- use local/system font stacks or vendored local fonts if truly needed

This is both a correctness and sovereignty fix, not just polish.

## Developer Value Layer

The cockpit should explicitly show what an LLM/developer can rely on.

Examples:

- structural truth available
- semantic path available or paused
- partial truth present in one or more projects

This turns the cockpit into a real operator/developer readiness console instead of a generic infra panel.

## What This Redesign Does Not Include

Out of scope:

- runtime control actions
- command relays to Rust
- multi-page navigation redesign
- dashboard-authored scheduling
- new runtime authority in Elixir

This is a cockpit redesign, not a control plane return.

## Recommended Implementation Sequence

1. remove CDN dependency and normalize watcher layout
2. rebuild the cockpit view model around operator information bands
3. introduce local reusable HEEx render helpers/components inside the LiveView or nearby modules
4. add streams for project and backlog sections
5. align tests with the new information hierarchy

## Delivery Criteria

The cockpit redesign is successful when:

- the page remains fully LiveView-native
- no CDN is required
- operator value is centered on progress, projects, and blockage
- runtime telemetry is visible but secondary
- the page makes partial truth explicit
- the page is visually intentional and not generic
- `mix test` remains green
