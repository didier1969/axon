# Bootstrap Prompt — Read first, every Axon-equipped LLM session

> **Canonical SOLL location**: this prompt is intended to live as `GUI-PRO-018` (Guideline) in the cross-project `PRO` SOLL graph. Currently stored as `DEC-PRO-001` (Decision workaround) because `soll_manager` does not yet accept the `guideline` entity — see **REQ-AXO-092** for the API gap. Migrate to `GUI-PRO-018` once that requirement lands.

> **How to invoke this prompt for any LLM**:
> *"Read `BOOTSTRAP_PROMPT.md` at the project root, OR `mcp__axon__cypher` with `SELECT description FROM soll.main.Node WHERE id = 'DEC-PRO-001'`, then execute its steps literally."*

---

Self-contained bootstrap prompt for any LLM picking up a project that has Axon MCP access. Set 2026-05-01 by Didier as the universal entry point — applies to every project equipped with Axon.

## STEP 1 — Cold-start reading order (mandatory, in order)

1. `~/.claude/CLAUDE.md` — cross-project standing rules.
2. Project-level `CLAUDE.md` — project-specific discipline.
3. `MEMORY.md` if present in the persistent memory dir.
4. `mcp__axon__help` — confirm MCP reachable. If timeout, restart the live brain via `bash /home/dstadel/projects/axon/scripts/lib/start-brain.sh` with `AXON_INSTANCE_KIND=live`, then retry.
5. `mcp__axon__status mode=brief` — runtime instance, profile, freshness, vector backlog, scheduler.
6. **Project Vision** — `mcp__axon__cypher` with
   `SELECT id, title, description FROM soll.main.Node WHERE project_code = THIS_PROJECT AND type = 'Vision'`.
   The Vision is the commercial purpose of THIS project. Without it, every code decision is an arbitrary local optimization.
7. **Project Pillars** — `mcp__axon__cypher` with
   `SELECT id, title, description FROM soll.main.Node WHERE project_code = THIS_PROJECT AND type = 'Pillar' ORDER BY id`.
   Read each pillar in full. They are the strategic axes already decided. Skipping causes drift.
8. **Already-completed work** — `mcp__axon__cypher` for accepted Decisions and completed Milestones in THIS project.
9. **Open problems** — `mcp__axon__soll_validate` returns current invariant violations.
10. **Work plan** — `mcp__axon__soll_work_plan format=brief limit=15 top=5` — scored topological order. Wave 1 score is authoritative.

Skipping this order is the single biggest source of LLM drift across sessions.

## STEP 2 — Intent reconciliation

User explicit ask **overrides** SOLL ordering. SOLL ordering wins by default when no direction is given. On conflict, state both options once, ask, then commit.

## STEP 3 — Operational loop (repeat for every meaningful unit of work)

**OBSERVE** — friction, bugs, simplifications, obsolete elements, LLM-contract violations. Do not wait for prompts.

**LOG** via `mcp__axon__soll_manager(action=create)`:
- `requirement` for actionable findings (bugs, simplifications, robustness gaps)
- `decision` for choices accepted
- `concept` for shared mental models
- `milestone` for time-anchored deliverables
- `validation` for proof
Even partial-analysis observations land — they capture friction before it consolidates.

**LINK** via `mcp__axon__soll_manager(action=link)`. Canonical directions:
- `REQ —BELONGS_TO→ PIL`
- `CPT —EXPLAINS→ REQ`
- `DEC —SOLVES/IMPACTS→ REQ`
- `PIL —EPITOMIZES→ VIS`

Graph density is what makes the work plan compute meaningful priorities.

**RE-PLAN** via `mcp__axon__soll_work_plan` after each batch.

**EXECUTE** the highest-score unblocker. Build, test, smoke-test before commit. Use `mcp__axon__axon_pre_flight_check` then `mcp__axon__axon_commit_work` to deliver — never raw `git commit`.

## STEP 4 — Hygiene

- Attach `acceptance_criteria` field on every Requirement created.
- Run `mcp__axon__soll_validate` after each batch — target zero violations.
- Orphan REQ → link to a Pillar.
- Decision without `SOLVES`/`IMPACTS` → link to a REQ.
- Empty REQ → archive with explicit title.

## STEP 5 — When to stop / interrupt the user

Interrupt **ONLY** for:
- destructive irreversible action (force-push, drop table, `rm -rf` data not in scope)
- architectural decision needing human authority
- hard blocker requiring information that cannot be derived
- milestone result worth communicating

Otherwise execute autonomously.

## Failure modes to avoid

- Skipping cold-start reading order. Most expensive mistake.
- Asking the user too much. Default is autonomous.
- Skipping the log step. Every observation lands in SOLL.
- Markdown fallback when MCP is reachable. Restart MCP if down.
- Closing a re-verified observation as "my misunderstanding". The misread IS the bug — reframe as an LLM-contract violation.
- Using raw `git commit`. Always `axon_commit_work`.
- Treating SOLL as a passive log. The protocol turns SOLL into an active driver.
- **Cypher `INSERT`/`UPDATE`/`DELETE`** to bypass missing API. Cypher is read-only by spec. If a path is missing (e.g., guideline create), file a Requirement, not a workaround. (Note: `soll_manager(action=create, entity=guideline)` IS now supported as of REQ-AXO-092 / commit 4891d0a.)
- **Premature stop on wave-1 unblockers.** Do NOT write a closing summary, `/schedule` offer, or "want me to continue?" question while the top-scored Decision in `soll_work_plan` still has open descendants AND the user has authorized autonomous execution (e.g., "go", "décide", "continue"). The "milestone reached" interrupt trigger means external impact (deploy, release, fix unblocking another human) — NOT internal velocity ("I made N commits"). Before any closing-shaped message, run `mcp__axon__soll_work_plan format=brief top=3` and verify no open descendants remain on the top Decision; if open, keep going. Documented as `feedback_no_premature_stop_with_open_unblockers.md` in user memory.
- **`axon_commit_work` only auto-stages `git rm` deletions, NOT `Edit`/`Write` modifications.** When a single intended commit mixes deletions and edits, run `git add <modified-files>` explicitly BEFORE calling `axon_commit_work` so they are pre-staged like the deletions. After the commit returns "Commit succeeded", ALWAYS run `git status --short` — if any `M` files remain, the commit captured only the deletions and HEAD is bisect-broken. Documented as `feedback_axon_commit_work_only_stages_deletions.md` in user memory.

---

**END OF PROMPT — START AT STEP 1 NOW.**
