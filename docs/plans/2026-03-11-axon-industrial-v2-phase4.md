# Axon v2 : Serveur MCP Natif (Phase 4) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Port the MCP server from Python to Rust within the `axon-core` binary to provide ultra-fast code intelligence tools.

**Architecture:** `axon-core` will support an `--mcp` mode. It implements the Model Context Protocol (JSON-RPC over stdin/stdout) and maps tools directly to Cypher queries on the local LadybugDB.

**Tech Stack:** Rust, `serde_json`, `lbug` (Cypher queries).

---

### Task 9: MCP Protocol & JSON-RPC (Rust)

**Files:**
- Create: `src/axon-core/src/mcp.rs`
- Modify: `src/axon-core/src/main.rs`

**Step 1: Define MCP JSON-RPC structures**
- `JsonRpcRequest`, `JsonRpcResponse`, `CallToolRequest`, etc.

**Step 2: Implement Stdin/Stdout loop**
- Read lines from stdin, parse as JSON-RPC.
- Handle `initialize`, `list_tools`, and `call_tool`.

**Step 3: Commit**
```bash
git add src/axon-core/
git commit -m "feat: implement native MCP JSON-RPC protocol in rust"
```

---

### Task 10: Port Core Tools to Cypher

**Files:**
- Modify: `src/axon-core/src/mcp.rs`
- Modify: `src/axon-core/src/graph.rs`

**Step 1: Implement `axon_query`**
- Tool that takes a `cypher` string.
- Executes it on LadybugDB and returns formatted JSON results.

**Step 2: Implement `axon_fleet_status`**
- Query LadybugDB for node counts (Files, Symbols).

**Step 3: Implement `axon_context`**
- Cypher traversal: `MATCH (f:File {path: $path})-[:CONTAINS]->(s) RETURN s`.

**Step 4: Commit**
```bash
git commit -m "feat: port mcp tools to native cypher queries"
```

---

### Task 11: Unified CLI Entry Point

**Files:**
- Modify: `src/axon-core/src/main.rs`

**Step 1: Add Argument Parsing**
- Use `std::env::args`.
- Default: Scan mode (Phase 1-3).
- Flag `--mcp`: Starts the persistent MCP server.

**Step 2: Integration Test**
- Launch `axon-core --mcp`.
- Send a `list_tools` request manually.
- Verify JSON-RPC compliance.

**Step 3: Commit**
```bash
git commit -m "feat: add --mcp mode to axon-core binary"
```
