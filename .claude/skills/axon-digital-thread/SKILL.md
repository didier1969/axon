---
name: axon-digital-thread
description: Specialized skill for managing the "Digital Thread" of any project. It enforces a factual Pillar -> Requirement -> Concept hierarchy to prevent context bloat and AI hallucinations.
---

# Axon Digital Thread: Factual Governance

## 1. The Factual Hierarchy (Macro to Micro)

To maintain readability and cognitive efficiency, every project MUST be organized into a 3-layer hierarchy.

| Layer | Type | Constraint | Naming Rule |
| :--- | :--- | :--- | :--- |
| **MACRO** | `Pillar` | **4 to 12 max** | **FACTUAL:** Must describe a concrete function (e.g., "User Authentication", not "Safety First"). |
| **MESO** | `Requirement` | Linked to a Pillar | **SPECIFIC:** What must the system do? |
| **MICRO** | `Concept` | Linked to a Req | **TECHNICAL:** How is it implemented? |

### The DNA Rule (Sequential IDs)
Identifiers are system-assigned by the DuckDB `soll.Registry` table. Do NOT invent IDs.
- Pillars: `PIL-[PROJ_SLUG]-[NUM]`
- Requirements: `REQ-[PROJ_SLUG]-[NUM]`
- Concepts: `CPT-[PROJ_SLUG]-[NUM]`

---

## 2. Mandatory Workflow

1. **Alignment:** Check the `Pillar` nodes first via `axon_export_soll` or `axon_query`. If a task doesn't fit a pillar, discuss with the architect.
2. **Nominal Sync:** Update the graph at every step. Use factual titles. **Do not write raw SQL/Cypher to mutate SOLL.** Use the strict MCP tools (`axon_add_concept`, `axon_update_concept`).
3. **Traceability:** Link physical symbols (`IST`) to `Concept` nodes. The Rust Data Plane manages the `SUBSTANTIATES` mappings.
4. **Safety:** The `soll.db` is a physical Sanctuary. It is read-only by default for SQL queries. Mutations MUST go through the typed MCP tools.

## 3. Available MCP Tools for SOLL
*   `axon_export_soll`: Extracts the entire SOLL intentional graph into a timestamped Markdown document for human review.
*   `axon_add_concept`: Creates a new Concept, auto-increments the registry, and assigns the correct ID.
*   `axon_update_concept`: (WIP) Updates the explanation or rationale of an existing concept.

---
© 2026 Axon Intelligence Framework
