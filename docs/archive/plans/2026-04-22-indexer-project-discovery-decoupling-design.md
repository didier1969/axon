# Indexer Project Discovery Decoupling Design

## Goal

Remove `indexer` dependence on live `SOLL` for project discovery and scan orchestration.

## Current Problem

- `indexer` currently reads `soll.ProjectCodeRegistry` to discover `(project_code, project_path)`.
- In split mode, `brain` is the `SOLL` writer.
- DuckDB live-file concurrency prevents `indexer` from attaching `soll.db` read-only while `brain` holds the writer lock.
- Result: no discovered projects, no initial scan, cold qualification stays at zero.

## Approved Direction

- `brain` keeps `SOLL`.
- `indexer` stops using live `SOLL` for project discovery.
- `indexer` derives project identities locally from canonical filesystem truth:
  - `.axon/meta.json`
  - local project paths
  - local scanner filters / ignore rules

## Design

### Discovery source

Use `project_meta::discover_project_identities()` as the canonical source for orchestration in `indexer`.

This already gives:
- canonical `project_code`
- canonical `project_path`
- local `.axon/meta.json` provenance

### Orchestrator behavior

- Replace the `soll.ProjectCodeRegistry` polling path inside `spawn_federation_orchestrator`.
- Build a local discovery set from `discover_project_identities()`.
- Ignore `PRO`.
- Orchestrate only identities with a non-empty path.
- Keep the existing `known_projects` de-duplication and watcher/scan fan-out.

### Scope

This tranche changes only project discovery/orchestration.

It does **not**:
- change `brain` ownership of `SOLL`
- change `SOLL` semantics
- add a `SOLL` reader replica

## Validation

1. Unit tests for local discovery selection.
2. `cargo test` targeted on the new helper and existing federation tests.
3. Cold dev run:
   - `reset-dev-baseline.sh`
   - `qualify-dev-cold.sh`
4. Runtime proof:
   - no `indexer` `SOLL` lock conflict during project discovery
   - non-zero cold pipeline movement when eligible files exist
