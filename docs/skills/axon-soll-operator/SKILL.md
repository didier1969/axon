---
name: axon-soll-operator
description: Use when operating Axon SOLL end-to-end (unit edits, bulk imports, verification, rollback) and when an MCP client needs deterministic SOLL workflows with minimal risk.
---

# Axon SOLL Operator

## Overview

This skill defines the canonical way to operate SOLL with Axon MCP:
- safe unit edits
- deterministic batch import
- explicit verification
- reversible changes

Core rule: default to read/verify first, mutate second, certify last.

Identity rule:
- Every SOLL entity is server-identified.
- Canonical IDs follow `TYPE-CODE-NNN`.
- `CODE` comes from the canonical project declaration in `.axon/meta.json`.
- The client/LLM never chooses the final ID or numeric suffix.
- `create` returns the canonical ID; every later `update`/`link` must reuse that ID.
- Batch workflow IDs are also server-owned and canonical:
  - `preview_id` uses `PRV-CODE-NNN`
  - `revision_id` uses `REV-CODE-NNN`
- `project_slug` must match the canonical slug declared in `.axon/meta.json`; aliases are rejected.

## SOLL Semantic Contract (Critical)

Use the following meanings strictly:

- `Vision`: the fundamental "why" of the project; human/system value and intended outcome.
- `Pillar`: a durable strategic principle that protects the vision.
- `Concept`: a stable domain concept used as shared vocabulary.
- `Requirement`: a testable capability the system must provide.
- `Decision`: an explicit architectural/product choice with rationale.
- `Milestone`: a time-bounded delivery checkpoint.
- `Stakeholder`: an actor impacted by value, risk, or constraints.
- `Validation`: objective proof that requirements are satisfied.
- `Evidence/Traceability`: artifacts linking intent to implementation and outcomes.

Hard boundary:
- `Vision` must never be implementation-specific.
- If a statement depends on a concrete stack/framework/protocol, it is not `Vision`; it belongs to `Decision` or lower.

Illustrative examples (examples only, not templates):
- `Vision` example: "Enable users to complete reservations with minimal cognitive load and high trust."
- Not a `Vision` example: "Build a Phoenix + Rust + DuckDB booking platform."

## Recommended Information Architecture (Complex Projects)

Use these ranges to keep SOLL readable and governable:

- `Vision`: exactly `1` per project.
- `Pillars`: `3..7`.
- `Concepts`: `8..20` stable concepts.
- `Requirements` total: `15..60`, structured in two levels:
  - Level-1 capability requirements: `8..15`.
  - Level-2 sub-requirements per parent: `2..5`.
- `Decisions`: `1..3` major decisions per Level-1 requirement.
- `Milestones` active horizon: `5..12`.

Depth policy:
- recommended max depth: `2`.
- acceptable exceptional depth: `3`.
- avoid deeper trees; they reduce operator comprehension and traceability quality.

Illustrative decomposition (examples only):
- Parent requirement: "Reservation flow is reliable under peak load."
- Child requirements:
  - "P95 checkout completion < 2.5s at target concurrency."
  - "No double-booking under concurrent writes."
  - "User receives deterministic failure reasons on capacity conflicts."

Quality constraints:
- Each requirement should have: owner, acceptance criteria, and at least one evidence artifact.
- Each decision should solve at least one explicit requirement.
- If a requirement cannot be traced to a vision/pillar in <= 2 graph hops, restructure.

## When to Use

Use this skill when:
- you need to create/update/link SOLL entities
- you need bulk ingestion from markdown/json/ndjson/yaml
- you need to attach evidence and compute requirement coverage
- you need revision commit/rollback safety

Do not use this skill for IST indexing operations (`reindex-project`, ingestion runtime), except for trace links (`SUBSTANTIATES`, `IMPACTS`) between SOLL intent and IST artifacts.

## Canonical Tooling Surface

MCP tools:
- `soll_validate`
- `soll_query_context`
- `soll_work_plan`
- `soll_manager`
- `soll_apply_plan`
- `soll_commit_revision`
- `soll_rollback_revision`
- `soll_attach_evidence`
- `soll_verify_requirements`
- `soll_export`
- `restore_soll`

Identity-sensitive arguments:
- `axon_init_project`: creates project in registry and suggests global guidelines.
- `axon_apply_guidelines`: applies selected global rule IDs (`GUI-PRO-XXX`) to the local project (`GUI-SLUG-XXX`) via inheritance.
- `axon_commit_work`: MUST be used to validate/commit work; checks modified paths against triggered guidelines. On success, it automatically generates a canonical Markdown export (Doc-as-Code backup) and executes the git commit.
- `soll_manager create`: send `project_slug` plus business fields; the server returns `TYPE-CODE-NNN`.
- `soll_manager update`: `id` is mandatory and must already be canonical.
- `soll_manager link`: `source_id` and `target_id` must already exist; the server validates the pair of types and accepts, rejects, or defaults the relation.
- `soll_apply_plan`: send canonical `project_slug`; the server prepares a revision preview and returns canonical `preview_id` (`PRV-CODE-NNN`).
- `soll_commit_revision`: consumes canonical `preview_id` and returns canonical `revision_id` (`REV-CODE-NNN`).
- `soll_validate(project_slug=...)`: validates only one project when requested.
- `soll_export(project_slug=...)`: exports only one project when requested.

Relation policy:
- The client/LLM may propose `relation_type`, but Axon is the final authority.
- If no `relation_type` is provided, Axon applies the canonical default when one exists.
- If the proposed relation is not allowed for the source/target pair, Axon rejects it and returns the allowed relations.
- Links are created only when both endpoints exist.
- `soll_validate` also flags dangling or policy-invalid relations.

CLI wrappers:
- `./scripts/axon soll-import --input <file> --format md|json|ndjson|yaml [--dry-run] [--strict]`
- `./scripts/axon work-plan --project <slug> [--limit N] [--top N] [--json] [--no-ist]`

## Operational Workflows

### 1) Safe unit workflow (recommended default)

1. `soll_validate`
2. `soll_query_context` (project scope)
3. targeted `soll_manager` (`create`/`update`/`link`)
   Creation returns server-owned IDs; keep them for all later mutations.
4. `soll_validate`
5. optional `soll_export`

Use this for:
- fixing orphan links
- small edits to requirement/decision/milestone/concept/vision
- explicit trace links to IST symbols

### 1b) Read-only planning workflow

Use when an operator or MCP client needs an ordered execution view from SOLL without mutating the graph.

1. `soll_validate`
2. `soll_query_context` (optional, project scope sanity)
3. `soll_work_plan`
4. review `blockers`, `cycles`, `ordered_waves`, `validation_gates`
5. only then choose a mutation path (`soll_manager` or `soll_apply_plan`) if changes are needed

Contract notes:
- V1 derives scheduling edges from `SOLL` only.
- `IST` contributes scoring/risk signals only, never precedence edges.
- `format=json` is the machine-consumable source of truth.
- `top_recommendations` is the operator-facing shortlist for immediate action.

### 2) Batch workflow (plan-driven)

Use when changes span many entities.

1. `soll_apply_plan` with `dry_run=true`
2. review returned `preview_id` and operations
3. `soll_commit_revision` with `preview_id`
4. `soll_verify_requirements`
5. optional `soll_export`

If needed:
- `soll_rollback_revision` on the committed revision.

### 3) Snapshot restore workflow

Use only for canonical replay from reviewed markdown snapshots.

1. `restore_soll` with a reviewed `SOLL_EXPORT_*.md`
2. `soll_validate`
3. targeted repairs via `soll_manager link`/`update`
4. `soll_export`

## Bulk Ingestion (CLI)

### Markdown restore (full replay)

```bash
./scripts/axon soll-import --input docs/vision/SOLL_EXPORT_YYYY-MM-DD_HHMMSS_xxx.md --format md
```

### Read-only work plan

```bash
./scripts/axon work-plan --project AXO
./scripts/axon work-plan --project AXO --limit 20 --top 5 --json
./scripts/axon work-plan --project AXO --no-ist
```

### Structured payload import

```bash
./scripts/axon soll-import --input /tmp/soll_payload.json --format json --project AXO --author codex --dry-run
./scripts/axon soll-import --input /tmp/soll_payload.ndjson --format ndjson --strict
./scripts/axon soll-import --input /tmp/soll_payload.yaml --format yaml
```

## Structured Payload Contract (json/yaml)

Server-owned identity contract:
- do not send final entity IDs for `create`
- do send canonical IDs for `update`, `link`, evidence attachment, and any rollback target
- do treat `preview_id` and `revision_id` as server-owned primary keys too
- treat SOLL IDs exactly like database primary keys

Top-level keys supported:
- `plan` (for `soll_apply_plan`): `pillars`, `requirements`, `decisions`, `milestones`
- `visions`
- `concepts`
- `stakeholders`
- `validations`
- `relations` (`source_id`, `target_id`, optional `relation_type`)
- `evidence` (`entity_type`, `entity_id`, `artifacts[]`)

Minimal example:

```json
{
  "plan": {
    "requirements": [
      {
        "logical_key": "req-runtime-authority",
        "title": "Canonical Runtime Authority",
        "description": "Writer path is canonical truth",
        "priority": "P1",
        "status": "current"
      }
    ],
    "decisions": [
      {
        "logical_key": "dec-rust-writer",
        "title": "Rust Writer Engine",
        "description": "Use Rust for ingestion"
      }
    ]
  },
  "relations": [
    {
      "source_id": "dec-rust-writer",
      "target_id": "req-runtime-authority",
      "relation_type": "SOLVES"
    }
  ],
  "evidence": [
    {
      "entity_type": "requirement",
      "entity_id": "req-runtime-authority",
      "artifacts": [
        {
          "artifact_type": "metric",
          "artifact_ref": "quality-mcp:pass",
          "confidence": 0.95
        }
      ]
    }
  ]
}
```

*Note on Identifiers & Relations*: The `soll_apply_plan` endpoint supports Zero-Shot Payload resolution. You can define new entities with a `logical_key` (e.g. `"req-runtime-authority"`) and immediately use that exact same `logical_key` as `source_id` or `target_id` inside the `"relations"` array (or `"entity_id"` in `"evidence"`). The server resolves these into canonical IDs (e.g. `REQ-AXO-001`) atomically. 

On success, `soll_apply_plan` returns an `identity_mapping` dictionary inside the `data` field bridging your `logical_key` inputs to the generated Canonical IDs (e.g. `{"req-runtime-authority": "REQ-AXO-001"}`).

## SOLL/IST Coherence Guidance

Use links to keep intent and runtime aligned:
- `SUBSTANTIATES`: SOLL concept/requirement ↔ IST symbol
- `IMPACTS`: decision/requirement impact scope to runtime entities

Practical rule:
- if code changed and intention changed, update SOLL in the same delivery wave.

## Guardrails

- Never auto-delete orphan SOLL entities silently.
- Use `dry-run` before high-volume changes.
- Prefer explicit `relation_type` for links in batch mode when the pair allows more than one canonical relation.
- Do not treat `relation_type` as free text; it is a server-validated proposal.
- Prefer `soll_apply_plan` + revision commit over ad-hoc multi-step writes.
- Never fabricate IDs from raw slugs like `DEC-BookingSystem-001`; expected canonical form is `DEC-BKS-001`.

## Fast Triage

- `soll_validate` reports violations:
  - inspect with `soll_query_context`
  - repair with targeted `soll_manager link/update`
  - verify again

- Batch fails midway:
  - if revision committed, use `soll_rollback_revision`
  - fix payload and rerun with `--strict`

## Skill Maintenance Policy

Update this skill when at least one of these conditions occurs:

1. Tool surface changed:
- MCP SOLL tools added/removed/renamed.
- Required/optional arguments changed.
- New CLI wrapper behavior (`./scripts/axon soll-import`, `./scripts/axon work-plan`) changed.

2. Data contract changed:
- entity schema changed (`Requirement`, `Decision`, `Validation`, etc.).
- relation taxonomy changed (`SOLVES`, `VERIFIES`, `SUBSTANTIATES`, ...).
- revision/rollback semantics changed.

3. Governance policy changed:
- meaning of SOLL entities changed.
- acceptance policy changed (what counts as done/partial/missing).
- risk policy changed (read-only vs auto-remediation, rollback expectations).

4. Operational evidence indicates drift:
- repeated operator errors due to ambiguous guidance.
- repeated client reports caused by missing runbook steps.
- quality gate failures linked to outdated instructions.

### Cadence and SLA

- Mandatory review: every 2 weeks.
- Immediate update (same day): for any breaking tool/schema change.
- Update within 48h: for non-breaking but high-impact operational changes.
- Release gate: no production release with SOLL surface changes unless this skill is updated in the same wave.

## SOLL Bootstrap Methodology (From Zero)

Use this sequence when a project starts with no reliable SOLL:

### Phase 0: Scope and boundaries

1. Define project slug and ownership.
2. Confirm IST/SOLL separation and intended traceability targets.
3. Capture source material: strategy docs, product goals, regulatory constraints, architecture context.

### Phase 1: Intent first (non-technical)

1. Create exactly one `Vision`:
- state user/system value and intended impact.
- avoid implementation details.
2. Define `Pillars` (`3..7`) as durable strategic constraints.
3. Define `Concepts` (`8..20`) as stable domain vocabulary.

### Phase 2: Capability model

1. Create Level-1 `Requirements` (`8..15`) as capabilities.
2. Decompose into Level-2 sub-requirements (`2..5` per parent).
3. For each requirement, set:
- owner
- priority
- acceptance criteria
- measurable success condition

### Phase 3: Decision model

1. Add `Decisions` tied to explicit requirements.
2. Keep rationale explicit (tradeoffs, constraints, alternatives).
3. Use links so every major decision can be traced to solved requirements.

### Phase 4: Execution scaffolding

1. Add active `Milestones` (`5..12`) for near-term horizon.
2. Add `Stakeholders` where governance/risk requires accountability.
3. Add first `Validation` nodes and initial evidence anchors.

### Phase 5: Traceability wiring

1. Link intent graph internally (`BELONGS_TO`, `SOLVES`, `VERIFIES`, etc.).
2. Link to IST where available (`SUBSTANTIATES`, `IMPACTS`).
3. Run `soll_verify_requirements` to classify done/partial/missing.

### Phase 6: Certification loop

1. `soll_validate`
2. targeted repairs (`soll_manager`)
3. `soll_validate` again
4. `soll_export` snapshot

### Recommended bootstrap execution mode

- First pass: `dry-run` for all batch operations.
- Second pass: commit a reviewed revision (`soll_apply_plan` + `soll_commit_revision`).
- Keep rollback path ready (`soll_rollback_revision`) during the whole bootstrap window.

Illustrative minimal bootstrap target (examples only):
- Vision: `1`
- Pillars: `4`
- Concepts: `12`
- Requirements: `10` L1 + `25` L2
- Decisions: `20`
- Milestones: `6`
- Initial validations: `5`
