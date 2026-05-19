# Skills Compaction Audit — 2026-05-04

Author: token-efficiency-specialist (Claude Opus 4.7, 1M context)
Status: AUDIT ONLY — no SKILL.md edited. User approval required before Phase 1 execution.

## 1. Inventory

- **595** SKILL.md files exist on disk under `~/.claude` and `~/projects` combined.
- **119** are actually loaded into the Claude Code skills system-reminder for this session (the rest live in nested project repos that are not active in the harness).
- Total injected-description size: **33,026 chars (~8,256 tokens)** every cold-start.

Breakdown of the 119 loaded skills by physical location (after symlink resolution):

| Bucket | Count | Description chars | Editability |
|---|---|---|---|
| `~/.claude/skills/<name>/SKILL.md` (real files) | 55 | 16,427 | **EDITABLE** |
| `~/antigravity-skills/skills/<name>/SKILL.md` (symlinked from `~/.claude/skills/<name>`) | ~45 | ~14,000 | **VENDORED** (upstream git repo — edits wiped on `git pull`) |
| `~/.claude/plugins/marketplaces/.../skills/<name>/SKILL.md` | ~13 | ~2,600 | **VENDORED** (plugin update wipes edits) |
| Missing / unparsed (probably namespaced like `carl:tasks:*`, `paul:*`, `gsd:*`) | 6 | 0 | n/a |

Important finding: **most "popular" Anthropic skills (`xlsx`, `docx`, `pdf`, `pptx`, `claude-api`, `memory-systems`, `frontend-design`, `ui-ux-pro-max`, etc.) are symlinks into `~/antigravity-skills`** — touching them in-place will be reverted on next upstream pull. They must be treated as VENDORED.

## 2. Top 30 offenders by description length

| skill | chars | location (resolved) | class |
|---|---|---|---|
| xlsx | 943 | `~/antigravity-skills/skills/xlsx/SKILL.md` | VENDORED |
| seo | 870 | `~/.claude/skills/seo/SKILL.md` | EDITABLE |
| docx | 799 | `~/antigravity-skills/skills/docx/SKILL.md` | VENDORED |
| ads | 752 | `~/.claude/skills/ads/SKILL.md` | EDITABLE |
| claude-api | 744 | `~/.claude/plugins/marketplaces/anthropic-agent-skills/skills/claude-api/SKILL.md` | VENDORED |
| ui-ux-pro-max | 732 | `~/antigravity-skills/skills/ui-ux-pro-max/SKILL.md` | VENDORED |
| pptx | 694 | `~/antigravity-skills/skills/pptx/SKILL.md` | VENDORED |
| reality-first-stabilization | 684 | `~/.claude/skills/reality-first-stabilization/SKILL.md` | EDITABLE |
| dev-cycle | 544 | `~/.claude/skills/dev-cycle/SKILL.md` | EDITABLE |
| memory-systems | 507 | `~/antigravity-skills/skills/memory-systems/SKILL.md` | VENDORED |
| agent-browser | 488 | `~/.claude/skills/agent-browser/SKILL.md` | EDITABLE |
| ds-consolidation | 480 | `~/.claude/skills/ds-consolidation/SKILL.md` | EDITABLE |
| seo-geo | 468 | `~/.claude/skills/seo-geo/SKILL.md` | EDITABLE |
| obsidian-cli | 467 | `~/antigravity-skills/skills/obsidian-cli/SKILL.md` | VENDORED |
| dogfood | 460 | `~/.claude/skills/dogfood/SKILL.md` | EDITABLE |
| pdf | 437 | `~/antigravity-skills/skills/pdf/SKILL.md` | VENDORED |
| doc-coauthoring | 428 | `~/antigravity-skills/skills/doc-coauthoring/SKILL.md` | VENDORED |
| visual-explainer | 416 | `~/.claude/skills/visual-explainer/SKILL.md` | EDITABLE |
| frontend-design | 399 | `~/antigravity-skills/skills/frontend-design/SKILL.md` | VENDORED |
| ads-budget | 369 | `~/.claude/skills/ads-budget/SKILL.md` | EDITABLE |
| context-fundamentals | 356 | `~/antigravity-skills/skills/context-fundamentals/SKILL.md` | VENDORED |
| context-degradation | 349 | `~/antigravity-skills/skills/context-degradation/SKILL.md` | VENDORED |
| ads-linkedin | 346 | `~/.claude/skills/ads-linkedin/SKILL.md` | EDITABLE |
| seo-programmatic | 340 | `~/.claude/skills/seo-programmatic/SKILL.md` | EDITABLE |
| ads-competitor | 335 | `~/.claude/skills/ads-competitor/SKILL.md` | EDITABLE |
| ads-creative | 330 | `~/.claude/skills/ads-creative/SKILL.md` | EDITABLE |
| react-best-practices | 329 | `~/antigravity-skills/skills/react-best-practices/SKILL.md` | VENDORED |
| internal-comms | 329 | `~/antigravity-skills/skills/internal-comms/SKILL.md` | VENDORED |
| phoenix-liveview-architect | 328 | `~/.claude/skills/phoenix-liveview-architect/SKILL.md` | EDITABLE |
| ads-microsoft | 327 | `~/.claude/skills/ads-microsoft/SKILL.md` | EDITABLE |

## 3. Top 20 editable offenders — proposed compactions

Each compaction preserves all explicit trigger keywords from the original. Telegraphic English; no "This skill should be used when...". Target ≤ 120 chars.

| skill | orig | new | proposed_description |
|---|---|---|---|
| seo | 870 | 119 | `Triggers: SEO, audit, schema, Core Web Vitals, sitemap, E-E-A-T, AI Overviews, GEO, structured data. Scope: full site/page audits.` |
| ads | 752 | 117 | `Triggers: ads, PPC, paid advertising, Google/Meta/LinkedIn/TikTok/Microsoft Ads, ROAS, ad audit, creative fatigue, bid strategy.` |
| reality-first-stabilization | 684 | 117 | `Triggers (FR): audit qualité, reprise projet, dette technique, Nix/Devenv instable, divergence docs/code. Stabilise avant optim.` |
| dev-cycle | 544 | 119 | `Triggers (FR): nouveau, crée, implémente, corrige, bug, refacto, documente, audite, review. Cycle TDD complet, MkDocs, mémoire.` |
| agent-browser | 488 | 117 | `Triggers: open website, fill form, click button, screenshot, scrape, login, automate browser, test web app. CLI for AI agents.` |
| ds-consolidation | 480 | 119 | `Triggers: consolidate project, cleanup code, verify project, prepare for release, audit codebase. Multi-lang prod-readiness.` |
| seo-geo | 468 | 117 | `Triggers: AI Overviews, SGE, GEO, AI search, LLM optimization, Perplexity, AI citations, ChatGPT search. llms.txt, citability.` |
| dogfood | 460 | 119 | `Triggers: dogfood, QA, exploratory test, find issues, bug hunt, test app/site/platform. Repro report w/ screenshots+videos.` |
| visual-explainer | 416 | 119 | `Triggers: diagram, architecture overview, diff review, plan review, comparison table, visual explanation. Self-contained HTML.` |
| ads-budget | 369 | 117 | `Triggers: budget allocation, bidding strategy, ad spend, ROAS target, media budget, scaling. 70/20/10 rule, 3x Kill Rule.` |
| ads-linkedin | 346 | 119 | `Triggers: LinkedIn Ads, B2B ads, sponsored content, lead gen forms, InMail, LinkedIn campaign. 25 checks, Thought Leader, ABM.` |
| seo-programmatic | 340 | 119 | `Triggers: programmatic SEO, pages at scale, dynamic pages, template pages, generated pages, data-driven SEO. Index-bloat safety.` |
| ads-competitor | 335 | 119 | `Triggers: competitor ads, ad spy, competitive analysis, competitor PPC, ad intelligence. Cross-platform copy/keyword/spend gaps.` |
| ads-creative | 330 | 116 | `Triggers: creative audit, ad creative, creative fatigue, ad copy, ad design, creative review. Cross-platform fatigue detection.` |
| phoenix-liveview-architect | 328 | 119 | `Triggers: Phoenix LiveView, OTP dashboard, Tailwind/Glassmorphism UI. Mission-critical + high-fidelity. Streams/PubSub patterns.` |
| ads-microsoft | 327 | 116 | `Triggers: Microsoft Ads, Bing Ads, Bing PPC, Copilot ads, Microsoft campaign. 20 checks, Google import validation, PMax.` |
| ads-tiktok | 326 | 119 | `Triggers: TikTok Ads, TikTok marketing, TikTok Shop, Spark Ads, Smart+, TikTok campaign. Creative-first, safe-zone, 25 checks.` |
| ads-youtube | 323 | 117 | `Triggers: YouTube Ads, video ads, pre-roll, bumper ads, YouTube campaign, Shorts ads. Skippable/non-skip/Demand Gen analysis.` |
| ads-audit | 322 | 119 | `Triggers: audit, full ad check, analyze my ads, account health check, PPC audit. Multi-platform parallel-subagent health score.` |
| ads-google | 322 | 116 | `Triggers: Google Ads, Google PPC, search ads, PMax, Performance Max, Google campaign. 74 checks: tracking, waste, structure.` |

## 4. Estimated savings

- Top-20 EDITABLE total today: **8,830 chars (~2,207 tokens)** at every cold-start.
- After compaction: **~2,360 chars (~590 tokens)**.
- **Net savings: ~6,470 chars (~1,617 tokens) per cold-start.**

If the user later approves a Phase-2 vendored cleanup, the achievable upper bound is ~25,000 chars / ~6,250 tokens (most of the 33 K total budget).

## 5. Recommended execution

### Phase 1 — EDITABLE compaction (this session, after approval)
- Apply the 20 proposed `description:` rewrites listed in §3.
- Each edit is a one-line YAML field change in the frontmatter; preserves all other content.
- Verifiable by `head -10 SKILL.md` per file. Reversible with `git diff` / shell undo.
- Expected immediate cold-start gain: **~1,600 tokens**.

### Phase 2 — VENDORED skills (deferred, requires policy decision)
- 58 vendored skills consume ~16,600 chars / ~4,150 tokens. Editing in-place is not durable (overwritten on `git pull` or plugin update).
- Three options for the user to choose from:
  1. **Disable rarely-used vendored skills** via `~/.claude/settings.json` `disabledSkills` array — cheapest, no upstream divergence. Candidates: `slack-gif-creator`, `algorithmic-art`, `theme-factory`, `react-native-skills`, `remotion`, `notebooklm`, the unused `seo-*` siblings (if not doing SEO work).
  2. **Replace symlinks with edited copies** under `~/.claude/skills/<name>/` — durable but creates drift from `antigravity-skills` upstream. Track which skills are forked.
  3. **Submit upstream PRs** to `antigravity-skills` and the plugin marketplaces shrinking the descriptions — slowest, but benefits all users and avoids drift.
- Recommendation: option 1 first (zero-risk, instant win), then option 3 for highest-frequency skills (`xlsx`, `docx`, `pdf`, `pptx`, `claude-api`).

### Phase 3 — Per-project skill scoping (longer-term)
- Many skills (`ads-*`, `seo-*`, `phoenix-liveview-architect`, `legal-to-typedb`, `typedb-3-architect`, `ladybug-architect`) are project-specific and should not load in unrelated projects (e.g. `axon`).
- Move them from `~/.claude/skills/` (global) into the relevant project's `.claude/skills/` (scoped). Estimated additional saving: ~2,500 tokens for non-ads, non-SEO sessions.

### Out of scope this run
- No SKILL.md was edited. No `settings.json` was changed. No skill was disabled.
