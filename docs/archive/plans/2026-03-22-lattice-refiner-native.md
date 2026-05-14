# Lattice Refiner (Native Rust) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a high-performance entity resolution engine in Rust to identify and link duplicate/similar symbols across the entire Global Knowledge Lattice using fuzzy matching and vector similarity.

**Architecture:**
1.  **Axon.Refiner (Rust):** A new internal module in `axon-core` performing batch comparisons of symbols.
2.  **Semantic Similarity:** Combining RapidFuzz (string) and L2 Distance (FastEmbed vectors).
3.  **Lattice Enrichment:** Creating `[:SAME_AS]` relationships in KuzuDB to unify fragmented project knowledge.
4.  **Proactive MCP:** Updating MCP tools to follow `[:SAME_AS]` links for richer context.

**Tech Stack:** Rust, RapidFuzz, Rayon, KuzuDB, Elixir.

---

### Task 1: Setup Similarity Engine

**Files:**
- Modify: `src/axon-core/Cargo.toml`
- Create: `src/axon-core/src/refiner.rs`

**Step 1: Add dependencies**
Add `rapidfuzz = "0.5"` to `Cargo.toml`.

**Step 2: Implement Similarity Traits**
Implement a `SimilarityScorer` that calculates a weighted score between two symbols based on name (Fuzzy) and kind.

---

### Task 2: Implement Blocking & Parallel Matching

**Files:**
- Modify: `src/axon-core/src/refiner.rs`
- Modify: `src/axon-core/src/main.rs`

**Step 1: Batch Fetch & Blocking**
Retrieve all symbols from KuzuDB and group them by `kind` (Blocking) to avoid $O(N^2)$ global comparison.

**Step 2: Parallel Scoring with Rayon**
Use `rayon` to compute fuzzy similarities in parallel across all blocks.

---

### Task 3: Lattice Materialization (SAME_AS)

**Files:**
- Modify: `src/axon-core/src/graph.rs`
- Modify: `src/axon-core/src/refiner.rs`

**Step 1: Schema Update**
Update `init_schema` in `graph.rs` to include `CREATE REL TABLE IF NOT EXISTS SAME_AS (FROM Symbol TO Symbol, score DOUBLE)`.

**Step 2: Batch Insert Relations**
Implement a method to insert found matches into KuzuDB using the batch transaction system.

---

### Task 4: Elixir Orchestration & MCP Integration

**Files:**
- Create: `src/dashboard/lib/axon_nexus/axon/refiner_server.ex`
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Triggering Refinement**
Add a GenServer in Elixir that sends a `REFINE_LATTICE` command to Rust after a major ingestion or when the system is idle.

**Step 2: MCP Omniscience**
Update `axon_inspect` to check for `SAME_AS` relations. If found, the AI report should mention: *"This symbol is a 96% match with [Symbol B] in [Project Y]."*
