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
2. **Nominal Sync:** Update the graph at every step. Use factual titles. **Do not write raw SQL/Cypher to mutate SOLL.** Use the strict MCP tools (`axon_soll_manager`).
3. **Traceability:** Link physical symbols (`IST`) to `Concept` nodes. The Rust Data Plane manages the `SUBSTANTIATES` mappings.
4. **Safety:** The `soll.db` is a physical Sanctuary. It is read-only by default for SQL queries. Mutations MUST go through the typed MCP tools.

## 3. Style & Tone: Strict Architectural Concreteness
*   **Interdiction du Marketing :** Bannissez les adjectifs enthousiastes, les promesses vagues et le langage "visionnaire".
*   **Contenu Physique :** Chaque titre et description doit décrire une contrainte technique, une structure de donnée ou une mécanique d'implémentation.
*   **Pragmatisme Froid :** Nous sommes des architectes système. Le texte doit porter une information structurelle pure. Si une phrase n'apporte pas de preuve ou de contrainte, supprimez-la.

## 4. Available MCP Tools for SOLL
*   `axon_export_soll`: Extracts the entire SOLL intentional graph into a timestamped Markdown document for human review.
*   `axon_soll_manager`: Command center for SOLL. Actions: `create`, `update`, `link`. Manages Registry auto-IDs.

---
© 2026 Axon Intelligence Framework
