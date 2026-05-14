# Refactoring Plan for `src/axon-core/src/mcp.rs`

Date: 2026-03-30
Status: proposed
Scope: incremental extraction without breaking MCP tool contracts

## Why Refactor Now

`mcp.rs` has become the concentration point for:

- JSON-RPC protocol handling
- tool catalog exposure
- tool dispatch
- SOLL export / restore parsing
- query and inspection logic
- audit and health reporting
- helper formatting
- MCP tests

This file is now too broad to evolve safely. The immediate risk is not only readability. It is coupling:

- a change in SOLL parsing can affect unrelated DX tools
- tool registration and tool behavior live in the same file
- tests are forced to anchor on a monolith
- future validation en conditions reelles scenarios will be harder to maintain

## Refactoring Constraints

The refactor must preserve:

- external MCP tool names
- current JSON-RPC behavior
- current green Rust signal
- current SOLL export and restore format
- current value-oriented wording for `axon_query`

The refactor should avoid:

- large rename waves
- behavior changes hidden inside structural movement
- simultaneous protocol redesign

## Target Shape

Keep a thin `mcp.rs` as entrypoint and move responsibilities into focused modules:

- `mcp/mod.rs`
  - `McpServer`
  - request entrypoints
  - common shared types
- `mcp/protocol.rs`
  - `JsonRpcRequest`
  - `JsonRpcResponse`
  - `JsonRpcNotification`
- `mcp/catalog.rs`
  - tool list declaration
  - per-tool metadata and schemas
- `mcp/dispatch.rs`
  - tool name to handler routing
- `mcp/tools/dx.rs`
  - `axon_fs_read`
  - `axon_query`
  - `axon_inspect`
  - `axon_bidi_trace`
  - `axon_debug`
- `mcp/tools/soll.rs`
  - `axon_soll_manager`
  - `axon_export_soll`
  - `axon_restore_soll`
  - SOLL markdown parsing helpers
- `mcp/tools/governance.rs`
  - `axon_audit`
  - `axon_health`
  - `axon_architectural_drift`
  - `axon_semantic_clones`
  - `axon_api_break_check`
- `mcp/tools/risk.rs`
  - `axon_impact`
  - `axon_diff`
  - `axon_simulate_mutation`
- `mcp/tools/system.rs`
  - `axon_refine_lattice`
  - `axon_batch`
  - `axon_cypher`
- `mcp/format.rs`
  - table formatting and response formatting helpers
- `mcp/tests/*.rs`
  - grouped by domain instead of one monolithic test block

## Recommended Extraction Order

### Phase 1: Zero-Risk Structural Split

Move without changing logic:

1. protocol structs to `mcp/protocol.rs`
2. formatting helpers to `mcp/format.rs`
3. SOLL parse helper structs/functions to `mcp/tools/soll.rs`

Acceptance:

- no tool behavior change
- `cargo test` still green

### Phase 2: Tool Catalog and Dispatch Separation

Extract:

1. `tools/list` payload into `mcp/catalog.rs`
2. `match name` dispatch into `mcp/dispatch.rs`

Acceptance:

- tool list stays identical
- dispatch stays identical
- tests around `tools/list` and representative tool calls stay green

### Phase 3: Domain Tool Modules

Move handlers by domain:

1. SOLL
2. DX
3. governance
4. risk
5. system

Acceptance:

- each phase individually tested
- no simultaneous semantic edits during movement

### Phase 4: Test Reorganization

Split tests by domain:

- `mcp/tests/catalog_tests.rs`
- `mcp/tests/soll_tests.rs`
- `mcp/tests/dx_tests.rs`
- `mcp/tests/governance_tests.rs`

Acceptance:

- tests remain behavior-focused
- new tests can be added without growing one giant block

## Immediate Tactical Recommendation

Do not start with behavior changes.

Start with:

1. extract protocol types
2. extract SOLL parsing helpers
3. extract tool catalog constants

Why:

- these are the least risky seams
- they reduce file size quickly
- they prepare later movement of handlers without breaking the mental model

## What Must Stay Central

Even after refactor, keep these central in one visible place:

- MCP version and server info
- the canonical list of exposed tool names
- the public `handle_request` entrypoint

That centrality matters for operability and for agent-facing trust.
