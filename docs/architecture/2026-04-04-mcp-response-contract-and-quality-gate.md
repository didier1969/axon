# MCP Response Contract and Quality Gate (2026-04-04)

## Objectives

- Keep transport truth separate from semantic truth.
- Prevent false negatives in MCP validation.
- Make MCP responses machine-stable for LLM agents.

## Standard response contract

Core tools now emit a common contract block:

- `Status`: `ok` | `warn_*` | `error_*`
- `Summary`: one-line intent/result
- `Scope`: `project:<slug>` or `workspace:*`
- `Confidence`: `low` | `medium` | `high`
- `Evidence`: factual payload (tables/metrics/details)
- `Next actions`: short actionable follow-ups

Current coverage:

- `query`, `inspect`
- `impact`, `simulate_mutation`, `diff`
- `health`, `audit`
- `debug`

## Brief vs verbose mode

Supported on core tools via input arg:

- `mode=brief` (default): evidence clipped for bounded payloads
- `mode=verbose`: full evidence when available

`diff` also supports:

- `limit` (10..500, default 120) symbols per file
- hard output cap (`truncated=true` marker when clipped)

## Validator behavior

`scripts/mcp_validate.py` improvements:

- Auto-discovers a valid symbol probe from `query` output.
- Uses `--symbol` override when provided.
- Classifies symbol-not-found patterns as `warn` (not transport `fail`).
- Emits two gates:
  - `transport_health`
  - `semantic_quality`

## Quality gate command

Run strict non-intrusive gate:

```bash
./scripts/axon quality-mcp
```

This validates core projects (`BookingSystem`, `axon`) with strict policy while skipping write-capable tools.
