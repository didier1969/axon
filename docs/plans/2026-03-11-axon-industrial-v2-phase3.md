# Axon v2 : Intelligence & Stockage (Phase 3) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Integrate LadybugDB (the maintained fork of KuzuDB) into the Rust Data Plane for in-process graph storage.

**Architecture:** The Rust binary initializes `lbug` on startup, ensures the schema exists, and automatically ingests parsed symbols and their relationships into the graph in real-time.

**Tech Stack:** Rust, `lbug` (LadybugDB crate), Cypher.

---

### Task 7: Setup LadybugDB in Data Plane

**Files:**
- Modify: `src/axon-core/Cargo.toml`
- Create: `src/axon-core/src/graph.rs`
- Modify: `src/axon-core/src/main.rs`

**Step 1: Add dependencies**
```toml
lbug = "0.15.1"
```

**Step 2: Implement GraphStore (graph.rs)**
- Initialize LadybugDB in `.axon/graph_v2` directory.
- Implement `init_schema()` with Cypher:
  - `CREATE NODE TABLE IF NOT EXISTS File (path STRING, PRIMARY KEY (path))`
  - `CREATE NODE TABLE IF NOT EXISTS Symbol (name STRING, kind STRING, PRIMARY KEY (name))`
  - `CREATE REL TABLE IF NOT EXISTS CONTAINS (FROM File TO Symbol)`

**Step 3: Connect GraphStore in main.rs**
- Instantiate `GraphStore` before the parallel scan.
- Ensure schema is created.

**Step 4: Commit**
```bash
git add src/axon-core/
git commit -m "feat: integrate ladybugdb as embedded graph storage"
```

---

### Task 8: Ingestion Pipeline

**Files:**
- Modify: `src/axon-core/src/main.rs`
- Modify: `src/axon-core/src/graph.rs`

**Step 1: Implement Ingestion Logic**
- Add `insert_file_symbols(&self, path: &str, symbols: &[Symbol])` to `GraphStore`.
- Execute Cypher `MERGE` queries to insert the File, the Symbols, and the `CONTAINS` relationships.

**Step 2: Hook into the Parallel Pipeline**
- Since we use Rayon, we need thread-safe access to the database (LadybugDB connections).
- Have each thread open a connection and push results.

**Step 3: Validation**
- Run `cargo run --release ../../`
- Verify that the `.axon/graph_v2` database grows in size and query a few symbols to ensure correctness.

**Step 4: Commit**
```bash
git commit -m "feat: implement real-time graph ingestion pipeline"
```
