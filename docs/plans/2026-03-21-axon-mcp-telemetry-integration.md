# MCP Telemetry Integration Plan

## Problem
Axon captures WASM parsing errors in the Elixir Control Plane (SQLite `indexed_files` table), but the MCP tools (`axon_health`, `axon_audit`, etc.) are implemented in the Rust Data Plane, which only queries KuzuDB (the Graph). Because failed files are *not* in the Graph, the AI has no way of querying or knowing about these failures using the standard MCP tools.

## Optimal Integration Strategy
To provide the best service to LLM clients, we need an MCP tool that explicitly exposes the "blind spots" of the graph.

### 1. New Tool: `axon_blindspots`
We will create a new tool `axon_blindspots` in `mcp.rs`.
Instead of querying KuzuDB, this tool will query the SQLite DB (`axon_nexus.db`) managed by Elixir, directly from Rust.
It will return a list of files that failed to index and the exact `error_reason`.

### 2. Update `axon_health`
`axon_health` should also report a top-level metric: "X files failed to index due to syntax/memory errors. Use `axon_blindspots` to investigate."

## Implementation Steps
1. Add `rusqlite` dependency to `src/axon-core/Cargo.toml`.
2. Add `axon_blindspots` tool definition in `mcp.rs`.
3. Implement `axon_blindspots` to query `/home/dstadel/projects/axon/src/dashboard/axon_nexus.db`.
4. Modify `axon_health` to do a quick count of failures and mention the new tool.