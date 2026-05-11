# Axon Methodology Delivery Specification

**Date:** 2026-05-11
**Status:** Design verrouillé — implémentation à venir
**Source:** session `/grill-me` 14-questions interview
**Audience:** LLM canonique (cette doc sera lue par la prochaine session pour exécuter)

---

## 0. Cadre commercial — pourquoi cette spec existe

Axon n'est pas qu'un indexeur graphe/vectoriel + couche SOLL. C'est aussi une **méthodologie de développement assistée par LLM de très haute qualité**, distribuée comme partie intégrante du produit commercial. Cette méthodologie se compose de :

1. **GUI-PRO (Guidelines)** : règles enforceables cross-project (TDD, deep modules, fail-fast, token-economy, etc.)
2. **CPT-PRO (Concepts)** : mental models canoniques (PDCA, onboarding loop, triage diagnostic, etc.)
3. **PIL-PRO (Pillars)** : axes méthodologiques groupant les guidelines par thème
4. **DEC-PRO (Decisions)** : décisions transverses (kickoff prompt, etc.)
5. **Skills** : `~/.claude/skills/*` SKILL.md déclencheurs front-door pour l'opérateur humain

Le présent document spécifie **comment livrer cette méthodologie aux clients d'Axon**. Le modèle retenu (γ — bundle versionné) impose un format canonique (JSON), un MCP tool d'application (`axon_apply_methodology_bundle`), et un cycle de versioning semver.

---

## 1. État courant analysé

### 1.1 Nœuds PRO existants (regularisation requise)

`soll.Node` contient **21 GUI-PRO + 3 CPT-PRO + 1 DEC-PRO** mais `ProjectCodeRegistry` ne référence **pas PRO**. Les nœuds existent en mode orphelin (seed bypass historique). Toute mutation via `soll_manager` / `soll_apply_plan` sur `project_code='PRO'` échoue actuellement avec `wrong_project_scope`.

```
Known project codes (registry, 2026-05-11):
APS, AXO, CCL, CTX, DOC, ERP, EXA, FLA, FSF, MFL, NBL, NEX, ODM,
OLL, OPT, RAL, RMC, SOK, SVZ, SWX, TE2, TRD, TRI, ZCL
                ^^^
                PRO ABSENT
```

### 1.2 Pattern d'héritage déjà opérationnel

`GUI-AXO-* INHERITS_FROM GUI-PRO-*` est déjà en production pour 4 consumer projects : AXO, FSF, MLD, NEX. C'est l'invariant exploitable :

```
soll.Edge (extrait) :
  GUI-AXO-001 INHERITS_FROM GUI-PRO-001
  GUI-FSF-001 INHERITS_FROM GUI-PRO-001
  GUI-MLD-001 INHERITS_FROM GUI-PRO-001
  GUI-NEX-001 INHERITS_FROM GUI-PRO-001
  …
```

### 1.3 Couverture méthodologique courante

**GUI-PRO actifs (21)** — voir titres dans `soll.Node WHERE id LIKE 'GUI-PRO-%' AND status='active'`.

Couverture forte :
- Code quality + APoSD : GUI-PRO-001 (TDD), 013-017 (DRY/SRP/KISS/cognitive/clean), 018-021 (Ousterhout)
- Reliability/ops : GUI-PRO-003-012 (fail-fast, no-mock, hermetic, telemetry, resilience, perf, security, IaC)

Couverture faible / absente :
- Workflow LLM (interview, vertical-slice, PRD pattern, handoff, diagnose loop)
- Resource economy (sub-agent policy, cache-TTL)
- Phase detection (Bootstrap vs Continuation)

**CPT-PRO actifs** : aucun (CPT-PRO-001/002/003 = placeholders synthétiques "MCP Validate Concept"). 4 CPT-AXO méthodologiques (CPT-AXO-019/020/024/025) sont à généraliser en CPT-PRO siblings.

### 1.4 Skills existants côté front-door

Pocock déjà mirorés dans `~/.claude/skills/` : `grill-me`, `tdd` (+5 docs), `to-issues`, `to-prd`, `improve-codebase-architecture` (+3 docs), `diagnose`, `caveman`. Format Pocock = GitHub Issues, non SOLL.

Skills SOLL-aware à créer (consumer-facing) : `axon-driven-development` (umbrella), `bootstrap-soll`, `to-prd-soll`, `to-issues-soll`, `improve-codebase-architecture-soll`, `axon-methodology-setup`.

---

## 2. Modèle de livraison retenu : γ — Bundle versionné

### 2.1 Choix architectural (Q14)

Modèle **γ seul** : bundle JSON versionné + MCP tool d'application, **sans** `system_managed` flag. Customer peut techniquement muter ses GUI-PRO mais on documente "don't mutate vendor methodology" et on track la divergence via `soll_validate`.

Rationale : MVP commercial v1.0 prioritise simplicité d'implémentation. Le flag `system_managed` (modèle β) reste backlog v2.0 si drift client devient un problème terrain.

### 2.2 Format de bundle canonique

```jsonc
{
  "schema": "axon-methodology-bundle-v1",
  "version": "1.0.0",
  "axon_min_version": "0.8.0",
  "project_code": "PRO",
  "released_at": "2026-05-11T00:00:00Z",
  "checksum_sha256": "<computed at build time>",

  "pillars": [
    {
      "logical_key": "pil_pro_code_quality",
      "title": "Code Quality",
      "description": "...",
      "status": "active"
    }
    // … 4 pillars total
  ],

  "concepts": [
    {
      "logical_key": "cpt_pro_soll_operational_protocol",
      "title": "...",
      "description": "...",
      "status": "active",
      "metadata": { "supersedes_pattern": "CPT-AXO-019" }
    }
    // … 4 concepts total
  ],

  "guidelines": [
    // 21 existing GUI-PRO regularization stanzas + 9 new
    // logical_key for existing : reuse-by-canonical-id-match
    {
      "logical_key": "gui_pro_tdd_obligatoire",
      "canonical_id_hint": "GUI-PRO-001",  // for re-apply idempotence
      "title": "TDD Obligatoire",
      "description": "...",
      "status": "active"
    }
    // …
  ],

  "decisions": [
    {
      "logical_key": "dec_pro_bootstrap_prompt",
      "canonical_id_hint": "DEC-PRO-001",
      "title": "BOOTSTRAP PROMPT - read first, every Axon-equipped LLM session",
      "description": "...",
      "status": "active"
    }
  ],

  "relations": [
    // BELONGS_TO theming (new)
    { "source": "gui_pro_tdd_obligatoire", "target": "pil_pro_code_quality", "type": "BELONGS_TO" },
    // … all GUI-PRO mapped to a PIL-PRO

    // Cross-project INHERITS_FROM (using canonical IDs of CPT-AXO existing)
    { "source": "CPT-AXO-019", "target": "cpt_pro_soll_operational_protocol", "type": "INHERITS_FROM" },
    { "source": "CPT-AXO-020", "target": "cpt_pro_llm_onboarding_loop", "type": "INHERITS_FROM" },
    { "source": "CPT-AXO-021", "target": "DEC-PRO-001", "type": "INHERITS_FROM" },
    { "source": "CPT-AXO-024", "target": "cpt_pro_llmonly_doc_methodology", "type": "INHERITS_FROM" },
    { "source": "CPT-AXO-025", "target": "cpt_pro_3way_diagnostic_triage", "type": "INHERITS_FROM" }
  ],

  "deprecations": [],

  "skills_manifest": [
    // future: Move 3 distribution
    { "name": "axon-driven-development", "version": "1.0.0", "url": "bundle:skills/axon-driven-development.tar.gz" },
    { "name": "grill-me", "version": "1.0.0" }
    // …
  ]
}
```

### 2.3 MCP tool d'application

Nouveau tool `mcp__axon__axon_apply_methodology_bundle` :

```
Inputs:
  bundle_path: str (filesystem path to methodology-X.Y.Z.json)
  dry_run: bool = false
  force: bool = false (skip checksum / version checks — admin only)
  apply_skills: bool = true (call skills installer if skills_manifest present)

Outputs:
  status: ok | input_invalid | partial | refused
  applied:
    pillars: { created: N, updated: N, skipped: N }
    concepts: { ... }
    guidelines: { ... }
    decisions: { ... }
    relations: { ... }
  revision_id: "<uuid>" (single SOLL revision wraps entire apply)
  parameter_repair: { ... } on failure
```

**Idempotence** : logical_keys + canonical_id_hints permettent re-apply sans dupliquer. La re-application d'un bundle déjà appliqué = no-op + status='ok'.

**Auto-apply trigger** : au premier `axon_init_project` quand `project_code=PRO` est requis pour `INHERITS_FROM` mais PRO n'a aucun GUI-PRO/CPT-PRO en base, Axon trigger automatiquement l'apply du bundle shipped (chemin par défaut `data/methodology/methodology-current.json` dans le binaire ou `${AXON_METHODOLOGY_BUNDLE}` env var).

---

## 3. Contenu méthodologique v1.0.0

### 3.1 Pillars (4 nouveaux — PIL-PRO-001..004)

Format : `<scope>. Spans: <enumeration of contained guidelines>.`

#### PIL-PRO-001 — Code Quality

> Architectural discipline ensuring code is testable, deep, well-bounded, free of warnings. Spans: test-first development (GUI-001), DRY/SRP/KISS/cognitive-limits/clean-as-you-go (GUI-013/014/015/016/017), APoSD foundations — deep modules, information hiding, pull-complexity-downwards, design-it-twice (GUI-018/019/020/021). Consumer project GUI-{code}-N covering same scope INHERITS_FROM corresponding GUI-PRO.

#### PIL-PRO-002 — Reliability & Operations

> Runtime guarantees + observable behavior under production load. Spans: fail-fast + zero-warning (GUI-003), zero-mock I/O verification (GUI-004), control-vs-data-plane separation (GUI-005), deterministic hermetic builds (GUI-006), native structured telemetry (GUI-007), failure-resilience-by-design (GUI-008), performance-as-native-property (GUI-009), shift-left-security + least-privilege (GUI-010), infrastructure-as-code reproducibility (GUI-012).

#### PIL-PRO-003 — Workflow Discipline

> Process methodology for LLM-assisted development. Spans: MCP-driven documentation (GUI-002), accessibility + cognitive ergonomics (GUI-011), and Pocock-derived patterns — design-tree interview (GUI-022), vertical-slice decomposition (GUI-023), PRD synthesis (GUI-024), throwaway prototype (GUI-025), Bootstrap/Continuation phase detection (GUI-026), handoff discipline (GUI-028), diagnose loop (GUI-030).

#### PIL-PRO-004 — Resource Economy

> Token/cache/context budget management for cost-effective LLM operation. Spans: sub-agent token economy with MCP-first main thread (GUI-027), cache-TTL aware end-to-end execution avoiding mid-task interrupts (GUI-029). Foundational for commercial viability of Axon-methodology workflow.

### 3.2 Concepts (4 nouveaux — promotions de CPT-AXO méthodologiques)

Format : `<rule + mechanism>. <quantitative or workflow detail>. <triggers/anti-patterns>. Generalization of CPT-AXO-XXX.`

#### CPT-PRO-004 — SOLL Operational Protocol — observe, log, link, re-plan, execute

> PDCA loop applied to canonical intent. (P) Plan = research SOLL+IST via `status`/`project_status`/`soll_query_context` BEFORE code; create REQ/DEC with `soll_manager link` to Pillar/Concept. (D) Do = execute highest-score wave-1 from `soll_work_plan`, one fix one commit, `axon_pre_flight_check` → `axon_commit_work`. (C) Check = run tests, query live MCP status (don't trust conversation context — lossy on compaction), cross-check SOLL acceptance criteria. (A) Act = `soll_manager update` REQ status + commit SHA + `soll_attach_evidence`, `soll_validate` target 0, `soll_work_plan` next. Generalization of CPT-AXO-019.

#### CPT-PRO-005 — LLM onboarding loop with Axon-equipped MCP

> 6-phase canonical loop for fresh LLM session : (1) probe MCP server reachability (curl tools/list); (2) `axon_init_project` (project_code auto-resolved from cwd); (3) read `kickoff_bundle.session_pointer` (kind ∈ file|url|soll_node|none) — apply pointed artifact BEFORE anything else; (4) `wave_1_unblockers` via `soll_work_plan top=3`; (5) `recent_req_commits` + `recent_soll_writes` for activity baseline; (6) first mutation = SOLL (REQ/DEC create/update) BEFORE code mutation. Generalization of CPT-AXO-020.

#### CPT-PRO-006 — LLM-only documentation methodology — SKILL/SOLL/MEMORY triad

> Three complementary surfaces, never duplicated, read in order at startup: (1) SKILL.md = machine-actionable contract (tool routing, recovery shapes, hygiene rules, examples); (2) SOLL = canonical mental models (CPT/GUI/DEC) + persistent intent (VIS/PIL/REQ/VAL); (3) MEMORY = operator preferences/feedback persisting across sessions. Triad enforces : SKILL describes HOW, SOLL describes WHAT/WHY, MEMORY describes WHO. Mutation paths : `soll_manager` for SOLL, file write for SKILL/MEMORY. Detection: same fact duplicated in SKILL+SOLL → consolidate to SOLL canonical, SKILL becomes pointer. Generalization of CPT-AXO-024.

#### CPT-PRO-007 — 3-way diagnostic triage — hallucination / real bug / commercial value-add

> Every unexpected MCP/runtime/code/doc deviation classifies into ONE branch before any logging : (1) HALLUCINATION = I assumed unverified column/type/param/behavior → positive control + `schema_overview` + 3 controlled repros; if explained → drop, log nothing. (2) REAL BUG = reproducible failure contradicts written contract (SKILL.md / SOLL DEC/REQ / tool description) → `soll_manager create requirement` tagged `axon-bug`+`llm-contract` with evidence = repros + schema check + positive control. (3) COMMERCIAL VALUE-ADD = works per doc but underperforms commercially (clarity, structured field, discoverability, recovery hint) → `soll_manager create requirement` tagged `axon-product-improvement`+`commercial-value`+`llm-friction`, framed as customer value (productivity gain, time saved, error avoided). NEVER log without explicit branch choice. Generalization of CPT-AXO-025.

### 3.3 Guidelines (9 nouveaux — workflow Pocock + ressource economy)

Format : `<trigger condition>. <prescription>. <detection skip or quantitative measure>. Skill: /<name>. Anchor: <book/REQ/CPT>.`

#### GUI-PRO-022 — Design-tree interview discipline

> Pre-commit on any design : walk decision-tree branch-by-branch, one question per turn, recommendation+alternatives per question, dependencies resolved depth-first. Stop only at shared-understanding. Detection skip : LLM produces plan/spec doc before 5+ Q/A turns. Skill: /grill-me. Anchor: Brooks Design of Design ch.3.

#### GUI-PRO-023 — Vertical-slice tracer-bullet decomposition

> Break feature into REQ-children REFINES that cut UI→API→storage→tests vertically, never horizontally per layer. Each slice independently demoable + flushes unknowns first. First slice MUST integrate all integration boundaries. Detection bad : REQ-A "build storage layer" then REQ-B "build API on storage". Skill: /to-issues-soll. Anchor: Hunt & Thomas Pragmatic Programmer ch.7.

#### GUI-PRO-024 — PRD synthesis pattern

> Feature destination doc = problem_statement + solution_architecture + user_stories[] + acceptance_criteria[] + implementation_decisions (non-prescriptive + durable). Persist as REQ-{code}-N umbrella status='current' priority + acceptance_criteria in body, sub-REQs REFINES umbrella. Detection skip : solution articulated before problem. Skill: /to-prd-soll.

#### GUI-PRO-025 — Throwaway prototype de-risking

> Validate design or data model unknowns via disposable prototype (standalone script, terminal demo, N route variants) before commit. Max 1-3 days. NEVER merged to main. Required when : new integration, new tech stack, ≥2 viable designs with empirical falsifier. Skill: /prototype. Anchor: Brooks Mythical Man-Month "plan to throw one away".

#### GUI-PRO-026 — Bootstrap-vs-Continuation phase detection

> At `axon_init_project` : VIS-{P}-001 absent → `kickoff_bundle.bootstrap_required=true` + `input_documents[]` scan (README/vision/brief/PRD/CONTEXT/*.md depth=1). LLM enters cascade grill-me Vision→Pillars→Concepts→Decisions. VIS present → Continuation flow (REQ umbrella → REFINES children → tdd). No mixed mode. Skill: /bootstrap-soll vs /to-prd-soll.

#### GUI-PRO-027 — Token-economy sub-agent policy

> Main-thread MCP-first for IST/SOLL queries (cost: ~5-50 tokens via query/inspect). Sub-agents allowed for : external research, doc-scan, closed-brief parallelism, MCP-independent tasks. MCP-needing sub-agents → `./scripts/axon mcp-call` CLI bridge. FORBIDDEN: sub-agent forced IST reconstruction via re-read source (cost: 100-200K tokens wasted). Skill: /improve-codebase-architecture-soll.

#### GUI-PRO-028 — Handoff document discipline

> Session end = canonical SOLL session_pointer (`CPT-{code}-N` status='current' kind='session_pointer'). Body: runtime state, branch+HEAD, REQs in-progress, next actions, blockers. Markdown fallback only when MCP unrecoverable. Surfaced via `axon_init_project.kickoff_bundle.session_pointer`. Detection skip : session ends without pointer update. Skill: /handoff.

#### GUI-PRO-029 — Cache-TTL economics & end-to-end execution

> Anthropic prompt cache TTL = 5 minutes. Any pause > TTL refactures full context (~$0.05–2 per pause). Auto/continuous mode : execute plan start→finish single burst. NO intermediate "should I continue?" / "here's progress so far" / mid-plan reports. Single terse final summary 1-3 sentences. Stop ONLY on : (i) genuine blocker no reasonable default, (ii) destructive-irreversible action requiring confirmation (rm -rf, force-push, drop table, mass SOLL delete, AGE_READ default flip), (iii) hard external blocker. Detection bad: LLM asks operator confirmation on routine reversible engineering choices.

#### GUI-PRO-030 — Diagnose loop discipline

> Bug or perf regression : (1) reproduce minimally, (2) hypothesize falsifiable cause, (3) instrument with 1-line env flag if possible, (4) fix, (5) regression-test. Failed hypothesis still produces VAL-{code}-N (VERIFIES or REJECTS REQ with evidence). Skip cargo-cult fixes ("try X first"). Detection bad: "should fix" without repro. Skill: /diagnose. REFINES CPT-PRO-004 PDCA.

### 3.4 Decisions (1 existante à confirmer)

DEC-PRO-001 "BOOTSTRAP PROMPT - read first, every Axon-equipped LLM session" — déjà en base, à laisser intacte. Le bundle v1.0.0 la **régularise** (logical_key + canonical_id_hint) sans modifier le body.

### 3.5 Mapping BELONGS_TO theming (21+9 GUI-PRO → 4 PIL-PRO)

| PIL-PRO | GUI-PRO inclus |
|---|---|
| PIL-PRO-001 Code Quality | GUI-PRO-001, 013, 014, 015, 016, 017, 018, 019, 020, 021 |
| PIL-PRO-002 Reliability & Ops | GUI-PRO-003, 004, 005, 006, 007, 008, 009, 010, 012 |
| PIL-PRO-003 Workflow Discipline | GUI-PRO-002, 011, 022, 023, 024, 025, 026, 028, 030 |
| PIL-PRO-004 Resource Economy | GUI-PRO-027, 029 |

⚠️ Question ouverte (cf §6) : GUI → PIL via BELONGS_TO est-il dans le schéma canonique de relations ? À vérifier via `soll_relation_schema` avant apply.

### 3.6 Cross-project INHERITS_FROM (méthodologie Axon-side)

| Source (existant) | Target (nouveau) | Relation |
|---|---|---|
| CPT-AXO-019 | CPT-PRO-004 | INHERITS_FROM |
| CPT-AXO-020 | CPT-PRO-005 | INHERITS_FROM |
| CPT-AXO-021 | DEC-PRO-001 | INHERITS_FROM |
| CPT-AXO-024 | CPT-PRO-006 | INHERITS_FROM |
| CPT-AXO-025 | CPT-PRO-007 | INHERITS_FROM |

CPT-AXO-018 ("MCP and runtime LLM-contract hygiene") **reste AXO-only** — c'est spécifique à l'infrastructure MCP d'Axon, pas méthodologie cross-project.

### 3.7 Propagation aux 4 consumer projects (FSF/MLD/NEX/AXO)

Pattern existant pour les 21 GUI-PRO actuels : chaque consumer project a son `GUI-{CODE}-N INHERITS_FROM GUI-PRO-N`. Pour les 9 nouveaux GUI-PRO :

Soit (i) **propagation automatique** dans le bundle apply tool : si un consumer project a déjà du INHERITS_FROM sur d'autres GUI-PRO, propager pour les nouveaux. Soit (ii) **propagation manuelle** par projet (consumer décide de s'aligner ou non).

Recommandation : (ii) — chaque consumer project explicitement opt-in via son propre kickoff. Évite l'effet "le bundle Axon m'a injecté des règles que je ne voulais pas".

---

## 4. Migration plan (régularisation PRO + apply v1.0.0)

### 4.1 Step 0 — Registry-level régularisation

Choix de chemin filesystem pour PRO :
- **Option A** (recommandée) : créer `~/projects/axon-methodology/` avec `README.md` + `methodology-1.0.0.json` + futur `CHANGELOG.md`. Sibling d'Axon. Représente concrètement le "bundle source".
- Option B : utiliser `~/projects/axon/data/methodology/` (sous-dir Axon). Conflate avec AXO project.
- Option C : path virtuel `data.path_exists_on_disk=false` → registration succeeds mais opérations filesystem échouent.

Action : `mcp__axon__axon_init_project project_path=~/projects/axon-methodology project_code=PRO`.

### 4.2 Step 1 — Apply v1.0.0 bundle

Tant que le MCP tool `axon_apply_methodology_bundle` n'est pas implémenté, l'apply se fait via `mcp__axon__soll_apply_plan` en 3 calls :

1. **Call 1** (project_code=PRO) : `pillars: [4]` + `concepts: [4]` + `relations: [theming BELONGS_TO]`
2. **Call 2** (project_code=PRO) : pour les guidelines (non supportées par soll_apply_plan), boucle de 9 `soll_manager(action=create, entity=guideline)`
3. **Call 3** (project_code=AXO ou neutre) : 5 INHERITS_FROM cross-project via `soll_manager(action=link)` itéré

### 4.3 Step 2 — Régularisation des 21 GUI-PRO existants

Les 21 GUI-PRO sont déjà actifs en base. La régularisation = ajout de :
- Métadonnée `logical_key` cohérente avec le bundle (pour idempotence future)
- BELONGS_TO PIL-PRO theming

Via `soll_manager(action=update)` + `soll_manager(action=link)`.

### 4.4 Step 3 — Validation finale

- `soll_validate project_code=PRO` → 0 violations
- `cypher SELECT count(*) FROM soll.Node WHERE project_code='PRO'` → ≥38 (4 PIL + 7 CPT [4 new + 3 placeholders] + 30 GUI [21+9] + 1 DEC = 42)
- `cypher SELECT count(*) FROM soll.Edge WHERE source_id LIKE 'CPT-AXO-%' AND relation_type='INHERITS_FROM'` → ≥5

---

## 5. REQ-AXO breakdown pour implémentation

Umbrella :

### REQ-AXO-NNN-A — Commercialize Axon-methodology delivery layer

Priority: P1. Status: planned. Pillar: PIL-AXO-001 (Stabilisation) ou nouveau pillar commercial.

Description: "Livrer Axon comme produit méthodologique. Bundle JSON versionné, MCP tool d'apply idempotent, auto-apply au premier init projet consommateur, distribution skills front-door. Référence: working-note 2026-05-11-axon-methodology-delivery-spec.md."

Acceptance criteria:
1. PRO enregistré dans ProjectCodeRegistry avec project_path canonique
2. `methodology-1.0.0.json` bundle écrit + checksum
3. MCP tool `axon_apply_methodology_bundle` opérationnel (idempotent + dry_run)
4. Auto-apply trigger au premier `axon_init_project` quand PRO empty
5. 6 SKILL.md consumer-facing créés
6. `axon-engineering-protocol/SKILL.md` mis à jour avec section consumer-vs-internal
7. `soll_validate project_code=PRO` retourne 0 violations
8. Documentation : `docs/methodology/README.md` + `docs/methodology/CHANGELOG.md`

### Children REQs

| ID logique | Title | Priority | Blocks |
|---|---|---|---|
| REQ-AXO-NNN-B | Regularize PRO in ProjectCodeRegistry | P1 | C,D,E,F,G,H |
| REQ-AXO-NNN-C | Write canonical `methodology-1.0.0.json` bundle | P1 | E,G |
| REQ-AXO-NNN-D | Implement `mcp__axon__axon_apply_methodology_bundle` MCP tool | P1 | E,G |
| REQ-AXO-NNN-E | Apply v1.0.0 bundle to live SOLL (4 PIL + 4 CPT + 9 GUI + 5 INHERITS_FROM) | P1 | F |
| REQ-AXON-NNN-F | Server: `axon_init_project` returns `bootstrap_required` + `input_documents[]` | P2 | (consumer-facing) |
| REQ-AXO-NNN-G | Create 6 consumer-facing SKILL.md (axon-driven-development umbrella + 5 sub-skills) | P2 | — |
| REQ-AXO-NNN-H | Update `axon-engineering-protocol/SKILL.md` with consumer-vs-internal sections + new IDs | P2 | E |
| REQ-AXO-NNN-I | Skills bundle distribution mechanism design (Move 3 — git submodule? installer?) | P3 | — |

---

## 6. Questions ouvertes / décisions différées

1. **Canonical relation GUI→PIL via BELONGS_TO** : à confirmer via `soll_relation_schema`. Si non canonique, ajouter via REQ-AXO ou utiliser `EXPLAINS` (CPT→PIL existe via BELONGS_TO selon `axon-engineering-protocol/SKILL.md`).

2. **Canonical relation CPT→CPT et CPT→DEC via INHERITS_FROM** : à confirmer. Pattern observé pour GUI→GUI mais non documenté pour autres types. Si non supporté, fallback : `REFINES` (DEC→CPT existe canoniquement).

3. **Skills distribution (Move 3)** : 3 options à arbitrer
   - git submodule de `axon-methodology-skills` dans `~/.claude/skills/`
   - script d'installation `axon-install-skills` qui télécharge depuis registry HTTPS
   - Claude Code skill plugin mechanism (si Anthropic le release — wait & see)

4. **Propagation cross-project des 9 nouveaux GUI-PRO** : opt-in manuel par projet (recommandé) vs auto-propagation dans le bundle apply tool. Décision pour REQ-AXO-NNN-G.

5. **Versioning des bundle** : semver strict ? Tagging git ? Comment communiquer les breaking changes aux clients ?

6. **Backward compatibility de l'inheritance** : si v1.1.0 supprime GUI-PRO-XXX, les consumer projects ayant GUI-{CODE}-N INHERITS_FROM GUI-PRO-XXX deviennent orphelins. Spec de migration nécessaire.

7. **Format LLM-optimisé** : on a convergé vers `<trigger>. <prescription>. <detection/measure>. Skill: /<name>. Anchor: <ref>.` Faut-il un linter `methodology-lint` qui valide ce format sur chaque GUI nouveau ?

8. **CPT-PRO-001/002/003 (placeholders "Synthetic MCP validation concept")** : laisser en l'état (status="" ne perturbe rien) ou archiver explicitement (status="archived") pour éviter confusion future ?

---

## 7. Synthèse 14-questions grilling (source de vérité conversationnelle)

| # | Question | Décision |
|---|---|---|
| Q1 | Niveau d'ambition | Codifier philosophie en SOLL + skills front-door |
| Q2 | Surface de livraison | GUI-PRO + SKILL.md dédié (pattern existant) |
| Q3 | Project code | PRO pour tout nouveau + migration progressive CPT-AXO |
| Q4 | Vague | Big bang (tous concepts en une livraison) |
| Q5 | PRD→SOLL | Modèle α : REQ-umbrella + REQ-children REFINES |
| Q6 | Détection Bootstrap/Continuation | Auto-server via présence VIS-{P}-001 |
| Q7 | Umbrella SKILL.md | Nouvelle `axon-driven-development` (axon-engineering-protocol reste Axon-internal) |
| Q8 | Granularité Bootstrap | Session unique cascade grill-me Vision→Pillars→Concepts→Decisions |
| Q9 | Sub-agents | Token-economy policy (MCP-first main + sub-agents OK research/closed-brief) |
| Q10 | improve-codebase-architecture | Hybride : main MCP discovery → brief clos → 3 sub-agents → main synthèse |
| Q11 | Inventaire initial | Big-bang validé puis révisé après découverte 21 GUI-PRO existants |
| Q12 | Périmètre révisé | 4 CPT-PRO + 9 GUI-PRO + 4 PIL-PRO + edges + skills + server REQ |
| Q13 | Format & structure | Pillars thematic + LLM-optimized dense descriptions |
| Q14 | Modèle livraison commercial | γ seul (bundle versionné sans system_managed) |

---

## 8. Prochaines actions concrètes (next session)

1. Lire cette spec en intégralité avant tout SOLL write
2. Confirmer questions ouvertes §6 (notamment GUI→PIL BELONGS_TO et CPT→CPT INHERITS_FROM via `soll_relation_schema`)
3. Exécuter Step 0 §4.1 (`axon_init_project` pour PRO)
4. Créer REQ-AXO-NNN-A umbrella + 8 children REQs via `soll_apply_plan` (project_code=AXO)
5. Exécuter `soll_work_plan project_code=AXO top=3` pour planifier wave 1
6. Itérer PDCA : Plan (REQ) → Do (commit) → Check (tests + soll_validate) → Act (update status + evidence)

**Ne pas** : tenter d'apply le bundle complet en une session. C'est un chantier multi-sessions ; chaque REQ-child = ~30-150 LOC + tests + SKILL.md update + commit.

---

**Fin de spec.** Mise à jour de cette doc autorisée pour clarifications mineures ; révisions majeures → nouvelle working-note datée.
