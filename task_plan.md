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
