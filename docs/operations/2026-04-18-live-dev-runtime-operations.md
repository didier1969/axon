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
