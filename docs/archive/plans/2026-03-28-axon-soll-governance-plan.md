# Axon SOLL Governance Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Establish strict governance over the SOLL layer by adding database indices for performance, implementing rigorous MCP mutation tools with auto-increment IDs (`soll.Registry`), and providing a Markdown extraction tool to maintain the Digital Thread.

**Architecture:** 
1. DuckDB indices on edge tables (`CALLS`, `CONTAINS`, `CALLS_NIF`) to drastically speed up `WITH RECURSIVE` queries.
2. New MCP Tools (`axon_add_requirement`, `axon_add_concept`) that encapsulate business logic. They will read `soll.Registry`, increment the sequence, format the ID (e.g., `REQ-AXO-001`), and execute the `INSERT` safely.
3. New MCP Tool (`axon_export_soll`) that queries the `soll` schema to build a structured Markdown document representing the current architectural intentions.

**Tech Stack:** Rust, DuckDB (SQL), MCP Server.

---

### Task 1: Add Performance Indices to DuckDB Schema

**Files:**
- Modify: `src/axon-core/src/graph.rs`

**Step 1: Write the failing test**
Create a test in `src/axon-core/src/graph.rs` to verify that indices exist.
```rust
    #[test]
    fn test_duckdb_indices_exist() {
        let store = GraphStore::new(":memory:").unwrap();
        // Query DuckDB internal system tables to check if our indices were created
        let index_count = store.query_count("SELECT count(*) FROM duckdb_indexes() WHERE index_name LIKE 'idx_%'").unwrap_or(0);
        assert!(index_count >= 6, "Expected at least 6 indices to be created");
    }
```

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test test_duckdb_indices_exist`
Expected: FAIL (assertion failed)

**Step 3: Write minimal implementation**
In `init_schema` of `src/axon-core/src/graph.rs`, append `CREATE INDEX` statements for the edge tables immediately after their creation.
```rust
        self.execute("CREATE INDEX IF NOT EXISTS idx_contains_source ON CONTAINS (source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_contains_target ON CONTAINS (target_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_calls_source ON CALLS (source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_calls_target ON CALLS (target_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_calls_nif_source ON CALLS_NIF (source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_calls_nif_target ON CALLS_NIF (target_id)")?;
```

**Step 4: Run test to verify it passes**
Run: `cd src/axon-core && cargo test test_duckdb_indices_exist`
Expected: PASS

**Step 5: Commit**
```bash
git add src/axon-core/src/graph.rs
git commit -m "perf(core): add B-Tree indices to graph edge tables for recursive CTE speed"
```

---

### Task 2: Implement Auto-ID SOLL Mutator MCP Tools

**Files:**
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Write the failing test**
In `src/axon-core/src/mcp.rs`, add a test for `axon_add_concept`.
```rust
    #[test]
    fn test_axon_add_concept_auto_id() {
        let server = create_test_server();
        // Initialize registry
        server.graph_store.execute("INSERT INTO soll.Registry (id, last_req, last_cpt, last_dec) VALUES ('AXON_GLOBAL', 0, 10, 0)").unwrap();
        
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_add_concept",
                "arguments": {
                    "name": "Test Concept",
                    "explanation": "To test auto id",
                    "rationale": "Because testing is good"
                }
            })),
            id: Some(json!(1)),
        };
        
        let response = server.handle_request(req);
        let result = response.unwrap().result.unwrap();
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        
        assert!(content.contains("CPT-AXO-011"));
        
        // Verify in DB
        let count = server.graph_store.query_count("SELECT count(*) FROM soll.Concept WHERE name = 'CPT-AXO-011: Test Concept'").unwrap();
        assert_eq!(count, 1);
    }
```

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test test_axon_add_concept_auto_id`
Expected: FAIL (tool not found)

**Step 3: Write minimal implementation**
1. Add `axon_add_concept` to the `tools/list` response in `mcp.rs`.
2. Implement `fn axon_add_concept(&self, args: &Value) -> Option<Value>` in `McpServer`.
   - Read `last_cpt` from `soll.Registry`.
   - Increment it: `UPDATE soll.Registry SET last_cpt = last_cpt + 1 WHERE id = 'AXON_GLOBAL' RETURNING last_cpt`.
   - Format ID: `CPT-AXO-{:03}`.
   - Insert into `soll.Concept`: `INSERT INTO soll.Concept (name, explanation, rationale) VALUES ($formatted_name, $expl, $rat)`.
3. Wire the tool call in the `match name.as_str()` block.

**Step 4: Run test to verify it passes**
Run: `cd src/axon-core && cargo test test_axon_add_concept_auto_id`
Expected: PASS

**Step 5: Commit**
```bash
git add src/axon-core/src/mcp.rs
git commit -m "feat(mcp): implement safe SOLL mutation tool with auto-increment IDs"
```

---

### Task 3: Implement SOLL Markdown Exporter MCP Tool

**Files:**
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Write the failing test**
In `src/axon-core/src/mcp.rs`, add a test for `axon_export_soll`.
```rust
    #[test]
    fn test_axon_export_soll() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO soll.Vision (title, description, goal) VALUES ('Test Vision', 'Desc', 'Goal')").unwrap();
        server.graph_store.execute("INSERT INTO soll.Concept (name, explanation, rationale) VALUES ('CPT-AXO-001: My Concept', 'Expl', 'Rat')").unwrap();
        
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_export_soll",
                "arguments": {}
            })),
            id: Some(json!(2)),
        };
        
        let response = server.handle_request(req);
        let result = response.unwrap().result.unwrap();
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        
        assert!(content.contains("# SOLL Extraction"));
        assert!(content.contains("Test Vision"));
        assert!(content.contains("CPT-AXO-001"));
        assert!(content.contains("Exported to"));
    }
```

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test test_axon_export_soll`
Expected: FAIL

**Step 3: Write minimal implementation**
1. Add `axon_export_soll` to `tools/list`.
2. Implement `fn axon_export_soll(&self) -> Option<Value>`.
   - Query `soll.Vision`, `soll.Pillar`, `soll.Requirement`, `soll.Concept`.
   - Format them into a single comprehensive Markdown string.
   - Use `chrono` (or standard time if available) to get a timestamp.
   - Write to `docs/vision/SOLL_EXPORT_<timestamp>.md` using `std::fs::write`. Create dirs if needed.
   - Return a success message with the file path and preview.
3. Wire the tool call.

**Step 4: Run test to verify it passes**
Run: `cd src/axon-core && cargo test test_axon_export_soll`
Expected: PASS

**Step 5: Commit**
```bash
git add src/axon-core/src/mcp.rs
git commit -m "feat(mcp): implement SOLL Markdown structured export tool"
```