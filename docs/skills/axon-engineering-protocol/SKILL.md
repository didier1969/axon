---
name: axon-engineering-protocol
description: Use when working in the Axon repository before coding, refactoring, structural diagnostics, or SOLL mutation. Defines the single canonical operator workflow for runtime truth, intent traceability, pre-flight validation, and commit discipline.
---

# Axon Engineering Protocol

## Environment Isolation & MCP Sandbox Rule
- **Production Server (Port 44129):** The central Control Plane serving all projects. Must run H24.
- **Development Worktree (e.g. `.worktrees/dev/...`):** Used strictly as a compilation/TDD laboratory with an offline copy of the database.
- **Anti-Sandbox Rule for MCP:** LLM clients (Gemini, Claude) automatically sandbox MCP processes if they see a `devenv.nix` file, which corrupts the JSON-RPC stdio protocol. To prevent this:
  - The MCP tunnel MUST be installed globally outside the project (`~/.local/bin/axon-mcp`).
  - Or, local project settings (`.gemini/settings.json`) MUST explicitly declare `"sandbox": false`.
- **Silent Dev Rule:** NEVER start the MCP server inside the development environment. Exposing two MCP servers with duplicate tool names crashes LLM clients.

## Core Principles & Identity Contract
- **Read/Verify First:** Always query IST/SOLL before mutating. Certify after.
- **Server-Owned Identity:** LLMs NEVER invent IDs.
  - Canonical IDs: `TYPE-CODE-NNN` (e.g., `REQ-AXO-001`). `CODE` is from `.axon/meta.json`.
  - Batch IDs: `PRV-CODE-NNN` (preview), `REV-CODE-NNN` (revision).
  - Use `logical_key` in batch payloads (`soll_apply_plan`); the server resolves them and returns `identity_mapping`.
- **Zero Hallucination:** Rely strictly on MCP tool outputs.

## Canonical Operator Flow
Use this single skill as the entry point for both:
- code/intention work
- structural diagnostics
- SOLL mutation

Default order:
1. `status`
2. `project_status` when an agent needs an immediate live situation report for a project
3. `impact` if code shape or blast radius may change
4. `why` if rationale / requirement / architectural intent matters
5. `path` if topology or source/sink flow matters
6. `anomalies` for cleanup, refactor, debt, or structural review
7. `conception_view` when a derived module/contract/flow map is needed before editing
8. `change_safety` before mutating a risky symbol or intent anchor
9. `snapshot_history` / `snapshot_diff` when structural evolution since the last session matters
10. `axon_pre_flight_check` before commit
11. `axon_commit_work` only after pre-flight passes

Live project situation:
- use `project_status` for project state, vision anchor, operator surface, degradation, derived diagnostics, and runtime health
- `project_status` must read its `Vision Anchor` from live `SOLL` source, not from `soll_export`
- `snapshot_history` and `snapshot_diff` are derived non-canonical memory surfaces stored outside `SOLL`
- `conception_view` and `change_safety` are derived read surfaces; they must not be mistaken for canonical intention truth
- canonical SOLL document export remains `soll_export`
- use `soll_export`, `soll_query_context`, and `soll_work_plan` for the actual intention graph

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
- `axon_apply_guidelines`: Inherits global rules locally (`GUI-PRO-XXX` -> `GUI-CODE-XXX`).

**Unit Mutations (Immediate):**
- `soll_manager(action="create|update|link", entity, data)`:
  - `create`: returns canonical ID.
  - `update`: requires canonical ID.
  - `link`: `source_id`, `target_id`. Server enforces adjacency matrix.

**Batch Mutations (Transactional):**
- `soll_apply_plan(project_code, author, dry_run, plan, relations, evidence)`: Zero-shot payload. Use `logical_key` for cross-referencing. Returns `identity_mapping` & `preview_id`.
- `soll_commit_revision(preview_id)`: Commits batch. Returns `revision_id`.
- `soll_rollback_revision(revision_id)`: Reverts.

**Audit & Traceability:**
- `axon_pre_flight_check(diff_paths)`: **MANDATORY** dry-run validation before commit.
- `axon_commit_work(diff_paths, message, dry_run?)`: validated commit workflow; use this instead of raw `git commit` in normal operator flow.
- `soll_validate(project_code)`: Audits graph invariants.
- `soll_verify_requirements()`: Computes requirement coverage.
- `soll_export(project_code)`: Generates canonical Markdown backup.
- `restore_soll(path)`: Replays a Markdown snapshot.

**Context & Discovery:**
- `status`: Aggregated runtime/operator truth across current runtime profile.
- `project_status`: Live project situation built from runtime truth, anomalies, and SOLL source context.
- `snapshot_history` / `snapshot_diff`: Derived structural memory outside `SOLL`.
- `conception_view`: Read-only derived conception map (modules, interfaces, contracts, flows, suspected boundaries).
- `change_safety`: Derived safety summary for a target based on coverage, traceability, and validation signals.
- `why`: Rationale-oriented explanation over traceability + SOLL + evidence retrieval.
- `path`: Topology/path explanation between source and sink or around an anchor.
- `anomalies`: Aggregated structural anomalies with severity/confidence/action.
- `mcp_axon_query` / `mcp_axon_inspect`: Read IST (Code).
- `retrieve_context(question, project?, token_budget?)`: Planner-driven evidence packet retrieval. Use when the agent needs compact answerable context rather than flat search hits.
- `axon_impact(symbol)`: **CRITICAL for refactoring.** Traces a code symbol's impact all the way up through the call graph and across the bridge to SOLL (revealing compromised Decisions, Requirements, and Visions).
- `axon_architectural_drift`: **CRITICAL for domain leakage.** Recursively traces pathfinding between layers (e.g., from `domain` to `infrastructure`) to explain why a boundary violation was detected.
- `soll_query_context`: Read SOLL state. Supports native filtering by `status` (e.g. 'proposed') and `type` (e.g. 'Requirement').
- `soll_work_plan(project_code, json=true)`: Ordered execution view (blockers, waves).

**Error Handling & AX (Agent Experience):**
- MCP tools use Fail-Fast with Context. If you pass an invalid `project_code`, the server returns the expected state and valid project codes.
- Mutation tools (`soll_apply_plan`) are strictly idempotent (Upsert). Re-running a plan safely returns "No changes" instead of crashing on SQL constraint errors.
- Some advanced tools are runtime-profile dependent. Treat `status` as the first truth surface for availability/degradation.

## IST-Driven Execution (Mandatory Workflow)
Before coding or mutating SOLL, you MUST:
1. **Status first:** Run `status` to learn runtime profile, degradation, and advanced surface availability.
2. **Query IST:** Run `mcp_axon_query` or `mcp_axon_inspect` on the target component. If refactoring, run `axon_impact` to assess strategic blast radius.
3. **Check rationale/topology when needed:** Run `why`, `path`, or `anomalies` depending on the task shape.
4. **Verify Drift:** Compare IST with SOLL plan. Formulate the gap.
5. **Plan & Execute:** Apply TDD. Write code.
6. **Wire (Traceability):** Link new/modified IST symbols to SOLL origins (`SUBSTANTIATES` or `IMPACTS`) using `soll_manager link`.
   - `SUBSTANTIATES`: SOLL (concept/req/dec) ↔ IST symbol.
   - *Rule:* If code changes and intention changes, update SOLL in the same wave. Implementation is incomplete until mathematically linked to SOLL.
7. **Pre-flight before commit:** Run `axon_pre_flight_check`.
8. **Validated commit:** Use `axon_commit_work`, not raw commit flow, unless explicitly operating outside the MCP workflow for a justified reason.

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
- The runtime profile / public tool discoverability contract changes.
