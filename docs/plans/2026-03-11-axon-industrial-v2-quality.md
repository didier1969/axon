# Axon v2 : Qualité & Couverture (Phase 5) Implementation Plan

**Goal:** Achieve >85% test coverage with 100% pass rate across Unit, Integration, and E2E tests.

**Architecture:**
- **Unit Tests:** Atomic logic in Rust (Scanner, Parser, Graph) and Elixir (BridgeClient).
- **Integration Tests:** Interaction between Rust modules (Parsing -> Ingestion) and Elixir components (PubSub -> LiveView).
- **E2E Tests:** Full system orchestration (Data Plane --[UDS]--> Control Plane).

---

### Task 12: Rust Comprehensive Testing (Data Plane)

**Files:**
- Create: `src/axon-core/src/graph.rs` (Add more unit tests)
- Create: `src/axon-core/src/mcp.rs` (Add unit tests for JSON-RPC)
- Create: `src/axon-core/tests/integration_test.rs` (New Integration Suite)

**Step 1: Unit Tests for Graph & MCP**
- Test `insert_file_symbols` with edge cases (empty symbols, special characters).
- Test `McpServer::handle_request` with various JSON-RPC payloads.

**Step 2: Integration Test (Scan -> Ingest)**
- Mock a project, run the parallel scanner/parser, and verify LadybugDB contents.

**Step 3: Commit**
```bash
git commit -m "test(rust): add unit and integration tests for graph and mcp"
```

---

### Task 13: Elixir Comprehensive Testing (Control Plane)

**Files:**
- Create: `src/dashboard/test/axon_dashboard/bridge_client_test.exs`
- Create: `src/dashboard/test/axon_dashboard_web/live/status_live_test.exs`

**Step 1: Test BridgeClient**
- Mock a Unix socket and verify `Msgpax` decoding and PubSub broadcasting.

**Step 2: Test StatusLive**
- Use `Phoenix.LiveViewTest` to simulate bridge events and verify UI updates (stats, table rows).

**Step 3: Commit**
```bash
git commit -m "test(elixir): add unit and liveview tests for dashboard"
```

---

### Task 14: E2E System Orchestration Test

**Files:**
- Create: `tests/e2e_v2_orchestration.py` (Script de test global)

**Step 1: Implement E2E Script**
- Launch `axon-core` in a temporary directory.
- Launch `axon_dashboard`.
- Simulate a file change.
- Verify that the Dashboard UI/Logs reflect the change.

---

### Task 15: Coverage Audit & Final PASS

**Step 1: Measure Rust Coverage**
- Run `cargo tarpaulin` or `cargo llvm-cov`.

**Step 2: Measure Elixir Coverage**
- Run `mix test --cover`.

**Step 3: Refinement**
- Add tests for uncovered branches until 85% is reached.
