---
phase: 02-daemon-central
plan: 02
status: complete
completed: 2026-03-02
---

# Plan 02-02 Summary — MCP Proxy + max_tokens

## What Was Built

### `src/axon/mcp/server.py`

Added two helpers:

- **`_get_local_slug()`** — reads `.axon/meta.json` slug from cwd; returns `None` on any error
- **`_try_daemon_call(tool, slug, args)`** — connects to `~/.axon/daemon.sock` with 5s timeout, sends JSON-line request via `encode_request`, reads response, returns result string or `None` on error/unavailability

Updated **`call_tool()`**:
- Resolves slug from `repo=` argument or `_get_local_slug()`
- Strips `repo` and `max_tokens` from args before forwarding to daemon
- Tries daemon first via `asyncio.to_thread(_try_daemon_call, ...)`
- Falls back to `_dispatch_tool()` + `_get_storage()` on `None` result
- Applies `max_tokens` truncation (MCP-side) after result is obtained

Added `max_tokens` optional property to all 7 tool inputSchemas (`axon_list_repos`, `axon_query`, `axon_context`, `axon_impact`, `axon_dead_code`, `axon_detect_changes`, `axon_cypher`).

Also updated imports: `import socket as _socket`, `daemon_sock_path` from `axon.core.paths`, `decode_request`/`encode_request` from `axon.daemon.protocol`.

### `tests/mcp/test_server.py` (new)

14 tests across 4 classes:
- `TestGetLocalSlug` — reads slug, absent file, invalid JSON, missing key
- `TestTryDaemonCall` — socket absent, success, connection error, daemon error response, args stripping
- `TestMaxTokens` — truncation logic (long, short, None)
- `TestToolsHaveMaxTokensSchema` — all 7 tools have max_tokens, none required

## Test Results

- New tests: 14/14 passed
- Full suite: 812 passed, 0 failures (was 798 before plan)

## Architecture After 02-02

```
MCP call_tool()
  → _try_daemon_call(slug, tool, daemon_args)  [5s timeout]
    → success: return daemon result             [no local KuzuBackend]
    → fail (OSError/timeout/absent sock): None
  → fallback: _dispatch_tool() + _get_storage()
  → max_tokens truncation applied to result
```

## What Was NOT Changed

- `_dispatch_tool()` — fallback path unchanged
- `src/axon/mcp/tools.py` — handler implementations unchanged
- `src/axon/mcp/resources.py` — resources stay direct (no proxy)
- `src/axon/daemon/` — daemon package unchanged
