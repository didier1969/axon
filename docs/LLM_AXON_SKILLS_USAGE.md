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

## Recommended SOLL Entry Path
1. Check MCP availability (`axon_health`, `axon_debug`).
2. Load `axon-soll-operator` workflow.
3. Execute SOLL workflow via MCP tools (unit or bulk path), not direct DB mutations.
4. Verify with SOLL validation tools before reporting completion.

## Read-Only Planning Path
- Use `soll_work_plan` when the goal is to derive an ordered execution plan from `SOLL` without mutating the graph.
- Preferred CLI wrapper: `./scripts/axon work-plan --project AXO [--limit N] [--top N] [--json] [--no-ist]`
- V1 rule: scheduling edges come from `SOLL` only; `IST` is scoring and risk signal only.
- Use `top_recommendations` when the goal is immediate operator action rather than full graph review.

## Runtime Robustness Path
- Use `./scripts/axon robustness-mcp` when the goal is to qualify MCP responsiveness and recovery under load.
- Preferred comparative run: `./scripts/axon robustness-mcp --modes mcp_only,full --duration 60 --concurrency 4`
- Read the output as a diagnostic, not a hard release gate: compare `responsive`, `success`, `p95`, `timeout`, and `recovered_without_restart`.
- Primary product question: does `full` degrade materially versus `mcp_only`, suggesting pressure from indexing or ingestion rather than a generic MCP failure.
