# Axon Engine - DuckDB Migration Design (Phase Apollo)

## 1. Context & Motivation
Axon v2 relied on LadybugDB/KuzuDB for its unified graph storage. However, the requirement to physically isolate the intentional layer (SOLL) from the physical layer (IST) revealed a critical limitation in KuzuDB: the `ATTACH` statement does not support cross-database queries or multiple active write contexts. 

To achieve the "Sanctuary Architecture" (100% isolation of SOLL from AI hallucinations) while maintaining graph traversals and vector searches, we are migrating the core database engine from LadybugDB to **DuckDB**.

## 2. Core Technologies
*   **DuckDB:** In-process SQL OLAP database.
*   **duckpgq:** Official DuckDB extension for Property Graph Queries (SQL:2023 standard).
*   **vss:** Official DuckDB extension for Vector Similarity Search (HNSW index for `FLOAT[384]`).
*   **Rust (Data Plane):** The C-FFI plugin will be rewritten from `axon-plugin-ladybug` to `axon-plugin-duckdb`.

## 3. Architecture & Data Model

### 3.1. Physical Isolation (The Sanctuary)
DuckDB will open two distinct physical files:
1.  `soll.db` (The Sanctuary): Attached in `READ_ONLY` mode by the operational LLM agents. Contains Vision, Pillars, Requirements, Concepts.
2.  `ist.db` (The Forge): The primary active database containing the parsed AST, Files, and Symbols.

### 3.2. Relational Schema to Graph Projection
Unlike Kuzu (which is graph-native), DuckDB is relational. The data will be stored in highly optimized Parquet/DuckDB tables and projected as a graph.

**Tables (IST Layer):**
*   `ist_File (path, project_slug, status, size, priority, mtime, worker_id)`
*   `ist_Symbol (id, name, kind, tested, is_public, is_nif, embedding FLOAT[384])`
*   `ist_Project (name)`
*   `ist_Contains (file_path, symbol_id)`
*   `ist_Calls (caller_id, callee_id)`

**Tables (SOLL Layer):**
*   `soll_Vision (title, description, goal)`
*   `soll_Pillar (id, title, description)`
*   `soll_Requirement (id, title, description, justification, priority)`
*   `soll_Concept (name, explanation, rationale)`
*   `soll_Substantiates (concept_name, symbol_id)` - *Note: This bridging table will physically reside in `ist.db` but logically link SOLL to IST.*

**Property Graph View (`duckpgq`):**
```sql
CREATE PROPERTY GRAPH axon_graph
VERTEX TABLES (
    ist_File LABEL File,
    ist_Symbol LABEL Symbol,
    soll_Concept LABEL Concept
)
EDGE TABLES (
    ist_Contains SOURCE KEY (file_path) REFERENCES ist_File (path) DESTINATION KEY (symbol_id) REFERENCES ist_Symbol (id) LABEL CONTAINS,
    soll_Substantiates SOURCE KEY (concept_name) REFERENCES soll_Concept (name) DESTINATION KEY (symbol_id) REFERENCES ist_Symbol (id) LABEL SUBSTANTIATES
);
```

## 4. MCP Firewall (The Shield)
To prevent agents from hallucinating destructive SQL (e.g., `DROP TABLE soll_Concept`), the `axon_cypher` MCP tool will be replaced:
1.  `axon_query`: Read-only SQL/PGQ queries. Blocked at the Rust layer if mutation keywords are detected.
2.  `axon_add_concept`, `axon_link_symbol`, etc.: Strongly typed, parameterized MCP tools for writing to the SOLL layer safely.

## 5. Migration Strategy (Option A - Radical Rename)
1.  **Codebase Refactoring:** Rename `src/axon-plugin-ladybug` to `src/axon-plugin-duckdb`. Update `Cargo.toml`, Elixir NIF bindings, and all module names.
2.  **Engine Replacement:** Replace the C-FFI Rust implementation to use the `duckdb` crate instead of `lbug`.
3.  **Data Preservation:** The old `.axon/graph_v2` will remain untouched until `.axon/duck_v3` is proven stable and passes all tests.
4.  **Testing:** Strict TDD verification of graph projections, vector search, and cross-db `ATTACH`.
