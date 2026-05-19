# Session 45 — Expert review round 1 (verbatim)

**Date** : 2026-05-18
**Operator** : Didier
**Expert agent** : `a048b39f950f9d365` (general-purpose, senior agent skills/MCP/multi-agent specialist)
**Brief sent by Claude** : design review of proposed SOLL entity types SKI + PRT + GUI in PRO namespace, MCP-only canonical methodology, bootstrap touchpoint preserved.

**Canonical SOLL anchors** :
- VAL node : VERIFIES REQ-AXO-91576 (umbrella)
- Linked from MIL-AXO-024 milestone "Validation expert" section
- Linked from PIL-AXO-9003 (Two-Sided Identity)

---

## TL;DR verdicts

| Q | Topic | Verdict |
|---|---|---|
| Q1 | Drop auto-detect, use MCP-mediated explicit invocation | **Works as proposed, with caveats** — but you're not "dropping" the detection layer, you're *moving* it. Make peace with that. |
| Q2 | SKI entity schema | **Needs revision** — proposal is sound; 6 fields added (invocation_mode, output_contract, evaluation_refs, failure_modes, llm_compatibility, fully-qualified tool_dependencies). |
| Q3 | PRT entity schema | **Needs revision** — `{{placeholder}}` markdown is the right primary surface, but you need a typed parameter spec or you'll regret it within 6 months. |
| Q4 | LLM autonomy + memory refresh | **Works as proposed but underspecified** — the canonical pattern (re-anchoring) exists in literature, you just haven't named it. Implement it explicitly. |
| Q5 | Blind spots | **Three serious risks** flagged: bootstrap problem, MCP latency tax, cross-LLM tool-name fragmentation. |
| Q6 | External validation | **Mixed** — Anthropic's own MCP spec validates you (prompts user-controlled, tools model-controlled). But you diverge from the now-de-facto-standard SKILL.md format adopted by OpenAI/Google/GitHub/Cursor in Dec 2025. That has commercial implications. |

## Q1 — Will explicit MCP invocation work?

**Verdict: works as proposed, with caveats.**

You are not "dropping the detection layer." You are *relocating* it from `(LLM scans frontmatter)` to `(LLM scans SOLL nodes via MCP)`. The probabilistic heuristic doesn't disappear — it just runs against richer, deterministic anchors. That's a real improvement but not a categorical one.

**Why this can work** :
- MCP spec aligns with your model. Prompts are user-controlled, tools are model-controlled, resources are application-controlled. Your `prompt_template_get` is doing what MCP prompts were designed for. Your `skill_invoke` is correctly a tool. **This is one of the cleanest validations of your design.**
- The "phase transition" research validates moving away from flat frontmatter scan. The arxiv 2601.04748 paper shows skill selection accuracy collapses non-linearly above ~50 skills, with γ exponent 1.65–1.71 — flat semantic matching against descriptions degrades sharply at scale. Hierarchical routing via SOLL graph edges (`MANDATES`, `REFINES`, `COMPOSES_WITH`) recovers 37–40% absolute accuracy at scale. You're inadvertently implementing hierarchical skill routing. Good.
- Methodology nodes naming skills explicitly = grounded routing.

**Known failure modes** :
1. **MCP roundtrip tax.** Each `skill_invoke` is at minimum one MCP roundtrip (5–50ms locally, 100–500ms with auth/network). Compared to filesystem read, you're adding latency. *(Note: corrected in round 2 — RAM-first SOLL changes this calculus to 1-3ms localhost.)*
2. **Model-decision-to-invoke-tool is also probabilistic.** Even tool calling involves model judgment. The LLM still has to *decide* to call `skill_invoke("handoff")`. The determinism comes from the *contract enforcement layer*.
3. **You lose the SKILL.md ecosystem.** Anthropic Dec 2025 standard adopted by OpenAI/Google/GitHub/Cursor.

## Q2 — SKI entity schema (revision recommended)

**6 critical fields missing from initial proposal** :

1. `invocation_mode` (MANDATED|RECOMMENDED|OPTIONAL) — without it, every skill = optional, deterministic methodology promise dies
2. `output_contract` — Anthropic explicitly: "examples pattern" and "template pattern" critical for output quality
3. `evaluation_refs` — Anthropic: "build evaluations BEFORE writing documentation". Each SKI must link to VAL proving it works
4. `failure_modes` — when LLM self-anchors, knowing how a skill misfires is more valuable than knowing how it succeeds
5. `llm_compatibility` matrix — without it, "LLM-agnostic" is marketing-ware, not verified property
6. Fully-qualified `tool_dependencies` — Anthropic: "Without server prefix, Claude may fail to locate the tool"

## Q3 — PRT entity schema (revision recommended)

**Mustache + typed parameter sidecar** :
- Use Mustache (logic-less, cross-language, no XSS surface) — not Jinja2
- Typed parameter sidecar mandatory (without it, LLM stringifies arbitrary objects, drops keys, misformats dates)
- Markdown body with `{{placeholder}}` substitution
- `expected_output` schema, `golden_examples`, `target_llms` matrix

## Q4 — LLM autonomy + memory refresh

**Verdict: works as proposed but underspecified.**

Pattern = re-anchoring (Reflexion-with-external-memory, Shinn et al. 2023). Detect degradation → externalize state → re-query canonical → discard stale tokens.

**Critical addition** : 4th MCP tool `re_anchor(reason)` returning canonical "where am I" packet (active methodology, mandated skills for phase, recent revisions, session_pointer, soll_work_plan top). Single-call re-orientation. Load-bearing.

## Q5 — Blind spots and risks

### Risk 1 (HIGH) — Bootstrap problem
A fresh LLM doesn't know your MCP tools exist. Must keep minimal filesystem touchpoint (CLAUDE.md / AGENTS.md / .gemini/system.md) saying "this project uses Axon MCP, first action `mcp__axon__status()`".

### Risk 2 (HIGH → LOW after round 2 correction)
MCP latency tax. *Corrected in round 2 to LOW after RAM-first SOLL clarification.*

### Risk 3 (MEDIUM) — Cross-LLM fragmentation
Claude/Codex/Gemini behave differently. "LLM-agnostic" must be verified, not asserted. Nightly evals per (SKI × LLM) pair mandatory.

### Risk 4 (MEDIUM) — Standard ecosystem walk-away
Dec 2025 Anthropic standard. Materialization tool as escape hatch insurance. *(Downgraded to optional in round 2.)*

### Risk 5 (LOW-MED) — Concept/Guideline naming the skill is brittle
Use stable IDs only, never names, in cross-references.

### Risk 6 (LOW) — Prompt injection via SOLL
SOLL write permissions per namespace + signed revisions + allowlist for high-trust scenarios.

## Q6 — External validation

### Strong validation
- MCP spec primitives map cleanly to your tools
- Hierarchical routing beats flat selection at scale (arxiv 2601.04748)
- Context engineering favors external memory + re-anchoring (Anthropic effective context engineering)
- Structured handoffs over compaction (Anthropic three-agent harness)

### Tensions
- You diverge from the Skills open standard (Anthropic Dec 2025)
- Anthropic's authoring guide silent on auto-detection failure modes at scale
- Progressive disclosure is filesystem-coupled in Anthropic's framing

### Patterns aligned but unnamed
- Reflection (Shinn et al., Reflexion 2023) — your `re_anchor` is reflexion-with-external-memory
- Plan-and-execute (LangGraph, ReWOO) — your methodology nodes are the plan, skills execute it
- BDI (Belief-Desire-Intention) — Pillar/Vision = desires; methodology nodes = beliefs; skills = intentions

## Top 10 recommendations (priority order)

1. **Keep bootstrap filesystem touchpoint.** Non-negotiable.
2. **Add `re_anchor` to tool set.** Drift prevention via explicit re-anchoring.
3. **Add `invocation_mode`, `output_contract`, `evaluation_refs`, `failure_modes`, `llm_compatibility` to SKI.** Critical fields.
4. **Adopt typed parameter spec for PRT (Mustache + sidecar).**
5. **Mandatory client-side caching + batch tool + eager manifest.** *(Reduced priority in round 2 after RAM-first correction.)*
6. **Match Anthropic SKILL.md constraints in SKI fields** (64-char names, 1024-char descriptions, gerund convention, third-person). Cheap interop insurance.
7. **Build a materialization tool** (`axon_skill_materialize --target X`) as escape hatch. *(Deferred to optional in round 2.)*
8. **Verify, don't assert, cross-LLM compatibility.** Nightly evals per (SKI × LLM) pair.
9. **Enforce contract layer.** Block commits/handoffs if mandated skill not invoked.
10. **Read the prior art** (Reflexion, LangGraph checkpoints, Anthropic three-agent harness).

## Closing assessment round 1

> "The design is not flawed. It is *underspecified* in three critical places (bootstrap, latency, output contracts), uses correct primitives but borrows them without explicitly naming the lineage, and trades ecosystem standardization for codified determinism without a stated mitigation. Fix the underspecification; document the trade-off consciously; ship it."

## Sources cited
- Anthropic Engineering — Equipping agents with Skills
- Anthropic Skill authoring best practices
- MCP Tools spec / MCP Prompts concepts
- Anthropic Effective Context Engineering / Multi-agent research system / Three-agent harness (InfoQ April 2026)
- arxiv 2601.04748 — Skill libraries phase transition
- arxiv 2601.17549 — MCP prompt injection
- Reflexion (Shinn et al., 2023)
