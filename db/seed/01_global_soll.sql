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
--   - 62 soll.Node rows : 5 PIL-PRO + 8 CPT-PRO + 3 DEC-PRO + 46 GUI-PRO
--   - 56 soll.Edge rows : cross-namespace BELONGS_TO / INHERITS_FROM /
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

-- Les GUI-PRO seedes ci-dessous portent des ids EN DUR, que
-- `soll.allocate_node_id` n'a jamais attribues : sans ce recalage le compteur
-- repart sous eux et l'allocateur boucle a vide jusqu'a les depasser (sa
-- garde `EXIT WHEN NOT EXISTS` empeche la collision, pas le gaspillage).
-- GREATEST pour ne JAMAIS reculer un compteur deja plus avance en base.
UPDATE soll.Registry SET last_gui = GREATEST(last_gui, 131) WHERE project_code = 'PRO';

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
VALUES ('GUI-PRO-119', 'Guideline', 'PRO', 'Une supersession retire sa cible', 'Règle SOLL déclarative (DEC-AXO-901652, REQ-AXO-902455). Une arête `SUPERSEDES` affirme que sa cible est retirée. Si le statut de la cible dit le contraire, le graphe se contredit lui-même — supersédé par une arête, ouvert par son statut — et le plan de travail compte un nœud de trop. Signalé par TE2 (llm_feedback #224), découvert en comptant les jalons ouverts de `soll_roadmap`.

RÉPARATION : retirer la cible (soll_manager action=update, status=superseded), ou retirer l''arête si la supersession n''était pas voulue (action=unlink).

Cette règle ne s''applique QUE lorsque la source est vivante. Source retirée = arête inversée, voir GUI-PRO-120.

## Correction du 2026-08-22 (session 124) — le vocabulaire de retrait était incomplet

La règle ne comptait que `superseded` comme « retiré ». Or `rejected` et `archived` le sont aussi : `DEC-AXO-073 SUPERSEDES DEC-AXO-072` était signalé alors que les DEUX extrémités sont retirées — la cible est `rejected`, délibérément. Conseiller de « retourner l''''arête » là-dessus aurait remis un nœud rejeté à `current`, c''''est-à-dire falsifié le registre pour verdir une porte.

Mesuré à la correction : **1 cas sur 11** relevait de ce défaut. Peu, mais c''''était la RÈGLE qui était fausse, pas la donnée (GUI-PRO-106 : corriger l''''origine).', 'current', '{"soll_rule": {"mode": "forbidden", "relations": ["SUPERSEDES"], "source_status_not_in": ["superseded", "rejected"], "target_status_not_in": ["superseded", "rejected", "archived"], "message": "la cible est encore ouverte alors que l''arête la déclare retirée"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-120', 'Guideline', 'PRO', 'Une arête SUPERSEDES ne part pas du nœud retiré', 'Règle SOLL déclarative (DEC-AXO-901652, REQ-AXO-902455). `A SUPERSEDES B` dit que A remplace B : A est vivant, B est retiré. Quand c''est la SOURCE qui porte un statut retiré et la cible qui est vivante, l''arête a été posée À L''ENVERS.

Mesure AXO du 2026-08-22 : 8 des 10 arêtes incohérentes étaient de cette forme (PIL-AXO-006 SUPERSEDES PIL-AXO-004, alors que 006 est le supersédé). C''est pourquoi cette règle est SÉPARÉE de GUI-PRO-119 : conseiller « retire la cible » sur celles-ci retirerait le nœud CANONIQUE survivant.

RÉPARATION : action=unlink, puis re-lier dans l''autre sens — le nœud retiré est la SOURCE.

## Correction du 2026-08-22 (session 124) — le vocabulaire de retrait était incomplet

La règle ne comptait que `superseded` comme « retiré ». Or `rejected` et `archived` le sont aussi : `DEC-AXO-073 SUPERSEDES DEC-AXO-072` était signalé alors que les DEUX extrémités sont retirées — la cible est `rejected`, délibérément. Conseiller de « retourner l''''arête » là-dessus aurait remis un nœud rejeté à `current`, c''''est-à-dire falsifié le registre pour verdir une porte.

Mesuré à la correction : **1 cas sur 11** relevait de ce défaut. Peu, mais c''''était la RÈGLE qui était fausse, pas la donnée (GUI-PRO-106 : corriger l''''origine).', 'current', '{"soll_rule": {"mode": "forbidden", "relations": ["SUPERSEDES"], "source_status_in": ["superseded", "rejected"], "target_status_not_in": ["superseded", "rejected", "archived"], "message": "arête INVERSÉE : la source est retirée et la cible vivante — ne PAS retirer la cible"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;
INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-121', 'Guideline', 'PRO', 'Deux nœuds vivants ne portent pas le même titre', 'Règle SOLL déclarative (REQ-AXO-902455, axe « unicité »). Un titre est ce par quoi un humain et un LLM désignent un nœud. Quand deux nœuds vivants le partagent, toute référence par le titre devient ambiguë et le lecteur ne peut pas savoir lequel fait foi.

PREMIÈRE règle qui compare des nœuds ENTRE EUX : c''est celle qui matérialise DEC-AXO-901673, la décision qui déplace la frontière posée par DEC-AXO-901649.

## État mesuré à la pose (2026-08-22)

Parc entier : 3 groupes, 109 nœuds. PRO 41 · TST 68 · tous les autres projets 0, AXO compris.

Les 41 de PRO sont le vrai signal, et il n''était visible d''aucune autre manière : ce sont des RÉSIDUS DE TESTS dans le namespace produit hérité par les 75 tenants — 21 « test skill — tdd obligatoire procedure » (SKI-PRO-002 … SKI-PRO-1040) et 20 « test prd body template » (PRT-PRO-001 … PRT-PRO-1018). La suite de tests écrit dans PRO. TST est le projet de test lui-même, sans enjeu.

## Divergence assumée avec le check qu''elle remplace

Le check `duplicate_titles` groupait par (type, titre) ; cette règle groupe par titre seul, donc elle attrape EN PLUS deux nœuds de types différents au même titre — une ambiguïté réelle pour un lecteur. Sur la donnée du 2026-08-22 la différence est nulle : aucun des 3 groupes ne mélange les types. La divergence est donc théorique aujourd''hui et voulue ; un test la vérifie dans les deux sens.

## RÉPARATION

Renommer le doublon, ou le retirer (statut `superseded` avec l''arête `SUPERSEDES` vers le canonique) s''il fait double emploi. Les résidus de tests relèvent de la seconde voie.', 'current', '{"soll_rule": {"subject_status_in": ["current", "planned"], "unique_by": "title", "message": "titre partagé par un autre nœud vivant — la référence par le titre est ambiguë"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-122', 'Guideline', 'PRO', 'Une exigence ouverte remonte à une Vision', 'Règle SOLL déclarative (REQ-AXO-902455, axe « atteignabilité »). Une exigence qu''aucun chemin de filiation ne relie à une Vision ne dit pas POURQUOI on la ferait. C''est exactement ce que VIS-AXO-001 nomme : le code dit ce qui est, la SOLL doit dire pourquoi — une exigence détachée n''apporte ni l''un ni l''autre.

Le chemin est transitif et suit les seules relations de filiation : SOLVES, BELONGS_TO, REFINES, TARGETS, EXPLAINS, EPITOMIZES. Le cas nominal est REQ -BELONGS_TO-> PIL -EPITOMIZES-> VIS.

## Portée : les statuts OUVERTS seulement, et c''est mesuré

État AXO au 2026-08-22 : 127 exigences n''atteignent aucune Vision, mais 100 d''entre elles sont `delivered` et 15 `rejected`. Les signaler serait un audit d''histoire, pas un plan de travail : le travail est fait, et rattacher après coup fabriquerait de la filiation devinée. La règle vise donc `current` et `planned` — 7 cas sur AXO.

Le rattachement rétroactif des 100 `delivered` est une question SÉPARÉE, à trancher avec l''opérateur, pas à imposer par une règle.

## RÉPARATION

soll_manager(action=link) vers le Pillar qui porte le sujet — la paire REQ → PIL n''admet que BELONGS_TO. Si aucun Pillar ne convient, c''est l''exigence qu''il faut requalifier : elle vise autre chose que ce que le produit poursuit.', 'current', '{"soll_rule": {"subject_kind": "Requirement", "subject_status_in": ["current", "planned"], "reaches": true, "other_kind": "Vision", "relations": ["SOLVES", "BELONGS_TO", "REFINES", "TARGETS", "EXPLAINS", "EPITOMIZES"], "message": "aucun chemin de filiation ne relie cette exigence à une Vision"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-123', 'Guideline', 'PRO', 'Un projet n''a qu''une seule Vision vivante', 'Règle SOLL déclarative (REQ-AXO-902455, axe « agrégat »). Une Vision est le nœud unique auquel toute la filiation remonte. Deux Visions vivantes dans un projet, c''est deux nords : GUI-PRO-122 devient satisfiable par l''une OU l''autre, et « pourquoi ce produit existe » cesse d''avoir une réponse.

## Cette règle ne signale RIEN aujourd''hui, et c''est dit plutôt que tu

Mesure du 2026-08-22 sur les 75 projets : AUCUN n''a plus d''une Vision vivante. C''est le seul des six axes du moteur qui soit posé sans cas réel — les autres en ont 41, 7, 88, 76 et 10.

Elle est posée quand même parce qu''elle garde un invariant que le code défend déjà par ailleurs (`axon_init_project` seul peut créer une Vision, `vision_creation_forbidden` refuse partout ailleurs) : la règle rend cet invariant VÉRIFIABLE sur la donnée au lieu de le laisser reposer sur le seul chemin d''écriture. Une garde de non-régression, pas un révélateur. Sa falsification est donc portée par son test, qui construit deux Visions vivantes et exige le rouge.

## RÉPARATION

Retirer la Vision surnuméraire (statut `superseded` + arête SUPERSEDES depuis la canonique), jamais la supprimer. Historiquement AXO en a absorbé quatre de cette façon (VIS-AXO-900/901/902/903 vers VIS-AXO-001).', 'current', '{"soll_rule": {"subject_kind": "Vision", "subject_status_not_in": ["superseded", "rejected", "archived"], "at_most": 1, "message": "plusieurs Visions vivantes dans le projet — la filiation n''a plus de nord unique"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-124', 'Guideline', 'PRO', 'Une exigence livrée n''est pas prouvée par un artefact cassé', 'Règle SOLL déclarative (REQ-AXO-902455, axe « preuves »). Une exigence `delivered` dont une preuve porte `artifact_status = broken` n''est pas prouvée : le fichier, le test ou la métrique qui la justifiait n''existe plus.

Demandée par VPC, avec sa mesure : sur 636 preuves d''un projet client, 485 (76 %) n''avaient jamais été vérifiées, et une exigence pouvait être `delivered` et « prouvée » par un fichier supprimé trois mois plus tôt sans que rien ne le dise.

## Portée cross-tenant, et le volume n''est PAS un critère

Reprend GUI-AXO-1032, qui restreignait à AXO au motif qu''une règle bruyante se fait désaffûter. Ce motif est écarté : le nombre de violations qu''une règle vraie révèle ne décide pas si on la pose. Ce qui décide, c''est sa justesse.

État mesuré à la pose (2026-08-22), sous des exigences `delivered` : AXO 88, TE2 25, FSF 6, ROM 4, NEX 2, MLD 1. Le statut `missing` n''existe dans AUCUNE ligne du parc — une règle qui l''aurait visé n''aurait jamais rien signalé, ce qui est pire qu''une règle absente : elle rassure.

## Ce que la règle ne peut pas distinguer, et qu''il faut lire avant de réparer

Sur les 88 d''AXO, quatre natures très différentes : 33 pointent un module renommé (pipeline_v2 → pipeline), 21 sont des fichiers /tmp qui n''auraient jamais dû être acceptés comme preuve, 6 visent un script supprimé volontairement, 23 sont de vrais fichiers disparus. La règle détecte juste ; le remède n''est pas unique.

## RÉPARATION

Rattacher une preuve valide (soll_attach_evidence), ou retirer la preuve périmée (soll_remove_evidence) et re-qualifier. Ne PAS remettre l''exigence à `current` par réflexe : le travail a pu être fait et seule sa trace avoir bougé.', 'current', '{"soll_rule": {"subject_kind": "Requirement", "subject_status_in": ["delivered"], "evidence_status_in": ["broken"], "message": "preuve cassée sous une exigence livrée — l''artefact qui la justifiait n''existe plus"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-125', 'Guideline', 'PRO', 'Un nœud retiré enregistre ce qui le remplace', 'Règle SOLL déclarative (REQ-AXO-902455, axe « direction entrante »). Un nœud `superseded` doit RECEVOIR une arête SUPERSEDES : sans elle, on sait qu''il est retiré mais pas par quoi, et le lecteur d''un corps périmé n''a aucun chemin vers la version vivante.

Demandée par OPV (llm_feedback #96), avec sa mesure : 105 nœuds retirés sans remplacement enregistré.

## Pourquoi elle n''était pas exprimable avant

Le moteur ne savait poser que des contraintes sur les arêtes SORTANTES. Celle-ci porte sur les ENTRANTES — « quelque chose pointe-t-il vers moi ». La même règle en sortante dit littéralement l''inverse (elle exigerait que le nœud retiré en supersède un autre) : `direction` porte du sens, et un test le vérifie.

## Portée cross-tenant

Reprend GUI-AXO-1033, restreinte à AXO par prudence de volume — motif écarté pour la même raison que GUI-PRO-124.

État mesuré à la pose (2026-08-22) : OPV 95 · AXO 76 · MPM 26 · SWT 14 · NEX 13 · TE2 6 · APS 5 · FSF 4, environ 240 sur le parc.

## Ordre de réparation, mesuré et contraint

Sur AXO, 6 des 76 sont le MÊME défaut que GUI-PRO-120 voit sous un autre angle : le nœud a bien une arête SUPERSEDES, mais posée à l''envers (CPT-AXO-038, DEC-AXO-092, GUI-AXO-1001, PIL-AXO-006, REQ-AXO-207, REQ-AXO-208). Retourner l''arête éteint les deux règles d''un coup. Traiter GUI-PRO-120 EN PREMIER, sinon ces 6 sont comptés deux fois.

## RÉPARATION

Poser l''arête manquante depuis le remplaçant : soll_manager(action=link, source_id=<le vivant>, target_id=<le retiré>, relation_type=SUPERSEDES). Attention au SENS — la source est le nœud VIVANT ; l''inverse est l''erreur que GUI-PRO-120 attrape.

Si rien ne le remplace, le statut juste est `rejected`, pas `superseded`. Mais VÉRIFIER d''abord : « aucune information dans le graphe » ne veut pas dire « rien ne le remplace ». Sur AXO, CPT-AXO-040 (Apache AGE) n''a ni metadata ni indice de titre, et pourtant MIL-AXO-017 et DEC-AXO-083 sont bien ce qui l''a remplacé — nommés en clair dans la documentation du dépôt, jamais écrits dans le graphe.', 'current', '{"soll_rule": {"subject_status_in": ["superseded"], "mode": "required", "direction": "incoming", "relations": ["SUPERSEDES"], "message": "nœud retiré sans arête SUPERSEDES entrante — on ignore ce qui le remplace"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;


INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-126', 'Guideline', 'PRO', 'Une exigence ouverte porte ses critères d''acceptation', 'Règle SOLL déclarative (REQ-AXO-902455, axe « métadonnée »). Une exigence ouverte sans critère d''acceptation ne dit pas à quoi on saura qu''elle est faite. C''est le trou que `soll_verify_requirements` ne peut pas combler : sans critère, il ne reste que la déclaration de celui qui la clôt.

## Sixième et dernier axe posé

Le moteur porte six prédicats ; celui-ci était le seul à n''avoir AUCUNE règle réelle. Poser la première est le test d''acceptation d''une capacité livrée (practice #1368) — c''est ainsi qu''on a découvert que `structural_invariants`, livré depuis des mois, n''avait jamais été exercé : 0 règle sur 258 Guidelines.

## État mesuré à la pose (2026-08-22), exigences OUVERTES seulement

CHC 12 sur 12 · LLL 5 sur 17 · MLD 4 sur 4 · APS 3 sur 49 · **AXO 0** — le tenant zéro est déjà conforme, ce qui rend cette règle sûre à hériter : elle ne signale que là où le manque est réel.

## Pourquoi les statuts ouverts seulement

Exiger un critère d''acceptation sur une exigence `delivered` demanderait de le reconstruire après coup, donc de le deviner. Un critère écrit après la livraison ne prouve rien : il est écrit en regardant ce qui a été fait. La règle ne vise que ce qui reste à faire.

## Ce que la règle ne peut PAS exprimer, et qui reste en code

Le check `uncovered_requirements` est une CONJONCTION — ni preuve rattachée NI critère d''acceptation. `parse_soll_rule` refuse deux prédicats dans une même règle, par construction : une violation combinée n''aurait pas de sens univoque et son message ne pourrait pas dire lequel a échoué. Cette règle-ci porte donc la moitié « critère » ; la moitié « preuve » reste dans `soll_completeness_snapshot_filtered`.

## Chevauchement MESURÉ avec `uncovered_requirements`, et ce qu''elle ajoute

Parc hors projets de test, exigences ouvertes : **29** sans critère. **11** sont aussi sans preuve, donc déjà vues par `uncovered_requirements`. **18 sont un ajout réel** — elles portent une preuve et n''ont pas de critère, donc la conjonction ne les a JAMAIS signalées. Le chevauchement est borné et connu ; il n''est pas une raison de ne pas poser la règle, mais il faut savoir qu''une exigence sans preuve ni critère apparaîtra sous les deux angles.

## RÉPARATION

`soll_manager(action=update, data={id, acceptance_criteria: [...]})`. Un critère utile est vérifiable par quelqu''un d''autre que son auteur : « le test X passe », « la métrique Y descend sous Z » — pas « c''est propre ».', 'current', '{"soll_rule": {"subject_kind": "Requirement", "subject_status_in": ["current", "planned"], "metadata_required": ["acceptance_criteria"], "message": "exigence ouverte sans critère d''acceptation — rien ne dira qu''elle est faite"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-127', 'Guideline', 'PRO', 'Une exigence ouverte est rattachée au graphe', 'Règle SOLL déclarative (REQ-AXO-902455, direction `either`). Une exigence que RIEN ne relie au reste du graphe — ni parent, ni décision, ni validation, ni preuve — n''a aucun chemin par lequel un lecteur puisse arriver jusqu''à elle. Elle existe sans être atteignable.

Distincte de `GUI-PRO-122` : celle-ci demande une arête, N''IMPORTE laquelle ; `122` demande un chemin de filiation qui remonte jusqu''à une Vision. Une exigence peut satisfaire l''une et violer l''autre.

## Pourquoi elle n''était pas exprimable avant

Le moteur ne savait tester une arête que dans UN sens. Cet invariant demande « une arête, de l''un OU l''autre côté ». Deux règles `outgoing` + `incoming` ne le remplacent pas : un nœud isolé produirait DEUX violations pour un seul défaut, et un nœud rattaché d''un seul côté en produirait une à tort. D''où la troisième direction, `either`, et un test qui montre qu''elle dit ce qu''aucune des deux autres ne peut dire.

## État mesuré à la pose (2026-08-22)

**1 seul cas** sur tout le parc hors projets de test (NTO). C''est peu, et c''est dit : cette règle ne révèle presque rien aujourd''hui. Sa valeur est ailleurs — l''invariant cesse d''être une branche Rust qu''un tenant ne peut ni ajuster ni désactiver sans un promote du cœur.

## RÉPARATION

`soll_manager(action=link)` vers le Pillar qui porte le sujet. Si rien ne convient, c''est que l''exigence n''a pas de place dans ce projet.', 'current', '{"soll_rule": {"subject_kind": "Requirement", "subject_status_in": ["current", "planned"], "mode": "required", "direction": "either", "message": "exigence reliée à rien — aucun chemin ne mène jusqu''à elle"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-128', 'Guideline', 'PRO', 'Une validation dit ce qu''elle vérifie', 'Règle SOLL déclarative (REQ-AXO-902455, direction `either`). Une `Validation` est une preuve d''intention : sans arête `VERIFIES`, on sait qu''une vérification a eu lieu mais pas de QUOI elle est la preuve. Elle ne prouve donc rien de rattachable.

L''arête est acceptée dans les deux sens : la convention d''écriture a varié selon les projets et les sessions, et exiger une orientation ferait dépendre la conformité de l''époque à laquelle le nœud a été écrit — pas de sa justesse.

## Pourquoi elle n''était pas exprimable avant

Le moteur ne savait tester une arête que dans UN sens. Cet invariant demande « une arête, de l''un OU l''autre côté ». Deux règles `outgoing` + `incoming` ne le remplacent pas : un nœud isolé produirait DEUX violations pour un seul défaut, et un nœud rattaché d''un seul côté en produirait une à tort. D''où la troisième direction, `either`, et un test qui montre qu''elle dit ce qu''aucune des deux autres ne peut dire.

## État mesuré à la pose (2026-08-22)

**0 cas** sur le parc hors projets de test. Garde de non-régression, comme `GUI-PRO-123`, et c''est écrit plutôt que tu.

## RÉPARATION

`soll_manager(action=link, relation_type=VERIFIES)` depuis la validation vers ce qu''elle éprouve.', 'current', '{"soll_rule": {"subject_kind": "Validation", "mode": "required", "direction": "either", "relations": ["VERIFIES"], "message": "validation sans arête VERIFIES — on ignore de quoi elle est la preuve"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-129', 'Guideline', 'PRO', 'Une décision ouverte dit sur quoi elle porte', 'Règle SOLL déclarative (REQ-AXO-902455, direction `either`). Une `Decision` que rien ne relie au graphe énonce un choix sans dire de quoi il décide. Le rationnel est là, son objet ne l''est pas.

## La règle N''ÉNUMÈRE PAS les relations, et c''est délibéré

Le check en dur qu''elle remplace codait `SOLVES | IMPACTS`, un SOUS-ENSEMBLE de ce que l''écrivain déclare légal — d''où `REQ-AXO-902405` : une décision rattachée par le chemin nominal avec `REFINES` naissait en violation, et le message lui reprochait de n''avoir « aucun lien » alors qu''elle en avait un, légal, posé par l''outil lui-même. Le correctif d''alors dérivait la liste depuis la politique au runtime.

Figer cette liste dans une règle-donnée recréerait exactement le défaut : ajouter une relation à la matrice exigerait de penser à la répercuter ici, ce que personne n''a fait la première fois. La règle demande donc UNE arête, quelle qu''elle soit. Vérifié en base au 2026-08-22 : **0 décision** n''est reliée uniquement par une relation hors politique — les deux formulations rendent le même verdict, et celle-ci ne peut pas diverger. La légalité de la relation est jugée séparément, par la matrice de paires.

## Pourquoi elle n''était pas exprimable avant

Le moteur ne savait tester une arête que dans UN sens. Cet invariant demande « une arête, de l''un OU l''autre côté ». Deux règles `outgoing` + `incoming` ne le remplacent pas : un nœud isolé produirait DEUX violations pour un seul défaut, et un nœud rattaché d''un seul côté en produirait une à tort. D''où la troisième direction, `either`, et un test qui montre qu''elle dit ce qu''aucune des deux autres ne peut dire.

## État mesuré à la pose (2026-08-22)

**0 cas** sur le parc hors projets de test.

## RÉPARATION

`soll_manager(action=link)` vers l''exigence ou le concept sur lequel la décision porte. `soll_relation_schema` donne les relations légales pour la paire.', 'current', '{"soll_rule": {"subject_kind": "Decision", "subject_status_in": ["current", "planned"], "mode": "required", "direction": "either", "message": "décision reliée à rien — son objet n''est pas dans le graphe"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-130', 'Guideline', 'PRO', 'Un nœud retiré le dit dans son CORPS, pas seulement dans son statut', 'Règle SOLL déclarative (REQ-AXO-902458, axe « contenu du corps »). Matérialise `GUI-PRO-110`, énoncée depuis des mois et **jamais mécanisée**.

Le `status` seul est INVISIBLE au scan d''un LLM : les LLM lisent le CORPS. Un nœud `superseded` dont le texte ne dit rien se fait citer, copier et suivre comme s''il était vivant — c''est exactement ce que `GUI-PRO-110` décrit, et pourquoi elle demande de marquer le corps à toute supersession.

## Complémentaire de GUI-PRO-125, pas redondante

`125` demande une ARÊTE (« qu''est-ce qui le remplace, dans le graphe »). Celle-ci demande une PHRASE (« le lecteur du texte est-il averti »). Un nœud peut satisfaire l''''une et violer l''''autre : l''''arête existe mais le corps se lit comme un document courant.

## Marqueurs acceptés

`supersédé` · `superseded by` · `remplacé par` / `remplacée par` · `caduc` · `pointeur canonique` · `obsolète`. Comparaison **sans casse**, sur le corps entier.

La liste est FERMÉE, et c''''est délibéré : accepter une expression régulière fournie par le tenant serait le langage de requête que `DEC-AXO-901673` continue d''''interdire. Un tenant qui veut son propre vocabulaire pose SA règle avec SES fragments — c''''est précisément ce que les règles-données permettent sans toucher au cœur.

## État mesuré à la pose (2026-08-22)

**255 nœuds retirés sans marqueur, sur 16 projets** : APS AXO CSC FSF HXH INK KKI LLL MLD MPM NEX OPV PRO ROM SWT VPC. C''''est la règle au plus fort volume du catalogue — le motif est universel, aucun projet n''''y échappe.

## RÉPARATION

`soll_manager(action=append_section)` avec un en-tête qui dit le retrait et pointe le remplaçant. Ne PAS réécrire le corps entier : la trace historique a de la valeur, c''''est l''''avertissement qui manque.', 'current', '{"soll_rule": {"subject_status_in": ["superseded"], "body_contains_any": ["supersédé", "superseded by", "remplacé par", "remplacée par", "caduc", "pointeur canonique", "obsolète"], "message": "nœud retiré dont le CORPS ne l''annonce pas — un LLM le lira comme vivant"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
VALUES ('GUI-PRO-131', 'Guideline', 'PRO', 'La filiation ne boucle pas', 'Règle SOLL déclarative (REQ-AXO-902458, axe « acyclicité »). Matérialise `DEC-AXO-098`, qui impose un graphe de filiation strictement acyclique.

Un cycle de filiation rend l''''intention circulaire : A existe pour B, qui existe pour A. Aucune racine, donc aucune réponse à « pourquoi ce travail ». `soll_work_plan` ne peut pas ordonner, `GUI-PRO-122` devient satisfiable par la boucle elle-même.

## Pourquoi le validateur de DEC-AXO-098 n''''a jamais été activé

`soll_manager(action=link)` pré-vérifie les cycles À L''''ÉCRITURE — mais seulement sur les arêtes qu''''il pose lui-même. Les cycles ANTÉRIEURS restent, et `soll_acyclic_audit` le dit dans son propre message : *« DEC-AXO-098 cycle validator activation requires these to be 0 »*. Mesuré sur AXO : **3 cycles**. La porte attendait un zéro que rien ne produisait — un gate qui se conditionne à sa propre cible ne s''''arme jamais.

Cette règle inverse le sens : elle SIGNALE les cycles au lieu d''''attendre qu''''il n''''y en ait plus.

## Le jeu de relations est explicite, et il compte

`SOLVES` · `BELONGS_TO` · `REFINES` · `TARGETS` · `EXPLAINS` · `VERIFIES` — la liste de filiation de `DEC-AXO-098`. Un cycle par `SUPERSEDES` n''''est PAS un cycle de filiation (c''''est une chaîne de versions, et deux nœuds qui se supersèdent mutuellement relèvent de `GUI-PRO-119`/`120`). Les confondre signalerait des nœuds que personne ne peut réparer.

## RÉPARATION

`soll_manager(action=unlink)` sur l''''arête qui ferme la boucle. Choisir laquelle demande de VOIR le cycle entier — c''''est pourquoi chaque violation nomme tous ses membres, pas seulement un.', 'current', '{"soll_rule": {"acyclic": true, "relations": ["SOLVES", "BELONGS_TO", "REFINES", "TARGETS", "EXPLAINS", "VERIFIES"], "message": "cycle de filiation — l''intention tourne en rond, aucune racine"}, "enforcement": "advisory", "phase": "soll-audit"}'::jsonb)
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
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-119', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-120', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-121', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-122', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-123', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-124', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-125', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-126', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-127', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-128', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-129', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-130', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;

INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata)
VALUES ('GUI-PRO-131', 'PIL-PRO-001', 'BELONGS_TO', 'PRO', '{}'::jsonb)
ON CONFLICT (source_id, target_id, relation_type) DO NOTHING;
