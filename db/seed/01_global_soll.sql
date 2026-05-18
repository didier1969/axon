-- DEC-AXO-082 seed half — canonical SOLL seed for the cross-tenant `PRO`
-- namespace. Applied via `psql -f` on every runtime startup (after
-- db/ddl/*.sql DDL files) by scripts/lib/ensure-runtime.sh
-- `apply_canonical_seed`. Each statement is idempotent (`ON CONFLICT DO
-- NOTHING`) so re-running on a warm DB is a few-ms no-op.
--
-- Scope : everything currently held in the `PRO` namespace (cross-tenant
-- methodology surface of Axon-produit per Pillar PIL-AXO-9003 Two-Sided
-- Identity) :
--   - 1 ProjectCodeRegistry row (PRO sentinel)
--   - 1 soll.Registry counters row (PRO namespace)
--   - 49 soll.Node rows : 5 PIL-PRO + 8 CPT-PRO + 3 DEC-PRO + 33 GUI-PRO
--   - 43 soll.Edge rows : cross-namespace BELONGS_TO / INHERITS_FROM /
--     EPITOMIZES / EXPLAINS / etc. that connect PRO methodology to AXO
--     and other consumer projects
--
-- Retires :
--   - `graph_bootstrap::seed_project_code_registry` (was Rust-hardcoded PRO row)
--   - `graph_bootstrap::seed_global_guidelines` (was Rust-hardcoded ~20 GUI-PRO entries)
--
-- The matching Rust functions in graph_bootstrap.rs are stubbed to single
-- `info!` log entries per DEC-AXO-082 consequence (function signatures
-- retained for binary-API stability ; bodies retired).
--
-- Regeneration : when PRO data changes (e.g. new GUI-PRO added via
-- soll_manager mutations in axon-projet dogfood), regenerate this file
-- via `scripts/seed/regenerate-pro-seed.sh` (future tooling, REQ to file).
-- Currently regenerated manually via psql format() generator
-- (see /tmp/gen_pro_seed.sql in session 45 git history if needed).
--
-- REQ-AXO-91577 (PRO sentinel unblock) + DEC-AXO-082 seed half delivery.

-- ProjectCodeRegistry section
INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name, session_pointer_json)
VALUES ('PRO', '(sentinel:cross-project-methodology)', 'System Global Namespace', NULL)
ON CONFLICT (project_code) DO NOTHING;

-- soll.Registry seed counters (per-namespace counter init)
INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_prv, last_rev)
VALUES ('PRO', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
ON CONFLICT (project_code) DO NOTHING;

-- PRO Nodes (Pillars, Concepts, Decisions, Guidelines)
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('CPT-PRO-001', 'Concept', 'PRO', 'MCP Validate Concept', 'Synthetic MCP validation concept', 'superseded', '{"rationale": "Validation-only concept outside AXO scope", "updated_at": 1778514540641, "archive_reason": "Synthetic MCP validation placeholder — superseded by canonical CPT-PRO-004..007. REQ-AXO-273 methodology track 2026-05-11."}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('CPT-PRO-002', 'Concept', 'PRO', 'MCP Validate Concept', 'Synthetic MCP validation concept', 'superseded', '{"rationale": "Validation-only concept outside AXO scope", "updated_at": 1778514541799, "archive_reason": "Synthetic MCP validation placeholder — superseded by canonical CPT-PRO-004..007. REQ-AXO-273 methodology track 2026-05-11."}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('CPT-PRO-003', 'Concept', 'PRO', 'MCP Validate Concept', 'Synthetic MCP validation concept', 'superseded', '{"rationale": "Validation-only concept outside AXO scope", "updated_at": 1778514543193, "archive_reason": "Synthetic MCP validation placeholder — superseded by canonical CPT-PRO-004..007. REQ-AXO-273 methodology track 2026-05-11."}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('CPT-PRO-004', 'Concept', 'PRO', 'SOLL Operational Protocol — observe, log, link, re-plan, execute', 'PDCA loop applied to canonical intent. (P) Plan = research SOLL+IST via `status`/`project_status`/`soll_query_context` BEFORE code; create REQ/DEC with `soll_manager link` to Pillar/Concept. (D) Do = execute highest-score wave-1 from `soll_work_plan`, one fix one commit, `axon_pre_flight_check` → `axon_commit_work`. (C) Check = run tests, query live MCP status (don''t trust conversation context — lossy on compaction), cross-check SOLL acceptance criteria. (A) Act = `soll_manager update` REQ status + commit SHA + `soll_attach_evidence`, `soll_validate` target 0, `soll_work_plan` next. Generalization of CPT-AXO-019.', 'current', '{"anchor": "Deming PDCA + Pocock /tdd", "updated_at": 1778514326346, "supersedes_pattern": "CPT-AXO-019"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('CPT-PRO-005', 'Concept', 'PRO', 'LLM onboarding loop with Axon-equipped MCP', '6-phase canonical loop for fresh LLM session: (1) probe MCP server reachability (curl tools/list); (2) `axon_init_project` (project_code auto-resolved from cwd); (3) read `kickoff_bundle.session_pointer` (kind ∈ file|url|soll_node|none) — apply pointed artifact BEFORE anything else; (4) `wave_1_unblockers` via `soll_work_plan top=3`; (5) `recent_req_commits` + `recent_soll_writes` for activity baseline; (6) first mutation = SOLL (REQ/DEC create/update) BEFORE code mutation. Generalization of CPT-AXO-020.', 'current', '{"updated_at": 1778514326835, "supersedes_pattern": "CPT-AXO-020"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('CPT-PRO-006', 'Concept', 'PRO', 'LLM-only documentation methodology — SKILL/SOLL/MEMORY triad', 'Three complementary surfaces, never duplicated, read in order at startup: (1) SKILL.md = machine-actionable contract (tool routing, recovery shapes, hygiene rules, examples); (2) SOLL = canonical mental models (CPT/GUI/DEC) + persistent intent (VIS/PIL/REQ/VAL); (3) MEMORY = operator preferences/feedback persisting across sessions. Triad: SKILL describes HOW, SOLL describes WHAT/WHY, MEMORY describes WHO. Mutation paths: `soll_manager` for SOLL, file write for SKILL/MEMORY. Detection: same fact duplicated in SKILL+SOLL → consolidate to SOLL canonical, SKILL becomes pointer. Generalization of CPT-AXO-024.

Density principles (token-efficient LLM consumption):
- Signal/token max: prose forbidden when schema/regex/table/example suffices.
- Future utility: nothing kept for history alone; revisions/git carry the timeline.
- Graph-as-index: structure (type/status/edges) IS the index, not tags/prefixes/strings.
- Lifecycle compression: post-delivery nodes = thin pointer; full intent lives in the final Revision.

Actionable rules: GUI-PRO-100. Canonical status vocabulary: DEC-PRO-100 (5 values).', 'current', '{"updated_at": 1778761091891, "supersedes_pattern": "CPT-AXO-024"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('CPT-PRO-007', 'Concept', 'PRO', '3-way diagnostic triage — hallucination / real bug / commercial value-add', 'Every unexpected MCP/runtime/code/doc deviation classifies into ONE branch before any logging: (1) HALLUCINATION = I assumed unverified column/type/param/behavior → positive control + `schema_overview` + 3 controlled repros; if explained → drop, log nothing. (2) REAL BUG = reproducible failure contradicts written contract → `soll_manager create requirement` tagged `axon-bug`+`llm-contract` with evidence = repros + schema check + positive control. (3) COMMERCIAL VALUE-ADD = works per doc but underperforms commercially (clarity, structured field, discoverability, recovery hint) → `soll_manager create requirement` tagged `axon-product-improvement`+`commercial-value`+`llm-friction`, framed as customer value. NEVER log without explicit branch choice. Generalization of CPT-AXO-025.', 'current', '{"updated_at": 1778514327792, "supersedes_pattern": "CPT-AXO-025"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('CPT-PRO-099', 'Concept', 'PRO', 'Universal concept', 'cross-project mental model', 'current', '{}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('DEC-PRO-001', 'Decision', 'PRO', 'Bootstrap protocol for cross-project LLM sessions', 'Bootstrap protocol for cross-project LLM sessions using Axon MCP (referenced as `kickoff_prompt` source by `axon_init_project.data.kickoff_bundle`).

STEP 0 — Pre-init MCP probe with auto-recovery (DEC-AXO-060):
- Probe: `curl -fs --max-time 2 -X POST http://127.0.0.1:44129/mcp -H "Content-Type: application/json" -d ''{"jsonrpc":"2.0","method":"tools/list","id":1}''`
- On failure: `./scripts/axon-live stop --hard 2>/dev/null; ./scripts/axon-live start --brain-only` ; wait `pgrep -f bin/axon-brain`.
- LLM-agnostic (Claude / Codex / Gemini).

STEP 1 — First MCP call: `mcp__axon__axon_init_project project_path=<cwd>` (REQ-AXO-119). Read `data.kickoff_bundle` (kickoff_prompt, methodology_summary, entry_points, session_pointer, in_progress_requirements, wave_1_unblockers, recent_req_commits, recent_soll_writes, bootstrap_required, input_documents — REQ-AXO-176/178/278).

STEP 2 — Operational loop (CPT-AXO-019 / CPT-PRO-006 SKILL/SOLL/MEMORY triad): observe → `soll_manager` log → `soll_manager link` → `soll_work_plan` re-eval → execute wave-1. Mid-task triage per CPT-AXO-025 (hallucination / Axon bug / commercial value-add).

STEP 3 — Relation contract for SOLL deltas:
- `DEC -SOLVES/REFINES-> REQ` (REQ-AXO-179: soll_validate output advertises IMPACTS but runtime allows SOLVES/REFINES only).
- `REQ -BELONGS_TO-> PIL`.
- All edges via `soll_manager(action=link)`.

[RECONSTRUCTED 2026-05-14 from cross-file references after data-loss incident REQ-AXO-323; original detailed text not preserved. Operator validation pending.]', 'current', '{"updated_at": 1778761045492, "reconstructed": true, "restoration_note": "Recovered after overwrite incident 2026-05-14 (REQ-AXO-323)", "reconstruction_sources": ["~/.claude/CLAUDE.md", "CPT-AXO-021", "REQ-AXO-149 metadata", "REQ-AXO-179 description", "workflow_project.rs default_kickoff_prompt"]}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('DEC-PRO-099', 'Decision', 'PRO', 'Cross-project canonical decision', 'body', 'current', '{"rationale": "R"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('DEC-PRO-100', 'Decision', 'PRO', 'Canonical SOLL status vocabulary — 5 values', 'Canonical SOLL `node.status` vocabulary = 5 values:
- `current`: actively owned, work in progress
- `planned`: intent recorded, not yet started
- `delivered`: terminal success
- `superseded`: replaced by another node (MUST have outgoing SUPERSEDES edge)
- `rejected`: terminal failure / abandoned

## Enforcement layers (applied 2026-05-14 on live PG)

1. **DB CHECK constraint** `soll_node_status_canonical CHECK (status IN (''current'',''planned'',''delivered'',''superseded'',''rejected'')) NOT VALID` — enforces all new INSERT/UPDATE. Legacy rows with non-canonical status (147 active + 76 empty + 51 accepted + 24 completed + 5 archived + 3 open + 3 in_progress + 1 draft + 1 done across non-AXO projects) intact until their own curate-soll run.

2. **DB DEFAULT `current`** — `node.status` defaults to `current` if not specified. Rationale: a node being created reflects an actively owned intent, not a deferred-start.

3. **Server validation (pending)** — `soll_manager.create/update` should validate status server-side BEFORE the DB rejects, returning LLM-friendly `data.parameter_repair` envelope. Tracked in REQ-AXO-325. Until shipped, raw DB error surfaces.

## Normalization mapping for legacy data

- completed/done/passed/closed/archived → delivered (or superseded if SUPERSEDES edge exists)
- accepted/in_progress/active/open/proposed/partial/pending → current or planned (per activity evidence)
- failed → rejected
- empty/null → reclassify via curate-soll pass_T

Enforced by GUI-PRO-100.', 'current', '{"updated_at": 1778763657009}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-001', 'Guideline', 'PRO', 'TDD Obligatoire', 'Les tests doivent être écrits avant ou avec le code source.', 'current', '{"phase": "pre-code", "updated_at": 1779040233475, "enforcement": "strict", "trigger_path": "src/axon-core/src/*", "required_path": "tests.rs", "restoration_note": "Recovered after overwrite incident 2026-05-14 (REQ-AXO-323)", "exempt_for_refactor": true, "restored_from_export": "SOLL_EXPORT_2026-05-08_150409_399.md"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-002', 'Guideline', 'PRO', 'Documentation MCP', 'Toute modification de `src/mcp/tools_*.rs` ou des contrats MCP (input schema, output envelope, error shapes) déclenche obligatoirement la mise à jour du SKILL.md correspondant (`docs/skills/axon-engineering-protocol/SKILL.md` pour le repo Axon, skill consommateur pour les autres projets).

## Process canonique

1. **Avant l''édition** : invoquer le skill `writing-skills` (review TDD pour documentation). Règle : SKILL.md = LLM-contract uniquement (reference guide), pas de narrative.
2. **Pendant l''édition** : respecter `GUI-PRO-100` (token-efficient writing) :
   - prose interdite si schema/regex/table/exemple suffit
   - aucune mention `recent / latest / set 20XX-XX-XX / observed during` (→ git log + soll.Revision)
   - aucune duplication d''info dérivable d''un mécanisme natif (Edges, IST query, Revisions). Références croisées `REQ-XXX-N` / `DEC-XXX-N` autorisées comme **pointers** — le SOLL node porte le contenu, le SKILL le cite.
   - post-delivery → thin pointer ; rich detail vit dans la `Revision` finale
3. **Après l''édition** : `axon_pre_flight_check diff_paths=[<tools_*.rs>, <SKILL.md>]` valide la cohérence avant commit.
4. **Auto-curation continue** : `/curate-soll` pass_D détecte les drifts (op-log creep, prose density, dates inline) et compresse à chaque fin de session ou sur demande.

## Anti-patterns spécifiquement interdits

- Section `## Tool contract changes (recent)` ou tout changelog inline
- Notes d''incident datées (`observed 20XX-XX-XX promotion`) mêlangées à des règles atemporelles
- `previous version was ...` ou récit de session conversationnel
- Listes de fichiers source dérivables via `query`/`inspect` IST
- Acceptance criteria en prose (→ `VAL` node + edge `VERIFIES`)

## Découvrabilité

La table `Search recovery` dans `axon-engineering-protocol/SKILL.md` documente toutes les catégories `parameter_repair` que les tools MCP exposent. Chaque ajout d''envelope (entity / project_code / relation_type / status / etc.) ajoute une row — contrat LLM stable, recouvrement en un round-trip.

Inherits-from GUI-PRO-100 (token-efficient writing). Enforced by `axon_pre_flight_check` Documentation MCP gate.', 'current', '{"phase": "post-code", "updated_at": 1779040234573, "enforcement": "strict", "trigger_path": "src/axon-core/src/mcp/tools_*", "required_path": "SKILL.md", "exempt_for_refactor": true}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-003', 'Guideline', 'PRO', 'Zéro Warning & Fail-Fast', 'Tout code doit compiler et passer l''analyse statique avec formellement zéro avertissement (ex: deny(warnings) en Rust, --strict en TS). La CI doit échouer immédiatement au premier avertissement détecté.', 'current', '{"phase": "compile", "enforcement": "strict", "trigger_path": "*"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-004', 'Guideline', 'PRO', 'Vérité Physique (Zéro Mock I/O)', 'Interdiction stricte d''utiliser des mocks ou stubs pour simuler les entrées/sorties (Réseau, FS, DB). Les tests d''intégration doivent instancier des ressources physiques isolées et éphémères (ex: DB temporaires sur disque) pour valider les comportements réels (verrous, WAL, concurrence).', 'current', '{"phase": "test", "enforcement": "strict", "trigger_path": "*"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-005', 'Guideline', 'PRO', 'Séparation des Plans (Control vs Data Plane)', 'Isolation architecturale obligatoire entre les processus gérant l''état/routage (Control Plane, asynchrone, faible latence) et les processus exécutant les calculs lourds ou la logique métier complexe (Data Plane, synchrone, intensif). Le Control Plane ne doit exécuter aucune logique bloquante.', 'current', '{"phase": "architecture", "enforcement": "strict", "trigger_path": "*"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-006', 'Guideline', 'PRO', 'Builds Déterministes & Hermétiques', 'La compilation d''un commit doit produire un artefact dont l''empreinte (SHA-256) est strictement identique partout (Tolérance 0%). 100% des dépendances (système et applicatives) doivent être épinglées via un fichier de verrouillage avec hash cryptographique. Le build doit réussir en isolation réseau (Air-Gap).', 'current', '{"phase": "build", "enforcement": "strict", "trigger_path": "*"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-007', 'Guideline', 'PRO', 'Télémétrie Structurée Native', '100% des événements applicatifs doivent être émis au format structuré (JSON/OTLP). Interdiction absolue des logs textuels bruts sur stdout nécessitant un parsing par regex. Propagation obligatoire des trace_id dans tous les appels RPC/IPC.', 'current', '{"phase": "runtime", "enforcement": "strict", "trigger_path": "*"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-008', 'Guideline', 'PRO', 'Résilience Mécanique (Design for Failure)', 'Les systèmes distribués doivent intégrer des patterns de résilience (Circuit Breakers, Back-pressure, Dégradation Gracieuse). Les seuils et mécanismes de défaillance doivent être spécifiés explicitement par des Décisions (DEC) ou Exigences (REQ) au niveau du projet.', 'current', '{"phase": "architecture", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": true}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-009', 'Guideline', 'PRO', 'Performance comme Propriété Native', 'La performance ne s''optimise pas a posteriori. Les budgets de latence (SLO/p99) et les contraintes de ressources (CPU/RAM) doivent être quantifiés et testés en CI pour chaque composant critique via des Exigences (REQ) locales du projet.', 'current', '{"phase": "architecture", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": true}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-010', 'Guideline', 'PRO', 'Sécurité Shift-Left & Moindre Privilège', 'La sécurité (scan de vulnérabilités, gestion des secrets) est automatisée dès la CI. L''accès aux ressources s''opère par RBAC granulaire. Les politiques exactes de rotation des secrets et d''authentification doivent être définies par les Décisions (DEC) du projet.', 'current', '{"phase": "security", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": true}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-011', 'Guideline', 'PRO', 'Évolutivité Humaine & Accessibilité Cognitive', 'L''architecture modulaire doit limiter la charge cognitive (DDD, Clean Architecture). Le nommage est un acte de design reflétant le métier. Le versioning des API doit être explicite. Les choix d''implémentation de ces frontières sont délégués aux projets.', 'current', '{"phase": "design", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": true}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-012', 'Guideline', 'PRO', 'Infrastructure as Code (IaC) & Reproductibilité d''Environnement', 'Les environnements doivent être éphémères et recréables à la demande. L''état de l''infrastructure est versionné (GitOps). L''outil d''automatisation (Nix, Terraform, Docker) est défini par les Décisions (DEC) spécifiques du projet.', 'current', '{"phase": "infrastructure", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": true}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-013', 'Guideline', 'PRO', 'DRY (Don''t Repeat Yourself) & Single Source of Truth', 'Éviter de décrire deux fois la même chose. Chaque connaissance, logique ou règle métier doit posséder une représentation unique et non ambiguë dans le système pour éviter la désynchronisation.', 'current', '{"phase": "coding", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": false}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-014', 'Guideline', 'PRO', 'SRP (Single Responsibility Principle) & Cohésion', 'Une fonction, une classe ou un fichier ne doit avoir qu''une seule raison de changer. Les ''God Objects'' (fichiers monolithiques) sont proscrits. Les responsabilités doivent être isolées.', 'current', '{"phase": "coding", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": false}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-015', 'Guideline', 'PRO', 'KISS (Keep It Simple, Stupid) & YAGNI', 'Ne pas sur-ingénieriser. Ne pas écrire de code ''au cas où'' (You Aren''t Gonna Need It) pour un besoin futur hypothétique. Privilégier la solution la plus simple et lisible permettant de résoudre le problème actuel.', 'current', '{"phase": "coding", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": false}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-016', 'Guideline', 'PRO', 'Limites Cognitives & Complexité Cyclomatique', 'Limitation stricte de l''imbrication et de la longueur des fonctions/fichiers. Une fonction doit idéalement être lisible sur un seul écran sans défilement mental complexe. Les seuils précis doivent être validés par les linters du projet.', 'current', '{"phase": "coding", "enforcement": "advisory", "trigger_path": "*", "requires_local_decision": true}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-017', 'Guideline', 'PRO', 'Clean-As-You-Go (Zéro Code Mort)', 'Le code obsolète, commenté ou remplacé doit être immédiatement supprimé une fois la nouvelle implémentation testée. La base de code ne doit contenir aucun code mort (fonctions sans appelants actifs).', 'current', '{"phase": "refactoring", "enforcement": "strict", "trigger_path": "*", "requires_local_decision": false}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-018', 'Guideline', 'PRO', 'Modules Profonds (APoSD ch.4)', 'Interface étroite + implémentation riche. Coût d''un module = surface d''interface (nb fn pub, params, exceptions), pas LOC. Préférer 1 module 500 LOC avec 3 fn pub à 5 modules 100 LOC avec 15 fn pub. Détection module shallow: ratio interface/impl > 0.3 ou 1 fn pub par 30 LOC. APoSD ch.4.', 'current', '{"updated_at": 1778241813470}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-019', 'Guideline', 'PRO', 'Information Hiding (APoSD ch.5)', 'Chaque module cache un secret structurel (algorithme, format wire, dépendance externe, choix de stockage). L''interface révèle le contrat, pas l''implémentation. Réduit cognitive load et couplage. Détection fuite: renommer un type interne casse plusieurs fichiers consommateurs; un changement de lib privative force update du contrat. APoSD ch.5.', 'current', '{"updated_at": 1778241813794}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-020', 'Guideline', 'PRO', 'Pull Complexity Downwards (APoSD ch.8)', 'Quand un choix est inévitable, absorber la complexité dans l''implémenteur (librairie, serveur, helper) plutôt que la propager au caller. L''API expose la valeur métier, pas le plumbing. Application: defaults sensibles, auto-resolution, recovery embarqué dans la réponse plutôt que dans la doc. APoSD ch.8.', 'current', '{"updated_at": 1778241814084}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-021', 'Guideline', 'PRO', 'Design It Twice (APoSD ch.11)', 'Pour toute décision architecturale (DEC) ou interface publique, explorer ≥2 alternatives radicalement différentes avant de figer. 10-30 min de variantes économisent des heures de refactor. Trace les alternatives écartées dans la DEC pour la postérité. APoSD ch.11.', 'current', '{"updated_at": 1778241815358}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-022', 'Guideline', 'PRO', 'Design-tree interview discipline', 'Pre-commit on any design: walk decision-tree branch-by-branch, one question per turn, recommendation+alternatives per question, dependencies resolved depth-first. Stop only at shared-understanding. Detection skip: LLM produces plan/spec doc before 5+ Q/A turns. Skill: /grill-me. Anchor: Brooks Design of Design ch.3.', 'current', '{"skill": "/grill-me", "anchor": "Brooks Design of Design ch.3", "pillar": "PIL-PRO-003", "updated_at": 1778514497953}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-023', 'Guideline', 'PRO', 'Vertical-slice tracer-bullet decomposition', 'Break feature into REQ-children REFINES that cut UI→API→storage→tests vertically, never horizontally per layer. Each slice independently demoable + flushes unknowns first. First slice MUST integrate all integration boundaries. Detection bad: REQ-A ''build storage layer'' then REQ-B ''build API on storage''. Skill: /to-issues-soll. Anchor: Hunt & Thomas Pragmatic Programmer ch.7.', 'current', '{"skill": "/to-issues-soll", "anchor": "Hunt&Thomas Pragmatic Programmer ch.7", "pillar": "PIL-PRO-003", "updated_at": 1778514499200}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-024', 'Guideline', 'PRO', 'PRD synthesis pattern', 'Feature destination doc = problem_statement + solution_architecture + user_stories[] + acceptance_criteria[] + implementation_decisions (non-prescriptive + durable). Persist as REQ-{code}-N umbrella status=''current'' priority + acceptance_criteria in body, sub-REQs REFINES umbrella. Detection skip: solution articulated before problem. Skill: /to-prd-soll.', 'current', '{"skill": "/to-prd-soll", "pillar": "PIL-PRO-003", "updated_at": 1778514499974}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-025', 'Guideline', 'PRO', 'Throwaway prototype de-risking', 'Validate design or data model unknowns via disposable prototype (standalone script, terminal demo, N route variants) before commit. Max 1-3 days. NEVER merged to main. Required when: new integration, new tech stack, ≥2 viable designs with empirical falsifier. Skill: /prototype. Anchor: Brooks Mythical Man-Month ''plan to throw one away''.', 'current', '{"skill": "/prototype", "anchor": "Brooks Mythical Man-Month", "pillar": "PIL-PRO-003", "updated_at": 1778514501204}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-026', 'Guideline', 'PRO', 'Bootstrap-vs-Continuation phase detection', 'At `axon_init_project`: VIS-{P}-001 absent → `kickoff_bundle.bootstrap_required=true` + `input_documents[]` scan (README/vision/brief/PRD/CONTEXT/*.md depth=1). LLM enters cascade grill-me Vision→Pillars→Concepts→Decisions. VIS present → Continuation flow (REQ umbrella → REFINES children → tdd). No mixed mode. Skill: /bootstrap-soll vs /to-prd-soll.', 'current', '{"skill": "/bootstrap-soll", "pillar": "PIL-PRO-003", "updated_at": 1778514501985}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-027', 'Guideline', 'PRO', 'Token-economy sub-agent policy', 'Main-thread MCP-first for IST/SOLL queries (cost: ~5-50 tokens via query/inspect). Sub-agents allowed for: external research, doc-scan, closed-brief parallelism, MCP-independent tasks. MCP-needing sub-agents → `./scripts/axon mcp-call` CLI bridge. FORBIDDEN: sub-agent forced IST reconstruction via re-read source (cost: 100-200K tokens wasted). Skill: /improve-codebase-architecture-soll.', 'current', '{"skill": "/improve-codebase-architecture-soll", "pillar": "PIL-PRO-004", "updated_at": 1778514503198}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-028', 'Guideline', 'PRO', 'Axon Hand Off — systematic post-session procedure', 'Canonical procedure for closing an Axon work session. Single source of truth — all boot-loaded docs (MEMORY.md, CLAUDE.md global/project, axon-engineering-protocol SKILL.md, kickoff_bundle SOLL nodes) reference this ID, NEVER duplicate its body.

## Trigger
Operator says ''Axon Hand Off'' / ''handoff'' / ''fait un handoff'' OR session is about to be cleared OR context approaches 70% remaining. The 5 steps below are MANDATORY and ORDERED.

## Step 1 — SOLL session_pointer update
Canonical `CPT-{code}-N` (kind=''session_pointer'', status=''current''). Body MUST contain :
- Runtime state : brain pid + binary md5 + `install_generation` + indexer state + PG/dashboard state
- Branch + HEAD SHA + any pending manifest in `.axon/live-release/`
- REQs in-flight with exact SOLL status
- 3 numbered concrete next-session actions
- Blockers + operator-gated stops
Update via `soll_manager(action=update, entity=concept, data={id, description, …})` then `axon_init_project session_pointer={kind, value, label}` to refresh kickoff bundle.

## Step 2 — SOLL cleanup + topological replan
Mandatory before close :
- `soll_validate project_code=<P>` → 0 violations. Close residue via `soll_remove_evidence` (broken file refs) or `soll_manager(action=update, status=archived)` (superseded REQs).
- `soll_verify_requirements project_code=<P>` → promote any `done` REQ still flagged `in_progress` to `completed`.
- `soll_attach_evidence` for every REQ shipped this session (commit SHA + test file + bench CSV if applicable).
- `soll_work_plan project_code=<P> top=8` → verify wave-1 reflects reality. Stale `updated_at` → bump via `entrench_nuance`.
- Log new issues per CPT-AXO-025 triage (branch 1/2/3 — never log without picking a branch).

## Step 3 — Boot-loaded docs prune + compact
Post-`/clear` auto-loaded docs MUST be 100% fresh, ZERO obsolete, compacted-without-precision-loss, LLM-context-optimized. NO content older than the canonical session_pointer. NO redundancy : each fact lives in exactly one source ; cross-references use canonical IDs, never copy.

| Doc | Pattern | Forbidden content |
|---|---|---|
| `~/.claude/CLAUDE.md` (global) | Trigger phrases + minimal source pointers | stale REQs, commit SHAs, version numbers, prose narratives |
| `~/projects/CLAUDE.md` (org) | Methodology pillars cross-project | project-specific REQs / version / SHAs |
| `<repo>/CLAUDE.md` (project) | Architecture pointers + tool routing + canonical command examples | session content, REQs in-flight, bench numbers |
| `<memory>/MEMORY.md` (auto-memory) | Feedback index + single `## Active handoff` line + Hard rules + Architecture facts table | ''Prior handoff'' / ''Previous handoff'' sections, accumulated session narratives |
| `<memory>/feedback_*.md` | Single rule per file, body has `Why:` + `How to apply:` | duplicate of MEMORY.md index |
| Kickoff bundle SOLL nodes (CPT-AXO-021 cold-start order, CPT-AXO-052 session_pointer) | Live SOLL via `soll_manager` ; `cypher SELECT description FROM soll.Node WHERE id=''<ID>''` reads canonical | hard-coded version/SHA/bench |

Stale-detection rule : any cited backend, binary version, build SHA, REQ status, bench number must be verifiable LIVE in same session via `git log` / `cat .axon/live-release/current.json` / `md5sum bin/axon-brain` / `soll_query_context`. If not verifiable → it is stale → remove (not ''maybe update later'').

Compactness rule : tables over prose ; remove ''what happened'' narratives (those live in `docs/working-notes/*handoff*`) ; keep ''how to act now'' only.

## Step 4 — axon-engineering-protocol skill consolidation
SKILL.md MUST be LLM-contract only :
- No prose explanation of historical state (''after migration X we moved to …'' belongs in SOLL DEC body, not SKILL.md)
- Tables for tool routing, error recovery, SOLL types, relations
- 1-line cross-references to SOLL canonical IDs ; never copy SOLL body into SKILL.md
- Section limit : any block >5 lines explaining ''why'' → move to SOLL CPT/DEC, leave a single pointer line
- Retired backends / superseded decisions / removed tools = pruned ; they live in superseded SOLL revisions, not in active skill
Same pattern for sibling skills : `/axon-driven-development`, `/bootstrap-soll`, `/to-prd-soll`, `/to-issues-soll`, `/handoff` (generic — leave alone, Axon-specific behavior is THIS guideline GUI-PRO-028).

## Step 5 — Working-notes audit
`docs/working-notes/<YYYY-MM-DD>-session-NN-<topic>.md` = audit-only, append-only narrative. They do NOT replace SOLL or canonical session_pointer. They MAY be referenced from session_pointer body for full prose context. Old working-notes (>1 month) leave on disk ; SOLL revisions are canonical.

## Detection (CPT-AXO-025 branch 1 trigger)
LLM at next session cites a fact that contradicts live SOLL / git / filesystem → previous session failed step 1, 2, or 3. Log incident as REQ + tag `methodology-failure-cause` + reference this guideline. Root cause = audit which step was skipped, then close loophole via `soll_manager update` on GUI-PRO-028 itself.

## No-redundancy enforcement
Before adding a line to ANY boot-loaded doc : does this fact already exist canonically elsewhere (table above)? If yes → cross-reference, don''t copy. If no → add to the canonical owner, then cross-reference from boot docs. Hot-spots that historically violate this : MEMORY.md (handoff sections accumulate), axon-engineering-protocol SKILL.md (DuckDB residue), repo CLAUDE.md (version pinning).

## Originator
2026-05-13 session-23 incident : Claude trusted stale MEMORY.md handoff snapshot (session 13, 4 sessions behind reality) and delivered a kickoff briefing wrong on backend / bench / next-action. Operator caught and demanded systematic correction. REQ-AXO-90007 logged for the residual Axon-side bug (cypher tool false ''DuckDB plugin error'' under PG-only).
', 'current', '{"skill": "/handoff", "pillar": "PIL-PRO-003", "priority": "P0", "updated_at": 1778691459083}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-029', 'Guideline', 'PRO', 'Cache-TTL economics & end-to-end execution', 'Anthropic prompt cache TTL = 5 minutes. Any pause > TTL refactures full context (~$0.05–2 per pause). Auto/continuous mode: execute plan start→finish single burst. NO intermediate ''should I continue?'' / ''here''s progress so far'' / mid-plan reports. Single terse final summary 1-3 sentences. Stop ONLY on: (i) genuine blocker no reasonable default, (ii) destructive-irreversible action requiring confirmation, (iii) hard external blocker. Detection bad: LLM asks operator confirmation on routine reversible engineering choices.', 'current', '{"anchor": "Anthropic prompt cache 5min TTL", "pillar": "PIL-PRO-004", "updated_at": 1778514504823}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-030', 'Guideline', 'PRO', 'Diagnose loop discipline', 'Bug or perf regression: (1) reproduce minimally, (2) hypothesize falsifiable cause, (3) instrument with 1-line env flag if possible, (4) fix, (5) regression-test. Failed hypothesis still produces VAL-{code}-N (VERIFIES or REJECTS REQ with evidence). Skip cargo-cult fixes (''try X first''). Detection bad: ''should fix'' without repro. Skill: /diagnose. REFINES CPT-PRO-004 PDCA.', 'current', '{"skill": "/diagnose", "pillar": "PIL-PRO-003", "updated_at": 1778514505734, "refines_concept": "CPT-PRO-004"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-099', 'Guideline', 'PRO', 'Test guideline', 'rule', 'current', '{}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-100', 'Guideline', 'PRO', 'Token-efficient writing for LLM-consumed artefacts', 'Every LLM-consumed artefact (SOLL node, SKILL, CLAUDE.md, MEMORY.md, docs/) must maximize signal/token. Rules:
1. Prose forbidden when a schema, regex, example or table suffices.
2. No "recent / latest / set 20XX-XX-XX / observed during ..." in durable artefacts (→ Revision or git log).
3. No duplication of info derivable from native mechanism (Edges, IST query, Revisions, git log) in prose.
4. Before write: `(intent_preserved ∧ tokens_minimized) ∨ rewrite`.
5. Post-delivery nodes compress to thin pointer; rich intent lives in the final Revision.
6. `curate-soll` pass_D detects and compresses nodes > 2K chars or matching op-log patterns.

Applies to ALL NEW writes across projects. Pre-existing rich descriptions in project-scoped SOLL (e.g. AXO) preserved as audit history; cleanup is structural only (status, edges, lifecycle) — not textual.

Refines CPT-PRO-006. Epitomizes GUI-PRO-013 (DRY).', 'current', '{"updated_at": 1778761058799}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-101', 'Guideline', 'PRO', 'Sentinel self-heal smoke', 'Body content sufficient to pass soll_validate criteria.', 'current', '{"updated_at": 1779126887843}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('PIL-PRO-001', 'Pillar', 'PRO', 'Code Quality', 'Architectural discipline ensuring code is testable, deep, well-bounded, free of warnings. Spans: test-first development (GUI-001), DRY/SRP/KISS/cognitive-limits/clean-as-you-go (GUI-013/014/015/016/017), APoSD foundations — deep modules, information hiding, pull-complexity-downwards, design-it-twice (GUI-018/019/020/021). Consumer project GUI-{code}-N covering same scope INHERITS_FROM corresponding GUI-PRO.', 'current', '{"updated_at": 1778514324408}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('PIL-PRO-002', 'Pillar', 'PRO', 'Reliability & Operations', 'Runtime guarantees + observable behavior under production load. Spans: fail-fast + zero-warning (GUI-003), zero-mock I/O verification (GUI-004), control-vs-data-plane separation (GUI-005), deterministic hermetic builds (GUI-006), native structured telemetry (GUI-007), failure-resilience-by-design (GUI-008), performance-as-native-property (GUI-009), shift-left-security + least-privilege (GUI-010), infrastructure-as-code reproducibility (GUI-012).', 'current', '{"updated_at": 1778514324896}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('PIL-PRO-003', 'Pillar', 'PRO', 'Workflow Discipline', 'Process methodology for LLM-assisted development. Spans: MCP-driven documentation (GUI-002), accessibility + cognitive ergonomics (GUI-011), Pocock-derived patterns — design-tree interview (GUI-022), vertical-slice decomposition (GUI-023), PRD synthesis (GUI-024), throwaway prototype (GUI-025), Bootstrap/Continuation phase detection (GUI-026), handoff discipline (GUI-028), diagnose loop (GUI-030).', 'current', '{"updated_at": 1778514325388}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('PIL-PRO-004', 'Pillar', 'PRO', 'Resource Economy', 'Token/cache/context budget management for cost-effective LLM operation. Spans: sub-agent token economy with MCP-first main thread (GUI-027), cache-TTL aware end-to-end execution avoiding mid-task interrupts (GUI-029). Foundational for commercial viability of Axon-methodology workflow.', 'current', '{"updated_at": 1778514325869}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('PIL-PRO-099', 'Pillar', 'PRO', 'Test methodology pillar', 'theming axis', 'current', '{}'::jsonb)
ON CONFLICT (id) DO NOTHING;

-- PRO Edges
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('CPT-PRO-004', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('CPT-PRO-005', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('CPT-PRO-006', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('CPT-PRO-007', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('DEC-PRO-001', 'REQ-AXO-273', 'SOLVES', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-001', 'GUI-FSF-001', 'SUPERSEDES', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-001', 'GUI-MLD-001', 'SUPERSEDES', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-001', 'GUI-NEX-001', 'SUPERSEDES', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-001', 'GUI-TE2-001', 'SUPERSEDES', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-001', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-002', 'GUI-FSF-002', 'SUPERSEDES', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-002', 'GUI-MLD-002', 'SUPERSEDES', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-002', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-003', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-004', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-005', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-006', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-007', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-008', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-009', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-010', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-011', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-012', 'PIL-PRO-002', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-013', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-014', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-015', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-016', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-017', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-018', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-019', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-020', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-021', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-022', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-023', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-024', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-025', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-026', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-027', 'PIL-PRO-004', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-028', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-029', 'PIL-PRO-004', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-030', 'PIL-PRO-003', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-099', 'PIL-PRO-099', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-101', 'GUI-PRO-001', 'INHERITS_FROM', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
