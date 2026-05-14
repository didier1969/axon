# MCP Graph-Only + Qualification Handoff

## Scope

This note freezes the runtime/qualification work delivered on branch `feat/mcp-robustness-isolation`.

Delivered capabilities:
- new runtime mode `graph_only`
- explicit MCP tool + CLI path to resume deferred vectorization
- unified qualification entrypoint `./scripts/axon qualify`
- comparative robustness qualification across runtime modes

## Runtime Model

`graph_only` means:
- MCP stays enabled
- watcher / scan / graph ingestion stay enabled
- semantic workers stay disabled
- background vectorization queue is not backfilled at boot
- graph ingestion does not enqueue new `FileVectorizationQueue` work

Operational consequence:
- `graph_ready` can progress while `vector_ready` remains false
- vectorization is resumed later via either:
  - restart in `full`
  - `./scripts/axon resume-vectorization`

## Qualification Surface

Public entrypoint:
- `./scripts/axon qualify`

Profiles:
- `smoke`: runtime restart + exhaustive read-only MCP validation
- `demo`: `smoke` + short MCP robustness qualification
- `full`: `demo` + ingestion qualification
- `ingestion`: compatibility path for ingestion-focused qualification

Recommended commands:
- `./scripts/axon qualify --profile demo --mode graph_only`
- `./scripts/axon qualify --profile smoke --compare mcp_only,graph_only,full`
- `./scripts/axon qualify --profile full --mode full --duration 60 --interval 5`

Artifacts:
- consolidated suite runs live under `.axon/qualification-suite-runs/...`
- robustness sub-runs remain nested under the suite run
- ingestion qualification remains nested under the suite run

## Validation Snapshot

Validated during this wave:
- `smoke` on `graph_only`: `pass`
- `demo` on `graph_only`: `pass`
- `smoke --compare mcp_only,graph_only,full`: `pass`
- `full` on `full`: overall `warn`

Reason for the `full` warning:
- no MCP transport failures
- no backend unavailability
- no timeouts
- `responsive=1.0`, `success=1.0`
- warning came from latency only: `p95=8554ms` during robustness qualification in `full`

Interpretation:
- the system is functionally stable
- the remaining issue is performance/isolation under `full`, not correctness of the MCP path

## Merge / Follow-up Guidance

Branch is ready for reintegration once the working tree is committed cleanly.

Recommended next architectural step:
- separate MCP/graph serving from vectorization/indexation heavy work
- keep MCP/graph serving high-priority
- keep vectorization low-priority and asynchronous

Immediate runtime default after this wave:
- prefer `graph_only` when the goal is MCP availability with graph freshness
- use `full` when vectorization catch-up is intentionally required
