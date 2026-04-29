# Brain/Indexer Cross-Dependency Audit

Date: 2026-04-22

## Goal

Classify active links between `brain` and `indexer` after the split so the runtime can be simplified deliberately instead of by symptom chasing.

## Target Contract

- `brain`
  - public MCP surface
  - dashboard
  - `SOLL` writer
  - `IST` reader replica only
- `indexer`
  - filesystem discovery
  - filters and scan policy
  - graph/vector pipeline
  - `IST` writer
  - no `SOLL` dependency for control or throughput-critical runtime decisions

## Test Contract

- `brain` qualification must be valid in isolation for:
  - MCP
  - dashboard
  - degraded reader-replica behavior
- `indexer` qualification must be valid in isolation for:
  - project discovery
  - scan
  - ingress/admission
  - graph/vector throughput
- split integration should only be used to validate:
  - authority truth
  - heartbeat/runtime truth
  - reader-replica visibility
  - operational convergence

## Categories

### Legitimate

- `brain -> IST reader replica`
  - explicit degraded mode when replica is absent or stale
- `brain <- indexer heartbeat/runtime truth`
  - used for split status and peer telemetry
- shared filesystem roots and local canonical metadata
  - `.axon/meta.json`
  - project paths
  - ignore/include filters

### Tolerable but should stay bounded

- split status reconstruction from heartbeat/runtime env files
- local scripts that read split state files directly instead of going through MCP
- Elixir/Phoenix dashboard as a read-only projection shell
  - acceptable only while treated as transitional
  - must not regain runtime authority, control, or mutation responsibilities

### Active legacy to remove

- `indexer -> SOLL.ProjectCodeRegistry`
  - project discovery/orchestration legacy path
  - partially removed in this tranche by switching to local canonical identity resolution
- status/qualification paths that overreach into rich MCP analytics by default
  - not required to validate cold ingest ramps

### Active and likely harmful

- any `indexer` runtime path that attaches `soll.db` during normal ingestion
- any operator/status path that causes `indexer` to execute `SOLL`-touching code even though `brain` owns `SOLL`

## Findings So Far

1. Project discovery was still falling back to registered identities in multiple places.
   - fixed toward local-first resolution in:
     - `project_meta.rs`
     - `scanner.rs`
     - `worker.rs`
     - `main_background.rs`

2. `status` still queried `soll.McpJob` unconditionally.
   - this is not acceptable for `indexer`
   - fixed so split `indexer` status omits `McpJob` counts entirely

3. `qualify_ingestion_run.py` still called rich MCP diagnostics by default.
   - these are not part of the core cold-run contract
   - changed to opt-in via `--include-rich-mcp-diagnostics`

4. The remaining `indexer -> SOLL` dependency was structural in `GraphStore`, not just in tooling.
   - `GraphStore::new_indexer_ist_writer_soll_reader(...)` still attached `soll.db` on:
     - writer session setup
     - reader session setup
     - reader refresh
   - this was the true cause of the split runtime lock conflict

5. Split `indexer` now boots through a no-`SOLL` store mode.
   - `RuntimeBootRole::IndexerShadow` now uses a `GraphStore` constructor that leaves `SOLL` detached
   - targeted tests pass:
     - `test_indexer_store_can_boot_while_brain_holds_soll_writer`
     - `test_reader_replica_publish_reuses_path_when_duckdb_temp_dir_exists`
     - `test_status_indexer_split_omits_soll_mcp_job_counts`
   - runtime validation on `dev` shows:
     - `indexer` = `HEALTHY`
     - `truth_status=canonical`
     - no `soll.db` attach error in `axon-dev-indexer` pane during startup or steady state
   - `brain` may still lag temporarily on `IST` reader freshness after reset, but that is now a replica-catchup issue, not a `SOLL` coupling issue

## Next Removal Pass

1. Keep `indexer` runtime strictly `SOLL`-free while continuing cold-run validation.
2. Investigate `brain` replica catch-up degradation after reset separately from `SOLL` coupling.
3. Re-run:
   - `reset-dev-baseline.sh`
   - `qualify-dev-cold.sh`
   - tmux pane inspection for absence of `soll.db` lock attempts
4. Only if a new `SOLL` touch reappears, classify it as:
   - tooling/operator path
   - latent legacy runtime path
   - test-only path

## Elixir / Dashboard Note

The dashboard stack now sits outside the core `brain <-> indexer` runtime split:

- it is no longer a control plane
- it is no longer a queueing/orchestration authority
- it is no longer a mutation surface

Its current role is:
- web operator shell
- bridge consumer
- read-only SQL/runtime projection

This means:
- it is not the next runtime authority problem to solve
- but it is a real maintenance and dependency cost
- and it should be treated as a bounded transitional layer unless a stronger future product reason emerges

## Exit Criteria

- no normal `indexer` runtime path touches `soll.db`
- cold qualification does not provoke `SOLL` lock conflicts
- remaining cross-links are only:
  - heartbeat/runtime truth
  - reader replica freshness
  - local scripts reading split state
