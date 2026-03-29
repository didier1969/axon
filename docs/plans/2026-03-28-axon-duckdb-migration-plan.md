# Axon DuckDB Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate the unified graph storage engine from LadybugDB/Kuzu to DuckDB, enabling proper cross-database attachment (Sanctuary architecture) and utilizing SQL/PGQ and VSS for graph and vector capabilities.

**Architecture:** We will replace the existing C-FFI plugin `axon-plugin-ladybug` with `axon-plugin-duckdb`. The Data Plane (Rust) will interface with DuckDB, manage the schema initialization using standard SQL, define a Property Graph view using `duckpgq`, and attach the isolated `soll.db` in `READ_ONLY` mode.

**Tech Stack:** Rust, DuckDB, `duckdb` crate, `duckpgq` extension, `vss` extension.

---

### Task 1: Rename and Scaffold the new C-FFI Plugin

**Files:**
- Rename dir: `src/axon-plugin-ladybug` -> `src/axon-plugin-duckdb`
- Modify: `src/axon-plugin-duckdb/Cargo.toml`
- Modify: `src/axon-plugin-duckdb/src/lib.rs`

**Step 1: Write the failing test**
Create a test in `src/axon-plugin-duckdb/src/lib.rs` to verify DuckDB initialization and extension loading.
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_duckdb_init() {
        let path = CString::new(":memory:").unwrap();
        unsafe {
            let ctx = duckdb_init_db(path.as_ptr());
            assert!(!ctx.is_null());
            
            // Check that we can execute a basic query
            let query = CString::new("SELECT 42;").unwrap();
            let res = duckdb_execute(ctx, query.as_ptr());
            assert!(res);
        }
    }
}
```

**Step 2: Run test to verify it fails**
Run: `cd src/axon-plugin-duckdb && cargo test`
Expected: Compilation failure because `duckdb_init_db` does not exist (currently `ladybug_init_db`).

**Step 3: Write minimal implementation**
1. Rename the folder `src/axon-plugin-ladybug` to `src/axon-plugin-duckdb`.
2. Update `Cargo.toml` to change the package name to `axon_plugin_duckdb` and replace `lbug` with `duckdb = { version = "1.0", features = ["vss"] }` (or similar stable duckdb crate).
3. Rewrite `lib.rs` to export `duckdb_init_db`, `duckdb_execute`, `duckdb_query_json`, `duckdb_query_count`. In `duckdb_init_db`, install and load `duckpgq` and `vss` extensions.

**Step 4: Run test to verify it passes**
Run: `cd src/axon-plugin-duckdb && cargo test`
Expected: PASS

**Step 5: Commit**
```bash
git add src/axon-plugin-duckdb src/axon-plugin-ladybug
git commit -m "refactor(core): rename ladybug plugin to duckdb and setup duckdb crate"
```

---

### Task 2: Adapt Axon Core Engine Interface

**Files:**
- Modify: `src/axon-core/src/graph.rs`

**Step 1: Write the failing test**
In `src/axon-core/src/graph.rs`, modify the existing tests to expect DuckDB schema and queries.

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test`
Expected: FAIL due to missing `libaxon_plugin_ladybug.so` or mismatched function signatures.

**Step 3: Write minimal implementation**
1. Update `find_plugin_path` to look for `libaxon_plugin_duckdb.so`.
2. Change the dynamically loaded function names from `ladybug_*` to `duckdb_*`.
3. In `GraphStore::new`, modify the initialization logic:
   - `ATTACH 'soll.db' AS soll (READ_ONLY);` (if not `:memory:`).
4. Update `init_schema` to execute standard SQL `CREATE TABLE` statements for IST and SOLL, instead of `CREATE NODE TABLE`. Add `CREATE PROPERTY GRAPH axon_graph ...` at the end.

**Step 4: Run test to verify it passes**
Run: `cd src/axon-core && cargo test`
Expected: PASS

**Step 5: Commit**
```bash
git add src/axon-core/src/graph.rs
git commit -m "feat(core): switch graph engine interface to duckdb and sql/pgq"
```

---

### Task 3: Refactor Data Insertion Queries

**Files:**
- Modify: `src/axon-core/src/graph.rs` (Insertion methods)
- Modify: `src/axon-core/src/worker.rs`

**Step 1: Write the failing test**
Ensure existing integration tests in `axon-core` cover `bulk_insert_files` and `insert_file_data_batch`.

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test`
Expected: FAIL because Cypher `MERGE` and `CREATE` statements are no longer valid SQL.

**Step 3: Write minimal implementation**
1. In `graph.rs`, update `bulk_insert_files` and `insert_file_data_batch` to use DuckDB's SQL syntax: `INSERT INTO File (path, project_slug, status, size, priority, mtime, worker_id) VALUES (...) ON CONFLICT (path) DO UPDATE SET ...`.
2. Ensure JSON parameters are correctly bound or formatted for DuckDB's execution.

**Step 4: Run test to verify it passes**
Run: `cd src/axon-core && cargo test`
Expected: PASS

**Step 5: Commit**
```bash
git add src/axon-core/src/graph.rs src/axon-core/src/worker.rs
git commit -m "feat(core): rewrite insertion queries from cypher to duckdb sql"
```

---

### Task 4: Firewall MCP Tools (Retire `axon_cypher`)

**Files:**
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Write the failing test**
Add a test in `mcp.rs` to verify that `axon_query` rejects mutation queries and that `axon_cypher` is removed.

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test mcp`
Expected: FAIL

**Step 3: Write minimal implementation**
1. Remove `axon_cypher` from the tool list and match block.
2. Ensure `axon_query` only accepts `SELECT` or `FROM GRAPH_TABLE` queries.
3. If an agent wants to mutate SOLL, they will need specific tools later (not implemented in this step, but the vulnerability is closed).

**Step 4: Run test to verify it passes**
Run: `cd src/axon-core && cargo test mcp`
Expected: PASS

**Step 5: Commit**
```bash
git add src/axon-core/src/mcp.rs
git commit -m "feat(mcp): replace axon_cypher with safe read-only axon_query for duckdb"
```