---
name: axon-mcp-server-operator
description: Skill for operating Axon MCP over HTTP JSON-RPC. Use when an LLM must discover tools with tools/list, execute tools/call safely, and run reliable request patterns for SOLL and runtime diagnostics.
---

# Axon MCP Server Operator

## Use this skill when
- You need to interact with `http://127.0.0.1:44129/mcp`.
- You need deterministic `tools/list` and `tools/call` flows.
- You need safe execution order for SOLL tools and diagnostics.

## Canonical request flow
1. Call `tools/list` first.
2. Validate the target tool name is present.
3. Call `tools/call` with explicit arguments.
4. Check `isError` and parse `content[0].text`.
5. For SOLL plans, prefer v2 flow:
6. `axon_soll_apply_plan_v2` with `dry_run=true`.
7. `axon_soll_commit_revision`.
8. `axon_soll_verify_requirements`.

## JSON-RPC templates

`tools/list`
```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "tools/list",
  "params": {}
}
```

`tools/call`
```json
{
  "jsonrpc": "2.0",
  "id": "2",
  "method": "tools/call",
  "params": {
    "name": "axon_soll_query_context",
    "arguments": { "project_slug": "AXO", "limit": 25 }
  }
}
```

## Safety rules
- Never assume a tool exists without `tools/list`.
- Never skip `dry_run` for large SOLL updates.
- Never use ad-hoc SQL writes for SOLL when MCP tools exist.
- Always surface `preview_id` and `revision_id` in outputs.

## Minimal troubleshooting
- `Tool not found`: stale runtime, restart core.
- HTTP 5xx: inspect core logs and retry once.
- `isError=true`: return the exact text + failing payload.
