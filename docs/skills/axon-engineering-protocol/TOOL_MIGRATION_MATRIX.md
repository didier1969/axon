# Tool Migration Matrix — MIL-AXO-019

Inventaire canonique des 59 tools MCP exposés par `axon-brain`, avec tier, catégorie (CPT-AXO-90007), dépendances, et statut de migration vers la surface tri-modale.

Spécifie les sous-REQ tool individuelles à créer dans MIL-AXO-019. Mise à jour en synchronisation avec `soll.node` (chaque tool migré gagne sa REQ ; chaque tool justifié gagne sa DEC).

REQ parente : **REQ-AXO-91491**.

---

## 1. Comptage par tier

| Tier | Définition | Count | Verdict requis |
|---|---|---:|---|
| **A** | analyse pure / cognitive | 19 | migration tri-modale OU DEC justification |
| **B** | SOLL CRUD / governance | 20 | compléter graphe SOLL existant |
| **C** | runtime / admin | 14 | exposer métriques migration |
| **D** | meta / escape hatch | 6 | DEC hors-périmètre |
| **TOTAL** | | **59** | |

---

## 2. Matrice complète (59 lignes)

### 2.1 Tier A — analyse pure (19 tools)

| # | tool | catégorie CPT-AXO-90007 | depends_on (impl + cognitif) | status | soll_link |
|---|---|---|---|---|---|
| 1 | `query` | single-lookup | — | pending | REQ à créer |
| 2 | `inspect` | single-lookup | query | pending | REQ à créer |
| 3 | `path` | structural | query | pending | REQ à créer |
| 4 | `bidi_trace` | structural | query, path | pending | REQ à créer |
| 5 | `impact` | structural + impact-context | query, path ; *cognitif* : bridge_symbols, centrality | pending | REQ à créer |
| 6 | `api_break_check` | structural + impact-context | query, path | pending | REQ à créer |
| 7 | `change_safety` | impact-context | impact, api_break_check | pending | REQ à créer |
| 8 | `simulate_mutation` | impact-context | impact | pending | REQ à créer |
| 9 | `architectural_drift` | anomaly-detection | path, impact | pending | REQ à créer |
| 10 | `anomalies` | anomaly-detection | — (standalone) ; *cognitif* : scc_enumerate, bridge_symbols | pending | REQ à créer |
| 11 | `semantic_clones` | clone-detection | vector + structural | pending | REQ à créer |
| 12 | `snapshot_diff` | structural | snapshot_history | pending | REQ à créer |
| 13 | `diff` | structural | snapshot_history | pending | REQ à créer |
| 14 | `why` | rationale | query, soll_query_context, retrieve_context | pending | REQ à créer |
| 15 | `conception_view` | rationale | soll_query_context, why | pending | REQ à créer |
| 16 | `truth_check` | rationale | retrieve_context, soll_query_context, query | pending | REQ à créer |
| 17 | `retrieve_context` | mixte (rationale/wiring/semantic) | query, inspect, soll_query_context ; déjà partiel | pending → refonte slice 5 | REQ-AXO-91489 |
| 18 | `retrieve_context_layered` | mixte | retrieve_context | pending | REQ à créer |
| 19 | `audit` | anomaly-detection + rationale | anomalies, architectural_drift, change_safety ; *cognitif* : centrality, bridges, scc | pending | REQ à créer |

### 2.2 Tier B — SOLL CRUD / governance (20 tools)

| # | tool | rôle | depends_on | status | soll_link |
|---|---|---|---|---|---|
| 20 | `soll_manager` | core writer SOLL | — | partial (graphe SOLL en RAM via REQ-AXO-322) ; améliorer pre-write validation | REQ-AXO-91492 (cycles), REQ-AXO-91498 (status pre-write) |
| 21 | `soll_query_context` | retrieve depuis SOLL | soll_manager | partial → refonte tri-modale slice 5 | REQ à créer |
| 22 | `soll_work_plan` | DAG SOLL traversal scored | soll_query_context | **bug REQ-AXO-91500** + refonte graphe RAM | corriger via REQ-AXO-91500 |
| 23 | `soll_verify_requirements` | verify REQ done state | soll_query_context, soll_validate | partial | REQ à créer |
| 24 | `soll_validate` | validate SOLL state | soll_query_context | partial | REQ à créer |
| 25 | `document_intent` | auto-classifier wrap soll_manager.create | soll_manager | current (CPT-AXO-019) | DEC justification (wrapper) |
| 26 | `refine_lattice` | SOLL lattice refinement | soll_manager | partial | DEC justification (specialty tool) |
| 27 | `entrench_nuance` | nuance handling | soll_manager | partial | DEC justification (specialty tool) |
| 28 | `infer_soll_mutation` | suggest SOLL changes from code | soll_query_context, retrieve_context | partial | REQ à créer (benefits tri-modal) |
| 29 | `axon_apply_guidelines` | apply GUI bundle | soll_manager | partial | DEC justification (bulk apply) |
| 30 | `axon_apply_methodology_bundle` | apply methodology | soll_manager, axon_apply_guidelines | partial | DEC justification (bulk apply) |
| 31 | `soll_apply_plan` | bulk plan apply | soll_manager | partial | DEC justification (bulk apply) |
| 32 | `soll_commit_revision` | commit SOLL revision | soll_manager | partial | DEC justification (CRUD) |
| 33 | `soll_rollback_revision` | rollback revision | soll_manager | partial | DEC justification (CRUD) |
| 34 | `soll_attach_evidence` | attach VAL evidence | soll_manager | partial | DEC justification (CRUD) |
| 35 | `soll_remove_evidence` | remove evidence | soll_manager | partial | DEC justification (CRUD) |
| 36 | `soll_export` | export SOLL | — | current | DEC justification (export = no analyse) |
| 37 | `soll_generate_docs` | gen docs from SOLL | soll_query_context | partial | REQ à créer (benefits tri-modal for doc selection) |
| 38 | `restore_soll` | restore from export | soll_manager | current | DEC justification (admin) |
| 39 | `soll_relation_schema` | discoverability tool | — | **bug REQ-AXO-91495** | corriger dans REQ-AXO-91495 |

### 2.3 Tier C — runtime / admin (14 tools)

Doivent exposer `migration_status` aggregate (VAL-AXO-081 critère).

| # | tool | rôle | depends_on | status | soll_link |
|---|---|---|---|---|---|
| 40 | `status` | runtime truth | — | **bug REQ-AXO-91497** (next_best_action loop) | corriger dans REQ-AXO-91497 + ajouter `tool_migration_status` |
| 41 | `health` | health check | — | pending (expose migration metrics) | DEC justification (admin) |
| 42 | `help` | tool routing/schemas | — | pending (compléter contracts) | DEC justification + REQ-AXO-91499 (metadata routing doc) |
| 43 | `job_status` | async job tracker | — | current | DEC justification (admin) |
| 44 | `mcp_surface_diagnostics` | MCP contract diag | — | current | DEC justification (admin) |
| 45 | `project_status` | project-level status | — | current | DEC justification (admin) |
| 46 | `project_registry_lookup` | project code resolve | — | current | DEC justification (admin) |
| 47 | `axon_init_project` | bootstrap project | — | current (CPT-AXO-020) | DEC justification (admin) |
| 48 | `axon_commit_work` | git commit helper | axon_pre_flight_check | current | DEC justification (admin) |
| 49 | `axon_pre_flight_check` | pre-commit check | — | current | DEC justification (admin) |
| 50 | `snapshot_history` | IST snapshot timeline | — | current | DEC justification (admin, support de tier A) |
| 51 | `diagnose_indexing` | indexer diag | — | current | DEC justification (admin) |
| 52 | `embedding_status` | embedder status | — | current | DEC justification (admin) |
| 53 | `debug` | debug tools | — | current | DEC justification (admin) |

### 2.4 Tier D — meta / escape (6 tools)

DEC justification systématique : hors-périmètre tri-modal par construction.

| # | tool | rôle | depends_on | status | soll_link |
|---|---|---|---|---|---|
| 54 | `sql` | raw SQL escape | — | current | DEC justification (raison: escape hatch ; gain_potentiel_pct: 0) |
| 55 | `batch` | batch executor | — | current | DEC justification (raison: meta-tool) |
| 56 | `fs_read` | file read escape | — | current | DEC justification (raison: escape hatch) |
| 57 | `schema_overview` | DB schema dump | — | current | DEC justification (raison: meta) |
| 58 | `list_labels_tables` | DB labels | — | current | DEC justification (raison: meta) |
| 59 | `query_examples` | SQL examples | — | current | DEC justification (raison: meta) |

---

## 3. DAG dépendances Tier A (ordre topologique de migration)

```
                     ┌──── query ────┐
                     │       │       │
                     │   inspect    path
                     │              │ │
                     │              │ ├── bidi_trace
                     │              │ ├── impact ────┐
                     │              │ │              │
                     │              │ └── api_break_check ─── change_safety
                     │              │                          │
                     │              │                          └── simulate_mutation
                     │              └── architectural_drift
                     │
                     ├── retrieve_context ──┬── retrieve_context_layered
                     │       │              │
                     │       │              └── truth_check
                     │       └── why ─── conception_view
                     │
                     └── (foundation)

   Standalone : anomalies, semantic_clones, snapshot_diff, diff
   Aggregator : audit (consomme anomalies + architectural_drift + change_safety)
```

### Vagues de migration recommandées

| Vague | Tools | Justification |
|---|---|---|
| **V1 — foundation** | `query`, `inspect`, `path` | aucune dépendance ; débloque tout le reste |
| **V2 — structural** | `bidi_trace`, `impact`, `api_break_check`, `anomalies`, `architectural_drift` | consomme V1 |
| **V3 — context** | `retrieve_context`, `semantic_clones`, `snapshot_diff`, `diff` | hybride graphe + vector + FTS ; benchmark de référence |
| **V4 — rationale** | `why`, `conception_view`, `truth_check` | consomme retrieve_context + SOLL |
| **V5 — impact-context** | `change_safety`, `simulate_mutation` | consomme V2 |
| **V6 — aggregators** | `audit`, `retrieve_context_layered` | consomme V1..V5 |

---

## 4. Statuts agrégés

| Métrique | Aujourd'hui | Cible MIL-AXO-019 closure |
|---|---:|---:|
| Tier A `migrated` | 0 | 19 |
| Tier A `justified` | 0 | 0 (tous migrables) |
| Tier A `pending` | 19 | 0 |
| Tier B `migrated` | 0 | 6 (les rationale-actifs) |
| Tier B `justified` | 0 | 14 (CRUD purs) |
| Tier B `pending` | 20 | 0 |
| Tier C `exposes_metrics` | 0 | 14 |
| Tier D `justified` | 0 | 6 |
| **Total `pending`** | **59** | **0** |
| **Total `half_baked`** | 0 | 0 (invariant GUI-AXO-1003) |

---

## 5. Liste opérationnelle des nodes SOLL à créer

### 5.1 REQ tool individuelles (25 REQ à créer)

Tier A (19 REQ) :
- `query`, `inspect`, `path`, `bidi_trace`, `impact`, `api_break_check`, `change_safety`, `simulate_mutation`, `architectural_drift`, `anomalies`, `semantic_clones`, `snapshot_diff`, `diff`, `why`, `conception_view`, `truth_check`, `retrieve_context_layered`, `audit`
- `retrieve_context` déjà couvert par REQ-AXO-91489 (slice 5)

Tier B (6 REQ rationale-actifs) :
- `soll_query_context`, `soll_work_plan`, `soll_verify_requirements`, `soll_validate`, `infer_soll_mutation`, `soll_generate_docs`

### 5.2 DEC justifications (28 DEC à créer)

Tier B CRUD (14 DEC) :
- `soll_manager`, `document_intent`, `refine_lattice`, `entrench_nuance`, `axon_apply_guidelines`, `axon_apply_methodology_bundle`, `soll_apply_plan`, `soll_commit_revision`, `soll_rollback_revision`, `soll_attach_evidence`, `soll_remove_evidence`, `soll_export`, `restore_soll`, `soll_relation_schema` (corrigé par REQ-91495)

Tier C admin (14 DEC) :
- 14 tools tier C → 14 DEC `raison: admin, expose_metrics_only` REFINES REQ-AXO-91491

Tier D meta (6 DEC) :
- 6 tools tier D → 6 DEC `raison: escape hatch ou meta, hors-périmètre`

### 5.3 Bugs déjà loggés ré-utilisés

| Tool | Bug REQ existant |
|---|---|
| `status` | REQ-AXO-91497 |
| `soll_relation_schema` | REQ-AXO-91495 |
| `soll_manager` (status pre-write) | REQ-AXO-91498 |
| `soll_manager` (ad-hoc fields) | REQ-AXO-91499 |
| `soll_work_plan` (Wave 1 ignore récents) | REQ-AXO-91500 |
| Bootstrap schema drift | REQ-AXO-91496 |
| Rust call graph = 0 | REQ-AXO-91493 |
| `sql` silent empty | REQ-AXO-91494 |

---

## 6. Notes de synchronisation

- Document `TOOL_MIGRATION_MATRIX.md` = source canonique.
- `soll.node` `metadata.migration_matrix_link = "TOOL_MIGRATION_MATRIX.md#L<row>"` à poser sur chaque REQ tool créée.
- Tool `tool_migration_status` (à implémenter dans REQ-AXO-91491) lit cette matrice + status SOLL pour produire l'agrégat de VAL-AXO-081.

### Critère acceptance REQ-AXO-91491

- 59 lignes présentes dans la matrice (cf. §2). ✓ atteint dans cette version.
- 0 omission vs `status mode=verbose`. ✓ vérifié.
- DAG dépendances explicite (§3). ✓ atteint.
- Pour chaque Tier A+B : path vers REQ tool (à créer) ou DEC justification (à créer). ✓ listé §5.

### Étape suivante

Exécuter en parallèle 25 créations REQ tool + 28 créations DEC justification (53 nodes SOLL au total), avec liens TARGETS depuis MIL-AXO-019 et REFINES vers REQ-AXO-91491. Création par vague (V1→V6) pour préserver l'ordre topologique de migration.
