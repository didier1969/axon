# SKILL.md axon-engineering-protocol — Extraction Map (S2)

**Source:** `docs/skills/axon-engineering-protocol/SKILL.md` (228 lignes, 26 688 chars)
**Date:** 2026-05-14
**Status:** Working-note temporaire, non-canonique. Consommé par S3 (migration) + S4 (rewrite).
**Reference:** REQ-AXO-335 / REQ-AXO-336 / REQ-AXO-337

## Légende classification

| Code | Action |
|---|---|
| **KEEP** | Reste inline (contrat factuel court, load-bearing) |
| **KEEP_COMPRESSED** | Reste inline mais 1-ligne ; détails déplacés vers cible SOLL pointer |
| **MIGRATE_EXISTING <ID>** | Body migre dans node SOLL existant (enrichir via `soll_apply_plan dry_run`) |
| **MIGRATE_NEW <type>** | Body migre dans nouveau node SOLL à créer |
| **DROP** | Suppression (redondant avec git log, SOLL existant, ou code source) |

## Classification ligne par ligne

| Source range | Section | Classification | Cible / Note |
|---|---|---|---|
| L1-4 | YAML frontmatter | KEEP | name + description (frontmatter obligatoire) |
| L6 | Header | KEEP | titre |
| L8 | LLM-only doc note + cypher→sql rename | KEEP_COMPRESSED | 1 phrase : "LLM-only doc per CPT-AXO-024 (LLM-only methodology). SOLL canonical via `sql` MCP tool." |
| L10 | Scope (Axon-repo internal) | KEEP | 1 phrase, route vers `/axon-driven-development` pour consumer projects |
| L12 | Methodology cross-references (CPT-PRO-* inheritance) | KEEP_COMPRESSED | 1 ligne : "Cross-project methodology canonical in CPT-PRO-* / GUI-PRO-* via `metadata.generalized_by`. Fetch via `soll_query_context`." |
| L14-25 | Boot — trigger phrases + kickoff_bundle field map | KEEP | table 7 rows, load-bearing pour init |
| L27-36 | Truth hierarchy | KEEP | table 7 rows, contract |
| L38-43 | Runtime model (live/dev/brain/indexer/dashboard) | KEEP | 5 lignes contract |
| L45-62 | Tool routing | KEEP | table 17 rows ; annoter chaque ID inline `(label)` |
| L63 | Backend note (PG+pgvector, DuckDB purged, AGE retiring) | DROP | redondant avec CLAUDE.md projet + MIL-AXO-017 |
| L65-69 | Search recovery server-guidance order | KEEP | 4 lignes contract |
| L71-89 | Recovery matrix 13 rows | KEEP_COMPRESSED | **R1** : 1-ligne par row, drop enumerations verbales ; format : `<status> → inspect data.parameter_repair.{...} (REQ-AXO-NNN (label))` |
| L91 | NEVER inspect Axon source | KEEP | 1 ligne contract |
| L93-99 | Why contract field priority | KEEP_COMPRESSED | 1 ligne ordered list inline |
| L100-101 | SOLL types | KEEP_COMPRESSED | 1 ligne enum |
| L103-120 | Canonical relations table 13 rows | KEEP_COMPRESSED | 1-2 lignes : "Use `soll_relation_schema` before unfamiliar pairs. Canonical pairs catalog in DEC-PRO-100 (relation vocabulary)." |
| L121 | Cypher canonical SOLL row + filter pattern | DROP | redondant (déjà dans tool routing `sql` row + tool description) |
| L123-124 | Vision rule format string | KEEP | format string load-bearing |
| L126-138 | SOLL writes table 7 rows | KEEP_COMPRESSED | 1-ligne par tool + `help(tool=X)` pointer for shape |
| L139 | Async pattern (job_id, wait, timeout_ms) | KEEP_COMPRESSED | 1 ligne |
| L141 | soll_work_plan format + temporal decay | MIGRATE_EXISTING CPT-AXO-024 | détails dans CPT body, garder 1 pointer line dans SKILL |
| L143 | soll_verify_requirements done/partial/missing | MIGRATE_EXISTING CPT-AXO-024 | idem |
| L145 | CLI bridge mcp-call large JSON | KEEP | 1 ligne (utile au quotidien) |
| L147-148 | Identity / ID format / Registry | KEEP | 1-2 lignes |
| L150-160 | Delivery flow 8 steps + "UN FIX = UN COMMIT" | KEEP | 8-line ordered list, load-bearing |
| L162-165 | Release flow | KEEP | 4 lignes |
| L167-171 | Qualification commands | KEEP_COMPRESSED | 2 lignes (`./scripts/axon qualify ...`) |
| L173-180 | 3-way triage CPT-AXO-025 | KEEP | table 3 rows, load-bearing (canonical issue logging policy) |
| L182-186 | PDCA with SOLL (CPT-AXO-024) | DROP | redondant avec CPT-AXO-024 body (canonical there) |
| L188-199 | Hygiene 12 bullets | MIGRATE_EXISTING CPT-AXO-024 | la plupart absorbés dans CPT body ; quelques-uns vivent dans code (TDD gate via cargo) → DROP inline mention |
| L201-209 | Architecture-state CPTs pointer table | KEEP | **R3** : laisser table pointer (CPT → anchor → when to load), bodies déjà dans SOLL |
| L211-216 | Performance investigation playbook 4 bullets | KEEP_COMPRESSED | 4 lignes courtes ou MIGRATE_NEW CPT |
| L218-222 | Sub-agent delegation rules | KEEP_COMPRESSED | 2 lignes + pointer GUI-PRO-027 |
| L224-225 | Hand Off pointer GUI-PRO-028 | KEEP | 1 ligne pointer (5 steps body lives in GUI-PRO-028) |
| L227-228 | Maintenance section | DROP | superseded by GUI-AXO-NEW (skill-immutability-in-handoff) créée en S6 |

## Synthèse migration vers SOLL

### MIGRATE_EXISTING (3 nodes à enrichir via soll_apply_plan dry_run)

| Target | Source range | Content to absorb |
|---|---|---|
| CPT-AXO-024 (LLM-only doc methodology) | L141, L143, L182-186, L188-199 | PDCA loop + soll_work_plan format + soll_verify_requirements done criteria + 12 hygiene bullets (those not in code) |

### MIGRATE_NEW (0-1 nodes à créer)

| Type | Logical key | Content | Decision |
|---|---|---|---|
| CPT-AXO-NEW | `perf-investigation-playbook` | L211-216 4-bullet playbook (instrument-first / 90s diagnostic probes / falsify cheaply / capture VAL even on failed hypothesis) | **DEFER** : laisser inline compressé. Migration ajoute round-trip sans bénéfice (playbook = runbook, pas méthodologie). |

### KEEP estimate

| Section | Lignes new | Chars estimés |
|---|---|---|
| Frontmatter + scope | 7 | 350 |
| Boot table | 9 | 700 |
| Truth hierarchy table | 9 | 600 |
| Runtime model | 6 | 350 |
| Tool routing table | 19 | 1 500 |
| Search recovery + matrix compressed | 22 | 900 |
| SOLL relations 1-liner | 2 | 150 |
| Vision rule | 3 | 250 |
| SOLL writes compressed | 9 | 700 |
| Identity + Delivery + Release + Qualification | 18 | 950 |
| 3-way triage | 6 | 500 |
| Sub-agent delegation 2 lines | 3 | 200 |
| Hand Off pointer | 2 | 150 |
| Architecture-state CPTs pointer table | 8 | 500 |
| Perf playbook compressed | 6 | 300 |
| **Total estimé** | **129** | **8 100** |

⚠ Cible audit : **≤ 5 500 chars**. Excédent estimé : 2 600 chars → besoin de compression supplémentaire S4 sub-agent. Hotspots compression :
- Tool routing : passer de 19 → 12 rows en groupant (DX tools / SOLL tools / governance / system)
- Recovery matrix : passer de 13 status à 8 (grouper inputs liés)
- Architecture-state CPTs : passer de 4 rows à 2 (CPT-AXO-054 + 1 catch-all)

## Decision summary pour S3

| Décision | Choix |
|---|---|
| Nodes à enrichir | CPT-AXO-024 seul (single target) |
| Nouveaux nodes | Aucun (defer perf playbook) |
| Risk REQ-AXO-323 silent UPSERT | Use `soll_apply_plan dry_run=true` even for 1-node update |
| Validation post-migration | `soll_query_context id=CPT-AXO-024` + `soll_validate project_code=AXO` |

## Open question pour operator (non-blocking)

- L141 `soll_work_plan` temporal decay : doit-il rester inline (utile au quotidien) ou migrer vers CPT-AXO-024 ? **Recommandation : 1-ligne dans SKILL + détails dans CPT-AXO-024.**
