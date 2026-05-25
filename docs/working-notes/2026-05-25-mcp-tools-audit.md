# Audit MCP Tools — Sources, Legacy Paths, Verdicts

**Date:** 2026-05-25
**Scope:** Tous les outils MCP publics exposes par axon-brain (catalog.rs)
**Methode:** Lecture exhaustive de catalog.rs + tous les fichiers d'implementation tools_*.rs

## Contexte Architecture

- **IST RAM** : `IstGraphView` (CSR snapshot in-process, PIL-AXO-9002). Canonical pour les traversals structurels.
- **PG IST** : `public.Edge` (persistence layer), `public.Symbol`, `public.Chunk`, `public.ChunkEmbedding`, `public.IndexedFile`. Canonical pour persistence.
- **PG SOLL** : `soll.Node`, `soll.Edge`, `soll.Revision`, `soll.RevisionChange`, `soll.Traceability`, `soll.ProjectCodeRegistry`.
- **Legacy retire** : `public.File` (remplace par `public.IndexedFile` + `public.Chunk`), `public.CONTAINS` / `public.CALLS` / `public.CALLS_NIF` (remplace par `public.Edge`), AGE (MIL-AXO-017), DuckDB (REQ-AXO-271).
- **`skip_legacy_relations()` = `true` invariant** : la methode retourne toujours `true` (graph.rs:133), donc les branches legacy `CALLS`/`CALLS_NIF`/`CONTAINS` sont mortes au runtime.

## Legende Verdicts

| Verdict | Signification |
|---|---|
| **OBSOLETE** | Fonctionnalite retiree, l'outil ne produit plus de resultats utiles |
| **LEGACY_PATH** | Fonctionne, mais utilise des tables/chemins retires (`CONTAINS`, `CALLS`, `File`, AGE Cypher) au lieu du chemin canonical (`public.Edge`, IST RAM, `public.Chunk`) |
| **REDONDANT** | Doublon fonctionnel d'un autre outil |
| **UTILE** | Fonctionne correctement sur le chemin canonical |

---

## Tableau d'Audit

### OBSOLETE

| Outil | Source | Chemin Legacy | Detail | Verdict |
|---|---|---|---|---|
| `refine_lattice` | PG_IST | **OUI** — AGE Cypher `MATCH (elixir:Symbol)<-[:CONTAINS]-(e_file:File)` + `MERGE (elixir)-[:CALLS_NIF]->(rust)` | Utilise exclusivement la syntaxe AGE Cypher (MATCH/MERGE) retiree depuis MIL-AXO-017. Le query passe par `graph_store.query_json()` qui ne supporte plus AGE. Resultat systematique : erreur ou vide. | **OBSOLETE** |
| `list_labels_tables` | PG_IST | **OUI** — reference `File`, `CONTAINS`, `CALLS`, `CALLS_NIF` dans les WHERE IN et columns discovery | Enumerere des tables legacy (`File`, `CALLS`, `CALLS_NIF`, `CONTAINS`) qui n'existent plus ou sont vides. Induit l'utilisateur en erreur sur le schema reel. Devrait lister `public.Edge`, `public.IndexedFile`, `public.Chunk`, `public.ChunkEmbedding`. | **OBSOLETE** |

### LEGACY_PATH

| Outil | Source | Chemin Legacy | Detail | Verdict |
|---|---|---|---|---|
| `query` | PG_IST + VECTOR + IST_RAM | **OUI** — `JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id` dans toutes les branches (symbol search, chunk fallback, without_contains) | La branche primaire du `query` tool joint `Symbol` via `CONTAINS` + `File`. Comme `skip_legacy_relations = true`, la table `CONTAINS` est vide/absente, donc le query principal retourne 0 rows et tombe dans la branche fallback `axon_query_from_chunks` ou `axon_query_without_contains`. La branche RAM lexical (`graph_ram_lexical`) et la branche `graph_r1` via `public.Edge` sont fonctionnelles. Resultats degrades : l'URI (file path) est souvent absente. | **LEGACY_PATH** |
| `inspect` | PG_IST + IST_RAM | **OUI** — le symbol detail utilise `JOIN CONTAINS` + `JOIN File` dans `axon_inspect` (tools_dx.rs:1548-1579 branch) | Quand `skip_legacy_relations = true`, la branche qui joint les callers via CALLS + CONTAINS est court-circuitee, mais la resolution de symbole et le fallback fonctionnent. Callers/callees proviennent de IST RAM quand warm. | **LEGACY_PATH** |
| `diff` | PG_IST | **OUI** — `JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id` (tools_risk.rs:629) | Chaque fichier touche par le diff est resolu via `Symbol JOIN CONTAINS JOIN File`. Comme CONTAINS est vide, la resolution symbole-par-fichier retourne vide. Le tool rapporte `surfaces_used: ["graph_pg"]` mais ne produit rien d'utile. | **LEGACY_PATH** |
| `conception_view` | PG_IST | **OUI** — `tools_framework_conception.rs` joint `CALLS` + `CONTAINS` pour flows et interfaces | Les flows et interfaces sont resolus via `CALLS` + `CONTAINS`. Comme `skip_legacy_relations = true`, les queries sont court-circuitees (retournent 0 modules/flows). Le tool retourne des compteurs a zero. | **LEGACY_PATH** |
| `diagnose_indexing` | PG_IST | **Partiellement** — reference `CALLS` / `CALLS_NIF` dans les compteurs, mais gate par `skip_legacy_relations` | Les compteurs `calls_direct` / `calls_nif` sont conditionnes par `skip_legacy_relations` : quand true (toujours), ils retournent 0. La cause `call_graph_gap` est donc toujours declenchee faussement. Le reste (IndexedFile, Chunk, Symbol, drain analysis) est canonical. | **LEGACY_PATH** |
| `truth_check` | PG_IST | **Partiellement** — liste `CALLS`, `CALLS_NIF`, `CONTAINS` dans les checks, mais gate par `skip_legacy_relations` | Les checks legacy sont sautes quand `skip_legacy_relations = true` (toujours). Le tool fonctionne sur `IndexedFile` + `Symbol` seulement. Manque `Edge`, `Chunk`, `ChunkEmbedding` dans les invariants. | **LEGACY_PATH** |
| `retrieve_context` | PG_IST + VECTOR + PG_SOLL + IST_RAM | **Partiellement** — `find_symbol_candidates` et `find_file_candidates` joingnent `CONTAINS` + `File` dans tools_context.rs:1571-1572, 1636-1637, 2071-2091 | Les entry candidates et chunk candidates utilisent `LEFT JOIN CONTAINS` + `LEFT JOIN File` pour materialiser les URIs. Comme CONTAINS est vide, les URIs sont souvent null. Le FTS, vector, graph expansion et SOLL join fonctionnent correctement. Degradation partielle de la qualite des resultats. | **LEGACY_PATH** |
| `why` | PG_IST + VECTOR + PG_SOLL + IST_RAM | **Herite de `retrieve_context`** | Wrapper autour de `retrieve_context` avec `include_soll=true`. Meme degradation partielle. | **LEGACY_PATH** |
| `retrieve_context_layered` | PG_IST + VECTOR + PG_SOLL + IST_RAM | **Herite de `retrieve_context`** | Wrapper autour de `retrieve_context` + re-organisation en bandes (intent/code/recent). Meme degradation. | **LEGACY_PATH** |
| `audit` | PG_IST | **Partiellement** — `get_circular_dependencies`, `get_domain_leakage` etc. dans graph_analytics utilisent potentiellement CONTAINS/CALLS | Les fonctions `graph_analytics` referent a des structures legacy. Dependant des implementations de `graph_store`. Surfaces_used = `["graph_pg"]`. | **LEGACY_PATH** |

### UTILE

| Outil | Source | Chemin Legacy | Detail | Verdict |
|---|---|---|---|---|
| `help` | NONE (static) | Non | Retourne le catalogue statique. Pur metadata. | **UTILE** |
| `status` | PG_IST + PG_SOLL + RUNTIME | Non | Runtime mode, profile, pressure signals, public tools. Utilise les tables canoniques. | **UTILE** |
| `project_status` | PG_IST + PG_SOLL + RUNTIME | Non | Compose `status` + `soll_query_context` + `conception_view`. La partie conception herite du legacy path mais le reste est canonical. | **UTILE** |
| `project_registry_lookup` | PG_SOLL | Non | Lookup dans `soll.ProjectCodeRegistry`. Canonical. | **UTILE** |
| `mcp_surface_diagnostics` | NONE (static) | Non | Diagnostic de surface MCP. Metadata pur. | **UTILE** |
| `debug` | PG_IST + RUNTIME | Non | Diagnostic systeme avance. Utilise les tables canoniques. | **UTILE** |
| `embedding_status` | PG_IST | Non | Compteurs sur `public.Chunk`, `public.ChunkEmbedding`, `public.Symbol`, `public.Edge`, `public.IndexedFile`. 100% canonical post-REQ-AXO-901653. | **UTILE** |
| `fs_read` | FILESYSTEM | Non | Lecture fichier directe. Pas de DB. | **UTILE** |
| `sql` | PG_IST + PG_SOLL | Non | Raw SQL read-only. L'utilisateur decide la query. | **UTILE** |
| `schema_overview` | PG_IST + PG_SOLL | Non | `information_schema.tables/columns` sur schemas `main` + `soll`. | **UTILE** |
| `query_examples` | NONE (static) | Non | Exemples de queries. Migre vers pipeline_v2 canonical (REQ-AXO-901653 slice-5d). | **UTILE** |
| `batch` | META | Non | Multi-call orchestrator. Pas de source propre. | **UTILE** |
| `job_status` | RUNTIME | Non | Tracking async des jobs de mutation. | **UTILE** |
| `impact` | IST_RAM + PG_IST + PG_SOLL | Non | RAM-first via `IstGraphView.reverse_at_radius()`. PG fallback via `public.callers_of()` sur `public.Edge`. SOLL join via `soll.Traceability`. Canonical. | **UTILE** |
| `simulate_mutation` | IST_RAM + PG_IST | Non | RAM-first via `IstGraphView.reverse_at_radius()`. PG fallback via `public.callers_of()`. Canonical. | **UTILE** |
| `path` | IST_RAM + PG_IST | Non | RAM-first via `IstGraphView.shortest_path()`. PG fallback via `public.path()` SQL function sur `public.Edge`. Canonical. | **UTILE** |
| `bidi_trace` | IST_RAM + PG_IST | Non | RAM-first via `forward_at_radius` / `reverse_at_radius`. PG fallback donne vide (CALLS mortes) mais documente comme `surfaces_degraded`. | **UTILE** |
| `api_break_check` | IST_RAM + PG_IST | Non | RAM-first via `reverse_at_radius(depth=1)`. PG fallback via `public.callers_of`. Canonical. | **UTILE** |
| `anomalies` | IST_RAM + PG_IST + PG_SOLL | Non | RAM-first pour wrappers, feature_envy, orphan_code, circular deps. PG fallback pour detours, abstraction_detours. SOLL traceability crosswalk. | **UTILE** |
| `semantic_clones` | VECTOR + IST_RAM | Non | pgvector cosine pre-filter + VF2 graph isomorphism via IstGraphView. Canonical. | **UTILE** |
| `architectural_drift` | IST_RAM | Non | `layer_violations` sur IstGraphView. Require warm snapshot. 100% RAM. | **UTILE** |
| `change_safety` | PG_IST + PG_SOLL | Non | `Symbol.tested` flag + `soll.Traceability` count. PG canonical. | **UTILE** |
| `health` | PG_IST | Non | Coverage score + god objects via `graph_store` aggregators. | **UTILE** |
| `ist_snapshot_warm` | PG_IST -> IST_RAM | Non | Charge le snapshot CSR depuis PG `public.Symbol` + `public.Edge` vers RAM. Canonical. | **UTILE** |
| `ist_centrality_pagerank` | IST_RAM | Non | PageRank sur le CSR in-memory. 100% RAM. | **UTILE** |
| `ist_structural_sccs` | IST_RAM | Non | Tarjan SCC sur le CSR in-memory. 100% RAM. | **UTILE** |
| `ist_shortest_path` | IST_RAM | Non | BFS shortest path sur le CSR in-memory. 100% RAM. | **UTILE** |
| `snapshot_history` | FILESYSTEM | Non | Historique des snapshots structurels (fichiers JSON locaux). | **UTILE** |
| `snapshot_diff` | FILESYSTEM | Non | Diff entre snapshots structurels. | **UTILE** |
| `soll_manager` | PG_SOLL | Non | CRUD sur `soll.Node` / `soll.Edge`. Canonical. | **UTILE** |
| `soll_apply_plan` | PG_SOLL | Non | Batch plan application. Canonical. | **UTILE** |
| `soll_commit_revision` | PG_SOLL | Non | Commit preview -> revision. Canonical. | **UTILE** |
| `soll_query_context` | PG_SOLL | Non | Read visions/requirements/decisions/revisions. Canonical. | **UTILE** |
| `soll_work_plan` | PG_SOLL | Non | Work plan read-only. Canonical. | **UTILE** |
| `soll_verify_requirements` | PG_SOLL | Non | Coverage verification. Canonical. | **UTILE** |
| `soll_validate` | PG_SOLL | Non | Structural validation. Canonical. | **UTILE** |
| `soll_acyclic_audit` | PG_SOLL | Non | Tarjan SCC sur le graphe SOLL. Canonical. | **UTILE** |
| `soll_attach_evidence` | PG_SOLL | Non | Traceability rows. Canonical. | **UTILE** |
| `soll_remove_evidence` | PG_SOLL | Non | Remove traceability rows. Canonical. | **UTILE** |
| `soll_rollback_revision` | PG_SOLL | Non | Rollback via journal. Canonical. | **UTILE** |
| `soll_export` | PG_SOLL + FILESYSTEM | Non | Export SOLL vers Markdown. Canonical. | **UTILE** |
| `soll_generate_docs` | PG_SOLL + FILESYSTEM | Non | Gen docs HTML+Mermaid. Canonical. | **UTILE** |
| `restore_soll` | PG_SOLL + FILESYSTEM | Non | Restore SOLL depuis export. Canonical. | **UTILE** |
| `soll_relation_schema` | PG_SOLL | Non | Relation policy lookup. Canonical. | **UTILE** |
| `infer_soll_mutation` | PG_SOLL | Non | Read-only assistive analysis. Canonical. | **UTILE** |
| `entrench_nuance` | PG_SOLL | Non | Stabilize nuance workflow. Canonical. | **UTILE** |
| `document_intent` | PG_SOLL | Non | Auto-classify + create SOLL entity. Canonical. | **UTILE** |
| `re_anchor` | PG_SOLL | Non | Single-call re-anchor packet. Canonical. | **UTILE** |
| `axon_init_project` | PG_SOLL | Non | Project initialization. Canonical. | **UTILE** |
| `axon_apply_guidelines` | PG_SOLL | Non | Instantiate global rules. Canonical. | **UTILE** |
| `axon_apply_methodology_bundle` | PG_SOLL | Non | Apply methodology bundle. Canonical. | **UTILE** |
| `axon_pre_flight_check` | PG_SOLL + FILESYSTEM | Non | Dry-run validation. Canonical. | **UTILE** |
| `axon_commit_work` | PG_SOLL + FILESYSTEM + GIT | Non | Validate + commit. Canonical. | **UTILE** |
| `skill_list` | PG_SOLL | Non | List SKI entities. Canonical. | **UTILE** |
| `skill_invoke` | PG_SOLL | Non | Resolve SKI entity. Canonical. | **UTILE** |
| `prompt_template_get` | PG_SOLL | Non | Resolve PRT entity. Canonical. | **UTILE** |
| `resume_vectorization` | PG_IST | Non | Backfill vectorization queue. Internal/indexer-only. | **UTILE** |
| `rescan_project` | PG_IST + RUNTIME | Non | Force delta/full re-scan. Canonical pipeline_v2. | **UTILE** |

---

## Synthese

| Verdict | Nombre | Outils |
|---|---|---|
| **OBSOLETE** | 2 | `refine_lattice`, `list_labels_tables` |
| **LEGACY_PATH** | 10 | `query`, `inspect`, `diff`, `conception_view`, `diagnose_indexing`, `truth_check`, `retrieve_context`, `why`, `retrieve_context_layered`, `audit` |
| **UTILE** | 45 | (tous les autres) |
| **REDONDANT** | 0 | — |

## Recommandations Prioritaires

### P0 — Retirer les outils obsoletes

1. **`refine_lattice`** : L'outil utilise exclusivement AGE Cypher (`MATCH`/`MERGE`) qui est retire. A supprimer du catalogue ou migrer vers `public.Edge` + `INSERT INTO public.Edge` pour les bridges NIF.

2. **`list_labels_tables`** : Reference un schema entierement legacy (`File`, `CONTAINS`, `CALLS`, `CALLS_NIF`). Doit etre reecrit pour montrer le schema actuel : `IndexedFile`, `Symbol`, `Chunk`, `ChunkEmbedding`, `Edge`.

### P1 — Migrer les chemins legacy des outils critiques

3. **`query`** (critique, outil le plus utilise) : La branche primaire `Symbol JOIN CONTAINS JOIN File` est morte. Migrer vers `Symbol JOIN public.Edge JOIN public.Chunk` ou directement materialiser l'URI depuis `public.Chunk.file_path`. Le fallback RAM lexical fonctionne mais n'a pas l'URI.

4. **`inspect`** : Meme pattern CONTAINS+File. Migrer callers/callees vers IST RAM (deja fait partiellement) et URI resolution vers `Chunk.file_path`.

5. **`retrieve_context`** / **`why`** / **`retrieve_context_layered`** : Les `find_symbol_candidates` et `find_file_candidates` joignent `CONTAINS` + `File`. Migrer vers `Chunk.file_path` pour la resolution d'URI.

6. **`diff`** : Migrer `Symbol JOIN CONTAINS JOIN File` vers `Symbol + Chunk WHERE file_path LIKE '%...'`.

7. **`conception_view`** : Migrer les flows/interfaces depuis `CALLS` + `CONTAINS` vers `public.Edge` ou IST RAM.

### P2 — Corriger les diagnostics faussement degrades

8. **`diagnose_indexing`** : Les compteurs `calls_direct` / `calls_nif` sont toujours 0 car gates par `skip_legacy_relations`. Devrait compter `public.Edge WHERE relation_type IN ('CALLS','CALLS_NIF')`.

9. **`truth_check`** : Ajouter `Edge`, `Chunk`, `ChunkEmbedding` aux invariants verifies. Retirer la branche `CALLS`/`CALLS_NIF`/`CONTAINS` (deja morte).

10. **`audit`** : Verifier que les `graph_analytics` sous-jacents (`get_circular_dependencies`, `get_domain_leakage`, etc.) n'utilisent pas de tables legacy.

---

## Pattern de migration commun

Le pattern `Symbol JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id` doit etre remplace par :

```sql
-- URI d'un symbole via Chunk
SELECT s.name, s.kind, c.file_path
FROM public.Symbol s
LEFT JOIN public.Chunk c ON c.source_id = s.id AND c.source_type = 'symbol'
WHERE ...
```

Ou pour les edges :
```sql
-- Callers d'un symbole via Edge
SELECT e.source_id, e.relation_type
FROM public.Edge e
WHERE e.target_id = $target AND e.relation_type IN ('CALLS', 'CALLS_NIF')
```
