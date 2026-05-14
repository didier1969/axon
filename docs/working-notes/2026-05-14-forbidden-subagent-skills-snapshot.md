# Forbidden sub-agent skills in Axon — snapshot 2026-05-14

**Reference :** GUI-PRO-027 (token-economy sub-agent policy)
**Context :** Audit mercenaire 2026-05-14 MACRO-9 P1 confirmé. 8 skills sub-agent-first installés dans `~/.claude/skills/` sans garde "DO NOT use in Axon repo".

## Scan

```bash
$ ls ~/.claude/skills/ | grep -E '(idea-to-delivery|driven-development|parallel-agents|multi-agent|executing-plans)'
```

## 8 skills identifiés (forbidden in `project_code=AXO`)

| Skill | Description (line 3 frontmatter) | Forbidden because |
|---|---|---|
| `concept-to-delivery` | Compatibility alias for `idea-to-delivery` | dispatches sub-agents for plan/exec/review cycle |
| `consensus-driven-delivery` | Compatibility alias for `idea-to-delivery` | idem |
| `dispatching-parallel-agents` | Use when facing 2+ independent tasks without shared state | spawns parallel sub-agents → no MCP |
| `executing-plans` | Use when you have a written implementation plan to execute in a separate session | spawns executor sub-agent |
| `feature-delivery` | Compatibility alias for `idea-to-delivery` | dispatches sub-agents |
| `idea-to-delivery` | Use when an idea/feature/concept/migration needs end-to-end delivery from concept shaping to planning to execution, with independent expert review, subagents | primary trigger — full sub-agent pipeline |
| `multi-agent-patterns` | When asked to "design multi-agent system", "implement supervisor pattern", "create swarm architecture", "coordinate multiple agents" | by design dispatches multiple sub-agents |
| `subagent-driven-development` | Use when executing implementation plans with independent tasks in the current session | spawns task sub-agents |

## Why forbidden in Axon

Coût documenté : **100-200K tokens par invocation accidentelle** (sub-agent re-reads source files to reconstruct IST instead of using MCP `query`/`inspect`/`retrieve_context`). MEMORY.md `feedback_no_subagents_for_code_in_axon` rule confirmé.

## Canonical Axon path (replaces sub-agent dispatch)

- **Main thread MCP-first** : `query` (~5-50 tokens) > `inspect` > `retrieve_context` > `impact` > `anomalies` > `architectural_drift`
- **Sub-agent CLI bridge** (when sub-agent must call MCP) : `./scripts/axon --instance live mcp-call call <tool> --args-file <file.json>`
- **Allowed sub-agent uses** : shell exec (`cargo build/test`), doc writing (no source reading), MCP-independent tasks, external research with `WebSearch`

## Refresh policy

Liste à re-vérifier tous les 30 jours OU à chaque ajout/retrait dans `~/.claude/skills/`. Detection automatique via `/curate-soll` pass_D (futur — extension propre).

## Originator

claude-code session 31, Wave 2 MACRO-9 enforcement, post-audit-2026-05-14. Attached as evidence to GUI-PRO-027.
