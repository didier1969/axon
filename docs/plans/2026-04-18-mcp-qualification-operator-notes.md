# MCP Qualification Operator Notes

## Canonical Entry Point

Use:

```bash
./scripts/axon qualify-mcp [options]
```

This is now the primary operator-facing qualification command for MCP surfaces.

## Surfaces

- `--surface core`
  Qualifies the core/public MCP surface.
- `--surface soll`
  Qualifies the SOLL-oriented MCP surface.
- `--surface all`
  Runs both and reports per-surface subresults where relevant.

## Checks

- `quality`
  Functional validation of the selected surface.
- `latency`
  Latency and measurement-oriented checks.
- `robustness`
  Runtime responsiveness and recovery under load.
- `guidance`
  Guidance golden replay on supported surfaces.

## Mutation Modes

- `--mutations off`
  Read-only qualification.
- `--mutations dry-run`
  Non-committing preview/simulation checks only.
- `--mutations safe-live`
  Explicit bounded live mutation checks.
- `--mutations full`
  Explicit real mutation qualification.

`safe-live` and `full` remain opt-in and require an explicit scenario in phase 1.

## Recommended Commands

Core steady-state:

```bash
./scripts/axon qualify-mcp --surface core --checks quality,latency --mode steady-state --project AXO
```

Core deep run:

```bash
./scripts/axon qualify-mcp --surface core --checks quality,latency,robustness,guidance --mode both --project AXO
```

SOLL read-only:

```bash
./scripts/axon qualify-mcp --surface soll --checks quality --mutations off --project AXO
```

SOLL dry-run:

```bash
./scripts/axon qualify-mcp --surface soll --checks quality --mutations dry-run --project AXO
```

## Legacy Compatibility

These commands still exist, but they are no longer the preferred operator interface:

- `./scripts/axon quality-mcp`
- `./scripts/axon validate-mcp`
- `./scripts/axon measure-mcp`
- `./scripts/axon compare-mcp`
- `./scripts/axon robustness-mcp`
- `./scripts/axon qualify-guidance`

Use them when you need their expert behavior directly. For normal operator use, prefer `qualify-mcp`.
