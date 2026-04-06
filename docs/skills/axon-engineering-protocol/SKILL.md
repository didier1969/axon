---
name: axon-engineering-protocol
description: Core engineering protocol for Axon. MUST BE READ before any coding, refactoring, or SOLL mutation. Defines IST-driven execution, TDD mandates, and the mandatory axon_commit_work MCP workflow.
---

# Axon Engineering Protocol

## Core Principles & Identity Contract
- **Read/Verify First:** Always query IST/SOLL before mutating. Certify after.
- **Server-Owned Identity:** LLMs NEVER invent IDs.
  - Canonical IDs: `TYPE-CODE-NNN` (e.g., `REQ-AXO-001`). `CODE` is from `.axon/meta.json`.
  - Batch IDs: `PRV-CODE-NNN` (preview), `REV-CODE-NNN` (revision).
  - Use `logical_key` in batch payloads (`soll_apply_plan`); the server resolves them and returns `identity_mapping`.
- **Zero Hallucination:** Rely strictly on MCP tool outputs.

## SOLL Semantic Ontology
- `Vision` (1/proj): Strategic outcome (no tech details).
- `Pillar` (3-7): Strategic principle.
- `Requirement` (15-60): Testable capability (L1 parent -> L2 child).
- `Decision`: Tech choice solving requirements.
- `Concept`: Domain vocabulary.
- `Guideline`: Perpetual engineering rule (e.g., TDD).
- `Milestone`: Delivery checkpoint.
- `Validation`: Proof of requirement.
- `Stakeholder`: Impacted actor.

## MCP Tooling Surface
**Identity & Projects:**
- `axon_init_project`: Creates project, reads global guidelines.
- `axon_apply_guidelines`: Inherits global rules locally (`GUI-PRO-XXX` -> `GUI-SLUG-XXX`).

**Unit Mutations (Immediate):**
- `soll_manager(action="create|update|link", entity, data)`:
  - `create`: returns canonical ID.
  - `update`: requires canonical ID.
  - `link`: `source_id`, `target_id`. Server enforces adjacency matrix.

**Batch Mutations (Transactional):**
- `soll_apply_plan(project_slug, author, dry_run, plan, relations, evidence)`: Zero-shot payload. Use `logical_key` for cross-referencing. Returns `identity_mapping` & `preview_id`.
- `soll_commit_revision(preview_id)`: Commits batch. Returns `revision_id`.
- `soll_rollback_revision(revision_id)`: Reverts.

**Audit & Traceability:**
- `axon_commit_work(diff_paths, message, dry_run)`: **MANDATORY** for code commits. Validates IST against SOLL Guidelines. Generates Markdown Doc-As-Code, executes Git.
- `soll_validate(project_slug)`: Audits graph invariants.
- `soll_verify_requirements()`: Computes requirement coverage.
- `soll_export(project_slug)`: Generates canonical Markdown backup.
- `restore_soll(path)`: Replays a Markdown snapshot.

**Context & Discovery:**
- `mcp_axon_query` / `mcp_axon_inspect`: Read IST (Code).
- `axon_impact(symbol)`: **CRITICAL for refactoring.** Traces a code symbol's impact all the way up through the call graph and across the bridge to SOLL (revealing compromised Decisions, Requirements, and Visions).
- `soll_query_context`: Read SOLL state.
- `soll_work_plan(project_slug, json=true)`: Ordered execution view (blockers, waves).

## IST-Driven Execution (Mandatory Workflow)
Before coding or mutating SOLL, you MUST:
1. **Query IST:** Run `mcp_axon_query` or `mcp_axon_inspect` on the target component. If refactoring, run `axon_impact` to assess strategic blast radius.
2. **Verify Drift:** Compare IST with SOLL plan. Formulate the gap.
3. **Plan & Execute:** Apply TDD. Write code.
4. **Wire (Traceability):** Link new/modified IST symbols to SOLL origins (`SUBSTANTIATES` or `IMPACTS`) using `soll_manager link`.
   - `SUBSTANTIATES`: SOLL (concept/req/dec) ↔ IST symbol.
   - *Rule:* If code changes and intention changes, update SOLL in the same wave. Implementation is incomplete until mathematically linked to SOLL.

## Payload Contract (`soll_apply_plan`)
```json
{
  "plan": {
    "requirements": [{"logical_key": "req-1", "title": "Auth", "priority": "P1", "status": "current"}],
    "decisions": [{"logical_key": "dec-1", "title": "Use JWT"}]
  },
  "relations": [
    {"source_id": "dec-1", "target_id": "req-1", "relation_type": "SOLVES"}
  ]
}
```
*Note:* The server atomically resolves `dec-1` to `DEC-AXO-001`, validating relations against the canonical policy.

## Style Attendu (Writing Guidelines)
- **Factual & Measurable:** No marketing fluff.
- **Requirements:** Must be inherently testable.
- **Decisions:** Must include context, rationale, and impact.

## Skill Maintenance
Update this file immediately if:
- MCP tools are added/renamed/modified.
- SOLL schema or relation taxonomy changes.
- Governance policies evolve.