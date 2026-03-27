# Plan: Stabilize MCP Server and Ingestion System

## Goal
Ensure the Axon MCP server and its underlying ingestion pipeline are absolutely reliable and robust before adding new language parsers.

## Phases

### Phase 1: Investigation & Reproducing Errors (Systematic Debugging)
- [x] Reproduce current MCP server failures (using test scripts or direct invocation).
- [x] Analyze the ingestion pipeline's end-to-end reliability (Watcher -> Oban -> Rust Bridge -> Database).
- [x] Identify root causes of unreliability or crashes.

### Phase 2: Fix Ingestion Pipeline Reliability
- [x] Implement defenses against identified ingestion failures (e.g., bridge disconnects, db locks, malformed messages).
- [x] Verify ingestion stability under load.

### Phase 3: Fix MCP Server Reliability & Vision Alignment
- [x] Resolve root causes of MCP server failing to respond or functioning incorrectly.
- [x] Ensure MCP endpoints cleanly interact with the graph database without blocking or crashing.
- [x] **Maestria Implementation:** Semantic Synthesis, Cross-Project Federation, and Proactive Notifications.

### Phase 4: Final Validation
- [x] Run comprehensive test suite for MCP.
- [x] Verify long-running background ingestion doesn't impact MCP availability.

### Phase 5: Zero-Sleep & MVCC Implementation (Maestria)
- [x] Task 5.1: Refactor Rust `GraphStore` and `main.rs` to remove `RwLock` and use native MVCC connections.
- [x] Task 5.2: Purge all `sleep` and `mcp_active` logic from `worker.rs`.
- [x] Task 5.3: Optimize Elixir `PoolFacade` and Oban config for hardware-aware backpressure.
- [x] Task 5.4: Validate Zero-Latency MCP under 36k file load.

### Phase 6: Deadlock Resolution & I/O Multiplexing
- [ ] Task 6.1: Increase Rust `QueueStore` capacity to 50,000 slots.
- [ ] Task 6.2: Refactor Elixir `PoolFacade` to decouple sending from receiving (Async Send).
- [ ] Task 6.3: Implement `{:active, :once}` or flow control in `PoolFacade` to ensure priority ACK processing.
- [ ] Task 6.4: Final Validation - Observe counter increasing beyond 3 files.
