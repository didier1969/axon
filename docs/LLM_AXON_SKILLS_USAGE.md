# LLM + Axon Skills: Operational Usage (Codex and Gemini)

## Objective
Provide deterministic skill resolution and Axon-first operational behavior for SOLL workflows.

## Single Source of Truth
- Skills source: `/home/dstadel/.claude/skills`
- Gemini skills path: `/home/dstadel/.gemini/skills` (symlink to source)
- Codex skills bridge: `/home/dstadel/.codex/skills/<skill>` symlinks to source

## Enforced Policy
- Do not infer workflows by scanning repository root.
- Resolve workflows from skills only.
- If Axon MCP is available, use `axon-soll-operator` before SOLL operations.
- Prefer MCP tools for SOLL operations; use shell only for bootstrap/verification.
- SOLL identity is server-owned: `create` returns canonical IDs in the form `TYPE-CODE-NNN`.
- `CODE` is resolved by Axon from the canonical project declaration in `.axon/meta.json`, not chosen by the client.
- `project_slug` must match the canonical slug declared in `.axon/meta.json`; aliases are rejected.
- All later SOLL mutations must use canonical IDs exactly, like primary keys.
- SOLL relations are also server-governed: the LLM may propose `relation_type`, but Axon validates the source/target pair, applies a canonical default when available, or rejects the link.

## Verification Commands
```bash
readlink -f /home/dstadel/.gemini/skills
readlink -f /home/dstadel/.codex/skills/axon-soll-operator
test -f /home/dstadel/.codex/skills/axon-soll-operator/SKILL.md && echo "codex-skill-ok"
grep -n "Axon Skills Resolution Policy" /home/dstadel/.gemini/GEMINI.md
```

Expected:
- Gemini skills resolves to `/home/dstadel/.claude/skills`
- Codex `axon-soll-operator` resolves to `/home/dstadel/.claude/skills/axon-soll-operator`
- Policy section exists in `GEMINI.md`

## Runtime Notes
- Start a new Codex/Gemini session after skill/path updates.
- Existing sessions may keep old context and not pick up new policies immediately.
- Runtime modes now include `graph_only` for watcher + graph indexing without background semantic/vector workers.
- Use `./scripts/axon resume-vectorization` to recreate missing chunk vectorization backlog explicitly.

## Recommended SOLL Entry Path
1. Check MCP availability (`health`, then `validate_soll` if SOLL scope matters).
2. Load `axon-soll-operator` workflow.
3. Execute SOLL workflow via MCP tools (unit or bulk path), not direct DB mutations.
4. Verify with SOLL validation tools before reporting completion.

## Public MCP Surface
- Public `tools/list` is intentionally reduced to the canonical operator surface.
- Hidden-by-default expert/internal tools remain callable when explicitly named, but they are no longer part of the normal client/LLM discovery path.
- Preferred public families:
  - DX: `query`, `inspect`, `impact`, `health`, `audit`, `fs_read`
  - SOLL read: `validate_soll`, `soll_query_context`, `soll_verify_requirements`, `soll_work_plan`, `export_soll`, `restore_soll`
  - SOLL write: `soll_manager`, `soll_apply_plan`, `soll_commit_revision`, `soll_rollback_revision`, `soll_attach_evidence`

## SOLL Identity and Scope
- Canonical examples: `VIS-AXO-001`, `DEC-BKS-001`, `STK-AXO-003`.
- Non-canonical example: `DEC-BookingSystem-001`.
- Use `validate_soll --project_slug <slug>` when the goal is project-scoped invariants rather than workspace-global triage.
- Use `export_soll --project_slug <slug>` when the goal is a project snapshot rather than a mixed workspace export.
- `validate_soll` now also catches dangling relation endpoints and relation-policy violations.

## Read-Only Planning Path
- Use `soll_work_plan` when the goal is to derive an ordered execution plan from `SOLL` without mutating the graph.
- Preferred CLI wrapper: `./scripts/axon work-plan --project AXO [--limit N] [--top N] [--json] [--no-ist]`
- V1 rule: scheduling edges come from `SOLL` only; `IST` is scoring and risk signal only.
- Use `top_recommendations` when the goal is immediate operator action rather than full graph review.

## Runtime Robustness Path
- Use `./scripts/axon qualify` as the default unified entry point for runtime qualification, demos, and regression checks.
- Preferred quick run: `./scripts/axon qualify --profile demo --mode graph_only`
- Preferred comparison run: `./scripts/axon qualify --profile smoke --compare mcp_only,graph_only,full`
- Use `--profile full` when ingestion qualification must be included in the same consolidated report.
- Use `./scripts/axon robustness-mcp` when the goal is to qualify MCP responsiveness and recovery under load.
- Preferred comparative run: `./scripts/axon robustness-mcp --modes mcp_only,full --duration 60 --concurrency 4`
- Read the output as a diagnostic, not a hard release gate: compare `responsive`, `success`, `p95`, `timeout`, and `recovered_without_restart`.
- Primary product question: does `full` degrade materially versus `mcp_only`, suggesting pressure from indexing or ingestion rather than a generic MCP failure.
