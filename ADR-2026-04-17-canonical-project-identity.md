# ADR-2026-04-17: Canonical Project Identity And Admission

## Status

Accepted on 2026-04-17.

## Context

Axon currently carries two partially overlapping ways to bind a file to a project:

- filesystem discovery through `.axon/meta.json`
- runtime memory through `soll.ProjectCodeRegistry`

The intended model is stricter:

- there is one canonical file truth
- there is one canonical memory truth
- memory is a copy of file truth
- no project may be ingested unless it is explicitly registered
- `PRO` is reserved for SOLL global guidelines and never for imported project files

In practice, the runtime still contains implicit fallbacks such as `GLOBAL` and `global`.
These fallbacks allow files to be imported even when no canonical project identity has been resolved.
This violates the admission contract and contaminates MCP quality because code and documents from one project can be indexed outside their canonical scope.

## Decision

Axon adopts a single canonical project identity contract:

- canonical file truth: `.axon/meta.json`
- canonical runtime truth: `soll.ProjectCodeRegistry`
- runtime truth must be a faithful mirror of file truth
- ingestion, scanning, watching, and worker normalization must resolve identity through the canonical runtime registry
- if a path cannot be resolved to a registered project, ingestion must reject it
- no implicit fallback to `GLOBAL`, `global`, `PRO`, or any invented code is allowed

Operationally:

- per-project scanners and watchers may carry an explicit trusted `project_code` only when they are spawned from a canonical registry entry
- workspace-wide scans must resolve each file by path against `soll.ProjectCodeRegistry`
- workers must reject unresolved files instead of rewriting them into a fake global scope
- `PRO` remains reserved to the SOLL global namespace and guidelines

## Consequences

Positive:

- one project identity contract across scanner, watcher, worker, and MCP
- no silent scope drift between `project_path`, `project_name`, and `project_code`
- better MCP retrieval quality because project-scoped queries see the right files

Costs:

- unregistered paths are no longer ingested
- stale data already indexed under non-canonical scopes must be reindexed separately
- tests and fixtures that depended on implicit global fallbacks must be updated

## Implementation Notes

The minimal correction is:

1. introduce registry-backed path resolution in `project_meta`
2. make scanner and watcher use registry-backed resolution for workspace-wide scans
3. remove worker fallback to `global`
4. reject unresolved files defensively in the worker
5. keep existing scan filters unchanged unless they conflict with the admission rule
