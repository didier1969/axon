# Proposal — Session Memory Auto-Checkpoint (REQ-AXO-XXX)

**Status**: proposed
**Priority**: high (commercial value-add)
**Tags**: axon-product-improvement, llm-friction, commercial-value, session-memory
**Originator**: Didier + Claude Opus 4.7 — session 2026-05-04

---

## Concept

Add an **automated session-memory checkpoint cycle** so an LLM agent
can hand off and resume across context-window saturation without
manual operator intervention beyond a single `/clear`.

The cycle uses three signals:

1. **LLM observes its own context utilisation** (via the
   `[MODERATE] (X% remaining)` markers exposed by Claude Code in
   UserPromptSubmit reminders).
2. **At a threshold** (recommended: 70% used = 30% remaining), the LLM
   emits an `axon_session_checkpoint` MCP call **proactively**, then
   prints a closing handoff including a marker token.
3. **A Claude Code hook** detects the marker token in the LLM stop
   event and triggers `/clear` + `axon init` in a fresh session.
   The fresh session reads the checkpoint via the kickoff bundle.

**Result**: continuous workflow across infinite session length, with
context regeneration every ~70% utilisation.

---

## Why this matters commercially

LLM agents degrade gradually as context fills. By 80% utilisation,
recall of mid-conversation facts drops; by 90% hallucination rate
climbs. The current operator workaround is to manually `/clear` and
re-init, which:

- Loses the session narrative (operator decisions, corrections, transient
  results not committed to SOLL).
- Burdens the operator with deciding *when* to checkpoint.
- Risks a too-late checkpoint (LLM already degraded) producing a
  poor-quality summary.

Axon's competitive position is "structural intelligence MCP for LLM
agents". Adding **agentic session continuity** turns Axon into a
complete substrate for autonomous multi-hour LLM workflows — a
categorical product differentiator vs. plain MCP servers.

---

## Architecture

### A. New SOLL entity: `Session` (SES-AXO-XXX)

Stored in `soll.main.Node` with `type='session'`. Suggested fields
(some in canonical columns, some in `metadata` JSON):

| Field | Storage | Purpose |
|---|---|---|
| `id` | column | `SES-{PROJECT}-{NNN}` (sequential) |
| `title` | column | one-line summary of session focus |
| `description` | column | free-form session narrative summary (300-1500 tokens) |
| `status` | column | `active` \| `closed` \| `superseded` |
| `metadata.context_pct_used` | JSON | LLM-reported utilisation at checkpoint emission |
| `metadata.decisions_active` | JSON | list of operator decisions still in force ("use TensorRT not CUDA", etc.) |
| `metadata.todos_pending` | JSON | what was about to happen but not done |
| `metadata.runtime_state` | JSON | { live_build, dev_running, bg_processes[], engine_cache_state } |
| `metadata.artifacts` | JSON | absolute paths to CSV/logs/working-notes from this session |
| `metadata.previous_session` | JSON | id of the session this one continues |

Edges: `Session → REFINES → Session` (continuity chain),
`Session → BELONGS_TO → Project`.

### B. New MCP tool: `axon_session_checkpoint`

```
axon_session_checkpoint(
  decisions_active: string[],
  todos_pending: string[],
  runtime_state: object,
  artifacts: string[],
  description: string,
)
```

Side-effects:
1. Creates new `SES-AXO-NNN` entity (status=active).
2. Marks previous active session for project as `closed`, links new → old.
3. Writes a markdown mirror at `docs/working-notes/<date>-session-{NNN}.md` for human auditing.
4. Returns the new session ID + marker token for the hook.

### C. Marker protocol — LLM ↔ Hook

When the LLM emits `axon_session_checkpoint`, it follows up with
its closing message ending in:

```
<<axon-session-checkpoint:SES-AXO-NNN>>
```

The Claude Code `Stop` hook (configured in
`~/.claude/settings.json`) regex-matches this marker, and on match:
1. Logs the session ID
2. Issues `/clear`
3. Issues `axon init` (the user's existing trigger phrase) which
   loads `kickoff_bundle.active_handoff` → that's now the `SES-AXO-NNN`
   entity, automatically continuing the work.

Operator only intervenes if they want to override or skip the
auto-resume.

### D. Kickoff bundle integration

`axon_init_project` already returns `data.kickoff_bundle.active_handoff`.
Extend resolution:
1. Query for newest `Session` with status=`active` for this project.
2. If found → its description + structured metadata → embedded in
   bundle as `active_handoff` (replacing the file-based `.md` for
   sessions; `.md` mirror still exists for humans).
3. Fallback to existing file-based handoff if no `Session` exists.

---

## Token cost analysis

| Phase | Tokens (estimate) |
|---|---|
| Init session — read kickoff bundle (already paid today) | ~1500 |
| Checkpoint write at 70% — `axon_session_checkpoint` payload + ack | 800–1500 |
| Resume session — read prior `Session` via kickoff bundle | 1500–2500 |
| **Marginal cost per cycle** | **~3–5K tokens** |
| **% of 1M-window** | **0.3–0.5%** |

Negligible. Gain: **zero operator intervention** for context regen,
and **full session continuity** across arbitrary durations.

---

## Acceptance criteria

1. `axon_session_checkpoint` MCP tool exists and creates a `Session`
   entity with `status=active`, links to previous active session.
2. LLM agent emits marker `<<axon-session-checkpoint:SES-XXX>>` at
   ≥70% context utilisation, after invoking the tool.
3. Claude Code `Stop` hook regex-matches the marker, runs `/clear`
   then triggers `axon init`.
4. Fresh session's `axon_init_project` returns the just-created
   `Session` as `active_handoff`, LLM resumes work without operator
   prompt.
5. End-to-end test: a 5-iteration loop on a perf workload spans
   ≥3 session boundaries autonomously.
6. Markdown mirror at `docs/working-notes/<date>-session-{NNN}.md`
   produced for human audit.
7. Token cost per cycle measured and logged: target ≤5K tokens.

---

## Anti-patterns to avoid

- **Don't auto-checkpoint at every turn** — burns tokens and
  fragments narrative. Only at threshold.
- **Don't store full conversation transcript** in `Session.description`
  — that's lost-in-middle re-creation. Store *synthesised decisions*
  and *runtime state*, not raw turns.
- **Don't make checkpointing depend on `/compact`** — `/compact`
  generates a narrative summary; we need *structured* state.
- **Don't trigger the hook on every assistant message** — only on
  `Stop` events containing the literal marker token.
- **Don't bypass operator override** — provide a slash command
  `/axon-skip-resume` that the operator can issue if they want a
  fully-clean session.

---

## Phasing

| Phase | Effort | Output |
|---|---|---|
| **P1** Schema + tool | 1 day | `Session` type in `soll_relation_schema`, `axon_session_checkpoint` MCP tool wired |
| **P2** Kickoff bundle integration | 0.5 day | `axon_init_project` resolves Session as active_handoff |
| **P3** Marker + hook | 0.5 day | LLM marker emission protocol documented in CPT-AXO-019, sample `~/.claude/settings.json` hook published |
| **P4** Validation | 1 day | E2E test: 3-session autonomous workflow on perf bench |

**Total: ~3 dev-days for P1+P2+P3, 1 day P4.**

---

## Reuse map (existing structure)

| Existing | Reused for |
|---|---|
| `axon_init_project` + `kickoff_bundle.active_handoff` | bootstrap of resumed session |
| `soll_manager(action=create)` | create new `Session` entity |
| `soll_attach_evidence` | attach CSV/logs to `Session` |
| `docs/working-notes/<date>-handoff-*.md` pattern | markdown mirror format |
| Claude Code `Stop` hook system | auto `/clear` + re-init trigger |
| User-side trigger phrases (`axon init`, `go`, `continue`) | resume command |

---

## Open questions

1. Should `Session` be a new SOLL entity type, or a sub-tag on
   `Decision`? Recommendation: new type — `Session` has different
   lifecycle (auto-closed when superseded) and shouldn't appear in
   normal `soll_work_plan` priorities.
2. How to handle multiple projects (`AXO`, `BKS`, etc.) in one
   conversation that may span sessions? Recommendation: `Session`
   is project-scoped; multi-project conversations get one
   `Session` per project, linked.
3. Should the threshold be tunable? Recommendation: start at 70%
   fixed, expose via `AXON_SESSION_CHECKPOINT_THRESHOLD_PCT` env
   for operators who want different timing.
