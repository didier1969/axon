# SOLL Evolution — Explicit Tech-Debt Tracking After Migration Decisions

> Status: CONCEPT — proposal awaiting review
> Origin: session-54 operator insight on residue accumulation from incomplete tech migrations
> Tracker: REQ-AXO-N (allocated at log time; umbrella REQ REFINES this document)
> Audience: LLM sessions, operator, Axon architecture reviewers
> Style: GUI-PRO-100 (tables > prose), GUI-PRO-015 (KISS), GUI-PRO-023 (vertical-slice decomposable)

---

## 1. Context and Problem

### 1.1 Operator statement (verbatim, session 54)

> « Lorsque je décide de remplacer une technologie par une autre [...] nous faisons un choix stratégique [...] Le problème [...] c'est qu'en fait le code généré contient partout des éléments liés à l'ancienne technologie. Nous ne voulons pas migrer le 100% ou pour des raisons X ou Y, nous n'allons pas le faire de manière brutale. [...] On vit après avec cette dette technologique de session en session, on oublie et ensuite notre produit n'est plus lean au lieu de pointer directement vers une technologie il nous reste des relents de technologies qu'on pousse avec nous. »

### 1.2 Architectural reframing

The SOLL graph captures **what should be** (intent). The IST captures **what is** (code). After a `Decision` to replace technology X with technology Y, the intent moves crisply: a `Decision` node + `Milestone` time-anchor + a `Requirement` umbrella (e.g. REQ-AXO-271 "DuckDB excision"). But the IST **continues to show traces of X** indefinitely:

- explicit residual code paths (call sites, type references, helpers)
- comments referencing the obsolete pattern ("DuckDB-era residue" in PIL-AXO-001 body)
- tests hardcoding pre-migration contracts (REQ-AXO-91560 hardcoding `'AXO'` because PG-canonical post-MIL-AXO-017 changed the contract)
- doc fragments and SKILL.md sections describing the old model

Today these are **discovered by accident** (during unrelated work, often by an LLM noticing inconsistency mid-task) and **fixed on the fly** without a queryable index. Each session re-discovers a subset. There is no closed-loop visibility on `% migrated` nor a pre-flight gate that warns when an LLM is about to touch a known-residue file.

### 1.3 Why the current 9 SOLL types are insufficient

| Need | Closest existing fit | Why it falls short |
|---|---|---|
| "All files still containing tech-X residue" | `Requirement` body (free text) | Not queryable structurally; bullet list rots |
| "Migration progress % over time" | `Milestone` status | Boolean-ish (planning / active / done); no per-symbol granularity |
| "Hybrid policy: keep both technologies for category C" | `Decision` body (prose) | Not machine-checkable at edit time |
| "Pre-flight warns on edit of residue file" | `Guideline` + LLM goodwill | Guideline is advisory text; no IST bridge |
| "Why does X still exist here?" trace | `SUPERSEDES` edge between Decisions | Operates at intent level, not at code-symbol level |

The SKI/PRT additions (REQ-AXO-91576) target methodology re-use across tenants — orthogonal to this problem.

### 1.4 Concrete Axon case studies

| Migration | Decision / Milestone | Residue evidence (session 54) |
|---|---|---|
| DuckDB → PG | MIL-AXO-015, REQ-AXO-271, DEC-AXO-083, MIL-AXO-017 | "DuckDB-era residue" still flagged in PIL-AXO-001 + CPT-AXO-053 bodies; ~50 AGE call sites identified as deferred during REQ-AXO-300 phase A |
| AGE retirement | DEC-AXO-083, MIL-AXO-017 | AGE schema still installed but dormant (gates 1+2 of VAL-AXO-073 still FAIL); REQ-AXO-300 phases B+C deferred |
| FVQ/GPQ telemetry purge | REQ-AXO-901674 | Rust emitters removed but consumer-side residue tracked manually |
| Pipeline v1 → v2 streaming | REQ-AXO-289, CPT-AXO-054 | Legacy `public.file` machine-à-états still referenced in dormant code; superseded `CPT-AXO-026/030/031/032/035/044` |

In every case the same anti-pattern: **intent reified in SOLL, residue accumulates in code, no bidirectional bridge**.

---

## 2. What SOLL Already Does Well

| Capability | Mechanism | Verdict |
|---|---|---|
| Capture strategic choice | `Decision` node (e.g. DEC-AXO-083 "Retire AGE; unify on public.Edge") | OK |
| Anchor a target date / scope | `Milestone` (MIL-AXO-017) | OK |
| Group residue cleanup work | `Requirement` umbrella + child REQs (REQ-AXO-271 slices 1-8) | OK at planning level |
| Express obsolescence between intent nodes | `SUPERSEDES` edge (20 instances in graph) | OK between SOLL nodes |
| Audit history of choices | `soll.Revision` + `soll.RevisionChange` | OK |
| Apply rule at delivery time | `Guideline` + `axon_pre_flight_check` | OK for textual policy, weak for IST-grounded rules |

The SOLL graph **knows the migration happened**. It does not **know what is left in the code** nor **how much remains**.

---

## 3. Gap Analysis — What Is Missing

| Gap | Concrete symptom | Today's workaround |
|---|---|---|
| G1 — No queryable residue inventory at file/symbol granularity | "Show me all DuckDB references" requires `rg duckdb`; not in SOLL | Manual grep, ad-hoc, no persistence between sessions |
| G2 — No migration progress signal | "Are we at 60% or 95% migrated off AGE?" unanswerable in SOLL | Operator estimates from slice tickets ("phases B+C deferred") |
| G3 — No edit-time gate | LLM edits `pipeline_v2_runtime.rs`, doesn't know it touches deprecated code path | LLM stumbles on stale comment; sometimes fixes, sometimes not |
| G4 — `Decision` lacks `from_tech` / `to_tech` typed semantics | Cannot answer "what decisions deprecate X?" structurally | Parse Decision titles by regex |
| G5 — No "debt policy" per migration | Hybrid vs full-clean is implicit in Decision prose | Operator must remember per-migration policy |
| G6 — No bidirectional SOLL↔IST link for residue | SOLL says "migrate from X"; IST has X-flavored symbols; no edge between them | LLM reconstructs from scratch each session |
| G7 — No debt budget / SLO | "We accept N residue lines for 6 weeks" unrepresentable | Slips silently past acceptable bounds |

---

## 4. Proposal — Option Recommended

### 4.1 Three options considered

| Option | Shape | Strength | Weakness |
|---|---|---|---|
| **A** | New entity type `TechnologyMigration` with `MIGRATES_FROM` / `MIGRATES_TO` / `HAS_REMNANT` edges + IST bridge MCP tool | Cleanest semantic surface; queryable inventory native | Schema migration in `soll.node.type` enum + new edge types; client docs to update |
| **B** | Extend `Decision` with `metadata.tech_replacement = {from, to, policy, inventory_ref}` + MCP tool `decision_residue` | Zero schema change; metadata-routed per REQ-AXO-91499 | Overloads `Decision` semantics; weak join surface for queries; inventory still has to live somewhere |
| **C** | New `Guideline` "X-residue forbidden in new code" + IST scan rule engine | Reuses existing scaffolding | Guidelines are advisory text, no machine semantics; rule engine is the real new work, hidden behind a Guideline |

### 4.2 Recommendation: **Option A** (with B as fallback if schema-migration cost is judged too high)

**Rationale**:

1. The problem **is** a first-class concept ("a migration with leftover residue"), not a metadata wart on `Decision`. Promoting it to an entity matches SOLL's existing pattern (Vision, Pillar, Concept… each is a recognised concern with its own queries).
2. **Queryable inventory** is the load-bearing feature. Joining `Symbol`/`Chunk`/`IndexedFile` (IST tables) against `TechnologyMigration.HAS_REMNANT` is natural with an edge type; awkward inside JSONB metadata.
3. The `HAS_REMNANT` edge is **the bidirectional SOLL↔IST bridge** that no other type provides today — that bridge is exactly what enables `axon_pre_flight_check` to warn at edit time.
4. KISS-compliant: one entity, ≤6 new edge types, one new MCP tool surfacing the inventory. Re-uses the existing `soll_manager` create/update/link/unlink protocol. Re-uses `axon_pre_flight_check` integration point.

**Tradeoffs accepted**:

- `soll.node.type` enum gains one variant → migration script + 1-line client doc update.
- New edge types must be added to `soll_relation_schema` registry.
- Indexer gets one new scan responsibility (Section 5.4). Detection-rule complexity is the real risk — mitigated by starting with pure regex/tree-sitter pattern matchers, no semantic analysis.

### 4.3 Schema sketch (Option A)

```text
TechnologyMigration {
  id:               TMG-AXO-N            -- per DEC-AXO-085 (new type prefix TMG)
  title:            "DuckDB → PostgreSQL"
  status:           planning | active | hybrid_accepted | complete | abandoned
  from_tech:        "DuckDB"
  to_tech:          "PostgreSQL"
  debt_policy:      full_clean | keep_dual | hybrid | freeze_legacy
  debt_budget:      { max_residue_files: 0, deadline: "2026-06-30" } | null
  detection_rules:  [ { kind: regex|symbol|import, pattern: "...", scope: "src/**" } ]
  description:      <prose — rationale, exception cases, hybrid rules>
}

Edges (additions to soll_relation_schema):
  TechnologyMigration  --DECIDED_BY-->     Decision
  TechnologyMigration  --ANCHORED_BY-->    Milestone
  TechnologyMigration  --HAS_REMNANT-->    IndexedFile|Symbol|Chunk   (IST cross-graph edge)
  TechnologyMigration  --SUPERSEDED_BY-->  TechnologyMigration         (chained migrations)
  Requirement          --RESOLVES_REMNANT->TechnologyMigration         (cleanup work tracker)
```

**Note on cross-graph edges**: `HAS_REMNANT` is the only SOLL→IST edge today. Implementation lives in `soll.edge` with `target_kind='ist:indexed_file' | 'ist:symbol' | 'ist:chunk'` discriminator. Resolution at query time joins through the discriminator. This is materially less work than a new graph table because the cardinality is bounded by detection rules (typically 10s–100s of remnants per migration).

---

## 5. Workflow Integration

### 5.1 `axon_pre_flight_check`

| Trigger | Action |
|---|---|
| Staged edit touches file in any `TechnologyMigration.HAS_REMNANT` set | Warning: `"file X has Y unresolved remnants of migration Z (policy=full_clean) — replace, document hybrid, or update debt_budget"` |
| Edit ADDS a pattern matched by an active detection rule | Hard block (override-able with `--ack-debt`): `"new residue of retired tech X — see TMG-AXO-N"` |
| Active migration has `debt_budget` exceeded | Block all promote-live until budget restored or operator explicitly reauthorises |

### 5.2 `query` / `inspect` / `retrieve_context`

| Tool | Augmentation |
|---|---|
| `query(symbol)` | Response includes `residue_of: ["TMG-AXO-N (DuckDB→PG)"]` when the symbol is in a `HAS_REMNANT` set |
| `inspect(symbol)` | Adds `tech_debt_context` block with migration id, policy, suggested action |
| `retrieve_context` | SOLL rationale section includes the migration node alongside the existing Decision/Milestone nodes — answers "why does this still look like X?" structurally |

### 5.3 Sub-agent briefing

When the main thread dispatches a sub-agent to touch a file, the briefing payload auto-injects:

```text
TECH_DEBT_CONTEXT:
  - TMG-AXO-001 (DuckDB→PostgreSQL): file has 3 remnants (lines 47, 102, 380); policy=full_clean
  - TMG-AXO-002 (AGE→public.Edge):  file has 1 remnant (line 215); policy=keep_dual_until=2026-07-01
```

Sub-agent operates with explicit residue awareness — eliminates the "discovers residue mid-task without context" failure mode.

### 5.4 Indexer-side residue detection

Detection runs as a small additional pass after stage A3 (graph persist), reading from `TechnologyMigration.detection_rules` and updating the `HAS_REMNANT` edge set incrementally. Per-file; same `IndexedFile(path, content_hash)` filter applies so unchanged files re-use prior results.

Cost: bounded by N_active_migrations × M_rules_per_migration × file_size. Typical Axon today: 4 active migrations × 5 rules each × tree-sitter visit = << A2 (parser) cost.

### 5.5 `soll_work_plan`

When any active migration has `debt_budget` breached or `deadline` within 14 days and remnants > 0, `soll_work_plan` boosts the cleanup `Requirement` (linked via `RESOLVES_REMNANT`) to wave 1 — automatic prioritisation without operator intervention.

### 5.6 New MCP tool

```
mcp__axon__tech_debt_inventory(
  migration_id: "TMG-AXO-001",              -- or filter by from_tech / to_tech
  group_by:     file|symbol|chunk,
  status:       all|unresolved|hybrid_accepted
) → {
  total_remnants: 47,
  by_file: [ { path, count, lines: [...], policy, suggested_action } ],
  budget:  { current: 47, max: 0, breached: true, deadline: "2026-06-30" }
}
```

---

## 6. Walk-Through — DuckDB→PG Migration As If This Existed

| Step | Actor | Action | SOLL/IST state |
|---|---|---|---|
| 1 | Operator | `soll_manager(create, technology_migration, {from_tech:"DuckDB", to_tech:"PostgreSQL", debt_policy:"full_clean", detection_rules:[{kind:"regex",pattern:"(?i)duckdb"},{kind:"import",pattern:"duckdb_rs::"}], attach_to:"DEC-AXO-083", relation_type:"DECIDED_BY"})` | TMG-AXO-001 created; linked to existing DEC-AXO-083, MIL-AXO-017 |
| 2 | Indexer | Next scan pass populates `HAS_REMNANT` edges for every file matching rules | 47 remnants found across 12 files; PIL-AXO-001 body included (comment-residue) |
| 3 | Sub-agent | Briefing for "edit pipeline_v2_runtime.rs" auto-injects residue context: "2 DuckDB lines, policy=full_clean" | Sub-agent knows BEFORE first read |
| 4 | Sub-agent | Migrates the 2 lines, commits | A3 re-scans the file; `HAS_REMNANT` count drops from 47 → 45 |
| 5 | `axon_pre_flight_check` | Reports: `TMG-AXO-001 progress: 45/47 remaining (4.3% cleared this commit)` | Operator sees real progress |
| 6 | Operator | `tech_debt_inventory(TMG-AXO-001, group_by=file)` | Single sorted list of 10 remaining files, ready for batch cleanup REQ |
| 7 | Cleanup REQ closes (last remnant migrated) | Indexer scan finds 0 remnants → status auto-transitions `active → complete` | Migration officially done; gate honest |

Compare to today: step 1 exists implicitly (DEC-AXO-083); steps 2-6 are manual or absent; step 7 has no signal — operator must trust REQ status which lags reality.

---

## 7. Acceptance Criteria for the Concept

| AC | Measurable |
|---|---|
| AC1 — Backward compatibility | All existing SOLL nodes/edges remain valid; no breaking change to `soll_query_context`, `soll_work_plan`, `soll_manager` for existing 9 types |
| AC2 — Query latency | `tech_debt_inventory` p95 < 100 ms on production-scale graph (Axon: ~5k symbols, ~50k edges) |
| AC3 — Detection precision | False-positive rate < 5% on Axon DuckDB ruleset measured against operator-curated gold set |
| AC4 — Detection recall | Recall ≥ 95% on same gold set |
| AC5 — Zero overhead when idle | Project with 0 active migrations sees < 1ms added to pre_flight_check and indexer stages |
| AC6 — Operator UX preserved | Existing `axon_commit_work` flow unchanged unless residue policy violated; opt-out via `--ack-debt` |
| AC7 — Auditability | Every status transition + remnant resolution recorded in `soll.Revision` |
| AC8 — Self-documenting | After 30 days of use, `soll_query_context` can answer "what are our open technology migrations and their progress?" with zero free-text parsing |

---

## 8. Risks and Mitigations

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Detection rules too strict → false-positive friction, LLMs ignore warnings | M | H | Start with regex-only, gold-set calibration before activation, explicit `--ack-debt` escape hatch |
| R2 | Schema migration cost (new `type` enum value + edge types) | L | M | Standard `soll.Node.type` migration; precedent exists (Guideline added via REQ-AXO-092) |
| R3 | Cross-graph SOLL↔IST edge introduces resolver complexity | M | M | Use discriminator column `target_kind` instead of new graph table; resolved at query time only |
| R4 | Indexer cost overhead on large projects | L | M | Detection runs only on changed files (existing IndexedFile filter); per-migration rule count bounded |
| R5 | Migration status drift if detection ruleset is incomplete | M | M | "Complete" status requires explicit operator confirmation OR 30 consecutive scans with 0 remnants |
| R6 | Operator must maintain `detection_rules` — overhead for each migration | M | L | Provide rule templates (regex per technology) + LLM-assisted rule synthesis from Decision body |
| R7 | Pre-flight blocks become noisy if budget logic too aggressive | M | H | Warnings (not blocks) by default; blocks only when explicit `debt_budget` declared AND breached |
| R8 | New entity competes with existing `Decision` for "where do I write this?" | L | M | `soll_manager` validation: creating `TechnologyMigration` requires linking to an existing `Decision` via `DECIDED_BY` — never standalone |

---

## 9. REQ-Children Candidates (Vertical Slices per GUI-PRO-023)

Each slice ships UI/CLI surface + storage + indexer integration + tests. First slice integrates all boundaries with one stub migration.

| Slice | Title | Scope |
|---|---|---|
| **REQ-N1** | `TechnologyMigration` entity end-to-end thin slice | Add `technology_migration` to `soll_manager` `entity` enum + 1 new edge type `DECIDED_BY`; one `tech_debt_inventory` MCP tool returning empty results from an empty `HAS_REMNANT` set; create TMG-AXO-001 stub for DuckDB→PG manually; pre-flight reads it but takes no action |
| **REQ-N2** | Indexer-side detection rule engine | Add detection-rule executor in A3 (regex + import patterns); populate `HAS_REMNANT` edges; backfill on TMG-AXO-001 (DuckDB) and TMG-AXO-002 (AGE) to validate against operator gold set |
| **REQ-N3** | `axon_pre_flight_check` warn-on-residue integration | Wire `HAS_REMNANT` lookup into pre-flight; emit warnings with migration id + policy + suggested action; opt-out flag `--ack-debt` |
| **REQ-N4** | `query` / `inspect` / `retrieve_context` residue surfacing | Augment three retrieval tools to inject `residue_of` / `tech_debt_context` blocks; verify on DuckDB case study |
| **REQ-N5** | Sub-agent briefing auto-injection | Briefing payload assembler reads `HAS_REMNANT` for target files; injects `TECH_DEBT_CONTEXT` block; test on Task tool dispatch |
| **REQ-N6** | Debt budget + `soll_work_plan` priority boost + progress dashboard | `debt_budget` field enforcement; auto-boost cleanup REQs to wave 1 when budget breached or deadline near; Elixir/Phoenix dashboard view (observation-only per existing arch) |

Suggested attach: each REQ-N* REFINES the umbrella `REQ-AXO-N` (this document's tracker); each is independently demoable.

---

## 10. Tags and Originator

**Tags**: `soll-evolution`, `tech-debt-tracking`, `migration-residue`, `session-54-operator-insight`, `concept-doc`, `axon-product-improvement`, `commercial-value`, `lean-code`

**Originator**: Operator (Didier Stadelmann), session 54, 2026-05-23. Triggered by recurring discovery of DuckDB/AGE/FVQ residue across sessions (DuckDB→PG migration spanning MIL-AXO-015 / REQ-AXO-271 / MIL-AXO-017 / REQ-AXO-300, still not 100% closed at session 54).

**Authored by**: sub-E1 (LLM expert) under direction of main thread.

**Cross-references**:

- `PIL-AXO-003` (Intentional Knowledge Continuity) — pillar this concept extends
- `GUI-AXO-1003`, `GUI-AXO-1023` — anti-half-implementation discipline (this concept turns that discipline into a queryable enforcement layer)
- `GUI-PRO-015` (KISS), `GUI-PRO-023` (vertical slicing), `GUI-PRO-100` (token-efficient writing) — methodology compliance
- `DEC-AXO-085` (canonical SOLL ID format) — `TMG-` prefix proposal compliant
- `CPT-AXO-053` (brain-vs-indexer split) — indexer is correct home for detection-rule execution
- Case studies: MIL-AXO-015, REQ-AXO-271, DEC-AXO-083, MIL-AXO-017, REQ-AXO-300, REQ-AXO-901674

---

## Appendix — Why NOT just add a `Guideline` and let LLMs enforce (Option C, dismissed)

Tried implicitly today: GUI-AXO-1003 and GUI-AXO-1023 already mandate "no half-implementation" and "Swiss-hiking discipline" (residue → resolved OR logged). Session-54 evidence shows these are necessary but insufficient: an LLM only knows about residue it stumbles on. The graph does not surface residue proactively. Adding more Guidelines without an IST bridge keeps the failure mode. Option A turns intent into structure that pre-flight and retrieval tools enforce — Guideline becomes the WHY, `TechnologyMigration` becomes the HOW.
