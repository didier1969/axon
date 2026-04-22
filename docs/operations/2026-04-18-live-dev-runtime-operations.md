# Live/Dev Runtime Operations

## Canonical Entrypoints

Prefer explicit wrappers:

```bash
./scripts/axon-live <command> ...
./scripts/axon-dev <command> ...
```

Equivalent unified form:

```bash
./scripts/axon --instance live <command> ...
./scripts/axon --instance dev <command> ...
```

## Runtime Roles

- `live`
  - stable truth runtime
  - bound to the current live databases
  - target for explicit release promotion
- `dev`
  - isolated development runtime
  - separate ports, sockets, pidfiles, run root, and graph root
  - safe place for migrations, resettable state, and qualification

## Default Resource Policy

The control plane is intentionally asymmetric:

- `live`
  - `resource_priority=critical`
  - `background_budget_class=balanced`
  - `gpu_access_policy=preferred`
  - `watcher_policy=full`
- `dev`
  - `resource_priority=best_effort`
  - `background_budget_class=conservative`
  - `gpu_access_policy=avoid`
  - `watcher_policy=bounded`

These defaults project onto existing runtime knobs such as:

- `MAX_AXON_WORKERS`
- `AXON_QUEUE_MEMORY_BUDGET_BYTES`
- `AXON_WATCHER_SUBTREE_HINT_BUDGET`
- and, for `gpu_access_policy=avoid`, `AXON_EMBEDDING_PROVIDER=cpu` unless explicitly overridden

The intent is simple:

- `live` keeps responsiveness
- `dev` yields first under pressure

## Lifecycle Commands

Start:

```bash
./scripts/start-live.sh --full
./scripts/start-dev.sh --full
```

Status:

```bash
./scripts/status-live.sh
./scripts/status-dev.sh
```

Stop:

```bash
./scripts/stop-live.sh
./scripts/stop-dev.sh
```

## Qualification

Unified runtime qualification:

```bash
./scripts/axon qualify --profile smoke --mode graph_only
./scripts/axon --instance live qualify --profile smoke --mode graph_only
```

Rules:

- `./scripts/axon qualify ...` now defaults to `dev`
- use `--instance live` only when you intentionally want to qualify the promoted live runtime
- the run summary now also exposes `runtime_quiescent`
- `runtime_quiescent=blocked` or `watch` degrades `runtime_smoke` to `warn`
- this is intentional: a runtime may be reachable while still not being ready for quiescent qualification

Artifacts written by `qualify` now include:

- `runtime-status.json`
- `runtime-quiescent-summary.json`

Core:

```bash
./scripts/axon-live qualify-mcp --surface core --checks quality,latency --project AXO
./scripts/axon-dev qualify-mcp --surface core --checks quality,latency --project AXO
```

SOLL:

```bash
./scripts/axon-live qualify-mcp --surface soll --checks quality --mutations off --project AXO
./scripts/axon-dev qualify-mcp --surface soll --checks quality --mutations dry-run --project AXO
```

## Seed Dev From Live

Canonical command:

```bash
./scripts/axon seed-dev-from-live
```

Default safety rules:

- `dev` must be stopped
- `live` must be stopped
- current dev graph root is backed up under `.axon-dev/backups/<timestamp>/`

Advanced mode:

```bash
./scripts/axon seed-dev-from-live --allow-live-running
```

This copies DB files plus WAL and is best-effort only.

## Identity Check

Always verify the target instance through MCP `status` before risky work.

Expected properties:

- `instance_identity.instance_kind`
- `instance_identity.data_root`
- `instance_identity.run_root`
- `instance_identity.mcp_url`
- `resource_policy.resource_priority`
- `resource_policy.background_budget_class`
- `resource_policy.gpu_access_policy`
- `resource_policy.watcher_policy`
- `resource_policy.embedding_provider`
- `runtime_version.release_version`
- `runtime_version.build_id`

## Canonical Ingestion Stage Truth

`status` now exposes `runtime_authority.canonical_ingestion_stage_model` as the canonical
runtime truth for watcher/file/graph/vector ownership boundaries.

`status` also exposes `runtime_authority.loop_semantics` as the canonical execution model:

- upstream loop
  - `mode=push`
  - `buffered_discovery -> persisted_file_pending -> graph_ready`
  - this is a high-level loop summary aggregating:
    - `supply/discovery`
    - `admission/production`
  - `persisted_file_pending` remains the critical throughput stock for the full system unless runtime evidence disproves it
- downstream loop
  - `mode=pull`
  - `graph_ready -> vector_ready`
  - paced by real GPU/VRAM availability
  - may idle cleanly when `graph_ready=0`
- finalize
  - `mode=async`
  - not part of the GPU hot path unless a hard safety invariant requires it

Read it as:

- `freshness`
  - `status(mode="brief")` is cached
  - use `status(mode="full")` when exact current counts matter
- `ingress_buffered`
  - all buffered ingress still living in memory before canonical `File` persistence
- `watcher_buffered`
  - watcher-originated ingress still buffered in memory
- `scan_buffered`
  - scan-originated ingress still buffered in memory
- `persisted_file`
  - files durably present in the canonical `File` table
- `persisted_file_pending`
  - durably persisted canonical file work still eligible and pending graph production
- `graph_wip`
  - canonical file work currently owned by the graph worker pool
- `structural_graph_backlog`
  - canonical file graphing backlog before `graph_ready`
- `graph_projection_queue_owned`
  - secondary graph projection or graph embedding work
  - diagnostic only, not the canonical file graphing backlog
- `graph_ready`
  - files marked graph-ready in `File`
- `file_vectorization_queue_owned`
  - files currently owned by `FileVectorizationQueue`
- `vector_ready`
  - files already marked vector-ready in `File`
- `explicitly_excluded_from_vectorization`
  - explicit deleted/skipped/oversized exclusions counted from `File`

This section is descriptive only. It does not invent additional lifecycle states beyond the
current ingress buffer, `File`, `GraphProjectionQueue`, and `FileVectorizationQueue` surfaces.

## Production Release Flow

1. Qualify candidate build.
2. Create manifest:

```bash
./scripts/axon create-release-manifest --state qualified
```

3. Promote explicit manifest:

```bash
./scripts/axon promote-live --manifest <manifest.json> --restart-live
```

4. If needed, rollback using a previously promoted manifest:

```bash
./scripts/axon rollback-live --manifest .axon/live-release/history/<generation>.json --restart-live
```

Promotion and rollback are valid only when MCP `status` proves the running live version.

## Split Rollback Procedure

When the runtime is in split shadow mode, treat rollback as an explicit return to the
legacy monolith. Do not promote the split shadow runtime until the rollback path is green.

Operator sequence:

1. Stop the split runtime cleanly:

```bash
./scripts/stop.sh
```

2. Confirm the stop released both writer guards:

```bash
./scripts/stop.sh --verify
```

This verification is strict: it must prove both lockfiles still exist and are unlockable after shutdown.

3. Reactivate the monolith explicitly:

```bash
./scripts/start.sh --rollback-monolith
```

4. Prove canonical truth and authority:

```bash
./scripts/status.sh
python3 scripts/qualify_runtime.py --instance dev --profile smoke --mode full --reuse-runtime
```

Expected rollback proof:

- `status` reports `rollback_path=green`
- `status` reports `promotion_allowed=true`
- `status` reports `canonical_truth_restored=true`
- `status` reports `version_identity_verified=true`
- `qualify_runtime.py` reports `promotion_allowed=true`
- `qualify_runtime.py` reports `rollback_path=green`
- `qualify_runtime.py` reports `version_identity_verified=true`

If any of those are red, keep the split runtime non-promotable and fix the rollback path before any cutover discussion.
