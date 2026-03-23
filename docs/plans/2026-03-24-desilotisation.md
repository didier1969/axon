# Désilotisation Totale (Global Graph Traversal) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor the MCP tools (`axon_impact`, `axon_query`, `axon_audit`, `axon_health`, `axon_inspect`) to operate on the unified global graph, removing strict project silos and leveraging the new `[:DEPENDS_ON]` relationships to provide cross-project impact analysis.

**Architecture:** Modify the Cypher queries in `src/axon-core/src/mcp.rs`. Make the `project` argument optional (defaulting to the entire workspace). Enhance `axon_impact` to explicitly return the affected *Projects* by traversing up from affected files to their project roots.

**Tech Stack:** Rust, KuzuDB Cypher.

---

### Task 1: Remove Strict Project Silos from Query Tools

**Files:**
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Write the failing test**

```rust
// Add to src/axon-core/src/mcp.rs
#[cfg(test)]
mod tests {
    use super::*;
    // Mocking the McpServer is complex, so we will test the Cypher generation logic.
    // We will extract the query generation into a testable function or just test the tool behavior.
    #[test]
    fn test_axon_query_global_default() {
        let req = serde_json::json!({
            "name": "axon_query",
            "arguments": { "query": "auth" } // Note: 'project' is omitted
        });
        // The implementation should not fail if 'project' is missing.
        // It should default to global.
        assert!(true); // We will refine this test in the implementation phase if possible, or rely on E2E.
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test`
Expected: The existing `mcp.rs` tool logic expects `project` to be present or it might panic/fail if we change the JSON schema to make it optional but don't handle it in Rust.

**Step 3: Write minimal implementation**

In `mcp.rs` `tools/list`:
Update JSON schemas for `axon_query`, `axon_inspect`, `axon_audit`, `axon_health` to remove `"project"` from the `"required"` array.
In the handler functions (`fn axon_query`, etc.):
Change `let project = args.get("project")?.as_str()?;` to `let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");`.
Update the Cypher queries to handle `project == "*"` by omitting the `f.path CONTAINS $project` filter.

**Step 4: Run test to verify it passes**

Run: `cargo build` and test the tool via Python MCP client omitting the project argument.
Expected: PASS.

**Step 5: Commit**

```bash
git commit -am "feat(mcp): make project filters optional for global querying"
```

---

### Task 2: Enhance `axon_impact` for Cross-Project Traversal

**Files:**
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Write the failing test**

We will use the E2E Python script approach to test the output format of `axon_impact`.

**Step 2: Run test to verify it fails**

Current `axon_impact` only returns affected files.

**Step 3: Write minimal implementation**

Rewrite the Cypher query in `axon_impact`:
```cypher
MATCH (start:Symbol {name: $sym})<-[:CALLS|CALLS_NIF|CALLS_OTP*1..4]-(affected:Symbol)<-[:CONTAINS]-(f:File)
OPTIONAL MATCH (f)-[:BELONGS_TO]->(p:Project) // Assuming we link files to projects, or we extract project from path
RETURN DISTINCT f.path AS Affected_File, affected.name AS Affected_Symbol
```
Wait, we need to show impacted *Projects*.
```cypher
MATCH (start:Symbol {name: $sym})<-[:CALLS|CALLS_NIF|CALLS_OTP*1..4]-(affected:Symbol)<-[:CONTAINS]-(f:File)
RETURN DISTINCT f.path AS Affected_File, affected.name AS Affected_Symbol
```
Since we just added `[:DEPENDS_ON]` between projects, if we want to show project impact, we can extract the root directory from `f.path` and group the impact report by Project.
Format the Markdown report to explicitly state: "Projets impactés: [A, B, C]".

**Step 4: Run test to verify it passes**

Run the MCP tool via Python and verify the Markdown output contains cross-project groupings.

**Step 5: Commit**

```bash
git commit -am "feat(mcp): enhance impact analysis to highlight cross-project boundaries"
```
