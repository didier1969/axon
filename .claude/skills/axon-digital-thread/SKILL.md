---
name: axon-digital-thread
description: Specialized skill for managing the "Digital Thread" of any project. It enforces a factual Pillar -> Requirement -> Concept hierarchy to prevent context bloat and AI hallucinations.
---

# Axon Digital Thread: Factual Governance

## 1. The Fractal Hierarchy (Recursive Structure)

Axon uses a recursive model-based engineering approach. Every element belongs to a project and follows a strict hierarchy.

| Layer | Type | Relationship | Naming Rule |
| :--- | :--- | :--- | :--- |
| **MACRO** | `Vision` | `CONTRIBUTES_TO` Vision | **OBJECTIVE:** High-level strategic goal. |
| **MACRO** | `Pillar` | `EPITOMIZES` Vision | **FACTUAL:** Technical constraint or domain. |
| **STRAT** | `Milestone` | `TARGETS` Requirement | **TEMPORAL:** Phase or version goal. |
| **MESO** | `Requirement` | `BELONGS_TO` Pillar <br> `REFINES` Requirement | **SPECIFIC:** Verifiable functional goal. |
| **TACTIC** | `Decision` | `SOLVES` Requirement <br> `IMPACTS` Symbol (IST) | **ADR:** Technical choice and rationale. |
| **MICRO** | `Concept` | `EXPLAINS` Requirement <br> `SUPERSEDES` Concept | **TECHNICAL:** Implementation mechanic. |
| **META** | `Stakeholder` | `ORIGINATES` Requirement | **ROLE:** Source of the intention. |
| **PROOF** | `Validation` | `VERIFIES` Requirement | **CERT:** Physical proof of realization. |

### The DNA Rule (Sequential IDs)
Identifiers are system-assigned by the DuckDB `soll.Registry` table. Do NOT invent IDs.
- Format: `[TYPE]-[PROJ_SLUG]-[NUM]` (e.g., `REQ-AXO-001`).

### Dynamic Metadata (Scenario A)
Every entity supports a `metadata` JSON object for flexible properties (e.g., risk level, cost, secondary tags) without schema changes.

---

## 2. Mandatory Workflow

1. **Alignment:** Check existing nodes via `axon_export_soll`. Ensure new entries refine or contribute to the existing hierarchy.
2. **Atomic Mutation:** Use exclusively `axon_soll_manager`.
   - **Action `create` :** To add a new entity. Rust handles the ID generation.
   - **Action `update` :** To refine properties (including `metadata`) without breaking the Digital Thread.
   - **Action `link` :** To establish relationships (e.g., linking a Decision to a Requirement).
3. **Traceability:** Link decisions directly to code via `IMPACTS` and symbols to concepts via `SUBSTANTIATES`.
4. **Safety:** The `soll.db` is read-only for standard queries. All mutations MUST use the `axon_soll_manager` to ensure transactional integrity.

## 3. Style & Tone: Strict Architectural Concreteness
*   **Zero Marketing :** Interdiction des termes "Sanctuary", "Sacré", "Forge". Bannissez les adjectifs non-mesurables.
*   **Contenu Physique :** Chaque titre et description doit décrire une contrainte technique, une structure de donnée ou une mécanique d'implémentation.
*   **Pragmatisme Froid :** Style "Architecte Système". Si une phrase n'apporte pas de contrainte physique ou structurelle, elle est inutile.

## 4. Available MCP Tools for SOLL
*   `axon_export_soll`: Extraction Markdown horodatée pour revue humaine.
*   `axon_soll_manager`: Gestionnaire unique (create, update, link).

---
© 2026 Axon Intelligence Framework
