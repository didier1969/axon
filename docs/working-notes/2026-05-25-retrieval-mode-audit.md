# Audit tri-modal retrieval : outils MCP Axon

**Date :** 2026-05-25
**Scope :** tous les outils MCP dans `src/axon-core/src/mcp/tools_*.rs`
**Méthode :** lecture exhaustive du code source, grep systématique des patterns SQL/RAM

---

## Conventions

| Mode | Marqueurs code |
|---|---|
| **STRUCTUREL RAM** | `IstGraphView`, `process_view()`, `forward_at_radius`, `reverse_at_radius`, `shortest_path`, `bridges_and_articulation`, `structural_sccs`, `layer_violations`, `vf2_subgraph_match` |
| **STRUCTUREL PG** | `public.Symbol`, `public.Edge`, `public.Chunk`, `public.callers_of`, `public.path`, `Symbol s LEFT JOIN Chunk ch` |
| **LEXICAL (FTS)** | `websearch_to_tsquery`, `content_tsv @@`, `ts_rank_cd`, `find_chunk_candidates_via_fts` |
| **SEMANTIQUE** | `batch_embed`, `<=> embedding`, `ChunkEmbedding`, `cosine_expr`, `vector_literal` |

---

## Matrice par outil

### Outils de recherche de symbole (STRUCT seul suffit, STRUCT+SEM toléré)

| Outil | Modes utilisés | Modes recommandés | Gap | Impact |
|---|---|---|---|---|
| **query** | STRUCT PG + SEM (Symbol.embedding cosine) + STRUCT RAM (lexical_symbol_search fallback) + STRUCT PG (graph_r1_neighbors) | STRUCT + SEM | Aucun gap -- conforme, voire surdimensionné. FTS non requis pour symbol lookup. | -- |
| **inspect** | STRUCT PG (Symbol WHERE) + STRUCT RAM (reverse/forward_at_radius pour callers/callees) | STRUCT | Aucun gap. | -- |

### Outils de recherche par question ouverte (les TROIS modes requis)

| Outil | Modes utilisés | Modes recommandés | Gap | Impact |
|---|---|---|---|---|
| **retrieve_context** | STRUCT PG (find_entry_candidates via Symbol+Chunk) + SEM (batch_embed + ChunkEmbedding cosine) + LEX (find_chunk_candidates_via_fts: websearch_to_tsquery + ts_rank_cd) + STRUCT RAM (collect_structural_neighbors via process_view) + RRF fusion (rrf_fusion.rs, derriere AXON_RRF_ENABLED) | STRUCT + LEX + SEM | **Aucun gap critique.** Les trois modes sont implementes. RRF fusion existe mais est gate derriere un env var -- integration partielle. | -- |
| **retrieve_context_layered** | Wrapper autour de retrieve_context + bandes intent/code/recent | STRUCT + LEX + SEM | Aucun gap -- herite des trois modes via retrieve_context. | -- |
| **why** | Delegue a retrieve_context (include_soll=true, include_graph=true) | STRUCT + LEX + SEM | **Aucun gap.** Herite les trois modes. | -- |

### Outils d'analyse de dependances (STRUCT seul)

| Outil | Modes utilisés | Modes recommandés | Gap | Impact |
|---|---|---|---|---|
| **impact** | STRUCT RAM (reverse_at_radius via process_view) + STRUCT PG fallback (public.callers_of) | STRUCT | Aucun gap. | -- |
| **path** (source+sink) | STRUCT RAM (shortest_path via process_view) + STRUCT PG fallback (public.path SQL) | STRUCT | Aucun gap. | -- |
| **bidi_trace** (path sans sink) | STRUCT RAM (reverse/forward_at_radius) + STRUCT PG fallback (empty, degraded flag) | STRUCT | Aucun gap. | -- |
| **api_break_check** | STRUCT RAM (reverse_at_radius depth=1) + STRUCT PG fallback (public.callers_of) | STRUCT | Aucun gap. | -- |
| **simulate_mutation** | STRUCT RAM (reverse_at_radius) + STRUCT PG fallback (public.callers_of) | STRUCT | Aucun gap. | -- |
| **diff** | STRUCT PG (Symbol JOIN Chunk WHERE file_path LIKE) | STRUCT | **Gap mineur** : pas de RAM path (IstGraph n'a pas de reverse index file->symbols). Documente dans le code (REQ-AXO-91520). | BAS |

### Outils de detection d'anomalies (STRUCT + SEM)

| Outil | Modes utilisés | Modes recommandés | Gap | Impact |
|---|---|---|---|---|
| **anomalies** | STRUCT RAM (wrappers, feature_envy, orphan_code, reciprocal_calls, god_objects, bridges, articulation_points, SCCs via process_view + algorithms) + STRUCT PG fallback (get_wrapper_candidates, etc.) + STRUCT PG (soll.Traceability crosswalk pour orphan_code) | STRUCT + SEM | **Gap moyen** : aucune composante semantique. L'ajout de SEM (embedding similarity pour detecter des clusters de fonctions semantiquement proches qui pourraient etre factorisees) enrichirait les resultats. | MOYEN |
| **semantic_clones** | SEM (Symbol.embedding cosine via pgvector `<=>`) + STRUCT RAM (VF2 subgraph isomorphism via neighborhood_subgraph + vf2_subgraph_match) + SEM PG (GraphEmbedding cosine pour la section "Similar Graph Neighborhoods") | STRUCT + SEM | Aucun gap. | -- |
| **architectural_drift** | STRUCT RAM only (layer_violations algorithm sur IstGraph CSR) | STRUCT | Aucun gap -- c'est un tool purement structurel (violations de couches). | -- |

### Outils de diagnostic/audit (STRUCT seul, compteurs)

| Outil | Modes utilisés | Modes recommandés | Gap | Impact |
|---|---|---|---|---|
| **diagnose_indexing** | STRUCT PG (Chunk, Symbol, Edge, IndexedFile compteurs + env vars) | STRUCT | Aucun gap. | -- |
| **truth_check** | STRUCT PG (compteurs canonical writer vs reader) | STRUCT | Aucun gap. | -- |
| **health** | STRUCT PG (coverage, god_objects, counters) | STRUCT | Aucun gap. | -- |
| **embedding_status** | STRUCT PG (Chunk, ChunkEmbedding, Symbol, Edge, IndexedFile compteurs + pipeline env) | STRUCT | Aucun gap. | -- |
| **audit** | STRUCT PG (security, coverage, tech_debt, god_objects, telemetry, circular_deps, domain_leaks) | STRUCT | Aucun gap. | -- |
| **status** | STRUCT PG (compteurs) + runtime state | STRUCT | Aucun gap. | -- |

### Outils de contexte architectural (STRUCT PG)

| Outil | Modes utilisés | Modes recommandés | Gap | Impact |
|---|---|---|---|---|
| **conception_view** | STRUCT PG (Chunk.file_path grouping, Symbol+Chunk interfaces, Edge CALLS flows) | STRUCT | Aucun gap. | -- |
| **change_safety** | STRUCT PG (Symbol.tested, soll.Traceability links) | STRUCT | Aucun gap. | -- |
| **project_status** | Aggregation : status + soll_query_context + conception_view | STRUCT + SOLL | Aucun gap. | -- |

### Outils IST algorithmes (STRUCT RAM seul)

| Outil | Modes utilisés | Modes recommandés | Gap | Impact |
|---|---|---|---|---|
| **ist_snapshot_warm** | PG load -> RAM cache | STRUCT | Aucun gap. | -- |
| **ist_centrality_pagerank** | STRUCT RAM (pagerank_top) | STRUCT | Aucun gap. | -- |
| **ist_structural_sccs** | STRUCT RAM (Tarjan SCC) | STRUCT | Aucun gap. | -- |
| **ist_shortest_path** | STRUCT RAM (BFS bidirectionnel) | STRUCT | Aucun gap. | -- |

### Outils SOLL (PG SOLL direct, pas de retrieval IST)

Tous les outils `soll_*` (`soll_manager`, `soll_work_plan`, `soll_query_context`, `soll_validate`, `soll_export`, `soll_commit_revision`, etc.) operent exclusivement sur le schema `soll.*` PostgreSQL. Pas de retrieval IST implique -- conforme aux regles.

### Outils systeme/skill/help

| Outil | Modes utilisés | Modes recommandés | Gap | Impact |
|---|---|---|---|---|
| **help** | Statique (catalog.rs) | N/A | -- | -- |
| **schema_overview** | STRUCT PG (information_schema) | N/A | -- | -- |
| **query_examples** | Statique | N/A | -- | -- |
| **skill_invoke/skill_list** | SOLL PG + filesystem | N/A | -- | -- |
| **sql** | Passthrough SQL | N/A | -- | -- |

---

## Focus : retrieve_context (outil le plus critique)

### Mode STRUCTUREL -- PRESENT

1. **Ancrage symbole** : `find_entry_candidates` -> `find_symbol_candidates` fait un `SELECT s.id, s.name, s.kind, ... FROM Symbol s LEFT JOIN Chunk ch ON ch.source_id = s.id` avec predicate lexical (LIKE + separator-normalised)
2. **Ancrage fichier** : `find_file_candidates` fait un `SELECT DISTINCT file_path FROM Chunk WHERE ...`
3. **Expansion graphe** : `collect_structural_neighbors` utilise `process_view().forward_at_radius()` (RAM-first, rayon 5-10, cap 20-50) avec fallback PG `query_graph_projection`
4. **Verdict : COMPLET**

### Mode LEXICAL (FTS) -- PRESENT

1. **FTS explicite** : `find_chunk_candidates_via_fts` (ligne 2193) utilise `websearch_to_tsquery('english', ...)` sur `Chunk.content_tsv` (GIN-indexed), classe par `ts_rank_cd`
2. **Integration** : les hits FTS sont merges dans le pool de chunk candidates (ligne 289-301) ; le rerank donne un bonus band `fts_rank * 4, cap 6.0` (ligne 2340-2346) ; `select_supporting_chunks` reserve des slots pour les FTS hits
3. **Gating** : desactivable via `AXON_IST_FTS_DISABLED=1` ou `AXON_HYBRID_RETRIEVAL_DISABLED=1`
4. **Verdict : COMPLET**

### Mode SEMANTIQUE -- PRESENT

1. **Embedding question** : `batch_embed(vec![question])` (ligne 1909) genere le vecteur de la question
2. **Cosine search** : `ChunkEmbedding ce ON ce.chunk_id = c.id ... (ce.embedding <=> vector_literal) < 0.55` (ligne 2066-2070) avec ORDER BY cosine ASC
3. **Service pressure gating** : `RetrievalRuntimeState::allow_semantic_search()` peut desactiver le SEM sous pression
4. **Verdict : COMPLET**

### RRF Fusion

Le module `rrf_fusion.rs` (REQ-AXO-91489) implemente l'algorithme Reciprocal Rank Fusion (Cormack 2009, k=60) avec boost de centralite optionnel. Il est **staging derriere `AXON_RRF_ENABLED`** -- non active en production. Actuellement le reranking utilise un scoring heuristique additif (anchor bonus + semantic distance + FTS rank + lexical hits). La migration vers RRF formel est planifiee (REQ-AXO-324 slice 2).

**Verdict global retrieve_context : les trois modes sont implementes et actifs. Gap RRF integration = staging.**

---

## Gaps identifies (resumes)

| # | Outil | Gap | Severite | Recommandation |
|---|---|---|---|---|
| 1 | **anomalies** | Pas de composante SEM | MOYEN | Ajouter un signal "clusters semantiques" : grouper les fonctions par embedding similarity pour detecter les refactorisations possibles non visibles structurellement |
| 2 | **diff** | Pas de RAM path (file->symbols reverse index absent d'IstGraph) | BAS | Documente et planifie. Ajouter `IstGraph::symbols_in_file(path)` reverse index (REQ-AXO-91520) |
| 3 | **RRF** | Module rrf_fusion.rs existe mais n'est pas branche dans retrieve_context | BAS | Activer quand la precision bench confirme amelioration (REQ-AXO-324 slice 2) |
| 4 | **query** | Pas de FTS | BAS | Non necessaire pour symbol lookup exact. Le mode SEM couvre deja la recherche floue. |

---

## Factorisation / DRY

### 1. Patterns SQL dupliques

#### Pattern A : `Symbol s LEFT JOIN Chunk ch ON ch.source_id = s.id AND ch.source_type = 'symbol'`

Apparait dans :
- `tools_dx.rs:653-654` (query, branche cosine, workspace)
- `tools_dx.rs:665-666` (query, branche cosine, project)
- `tools_dx.rs:682-683` (query, fallback lexical, workspace)
- `tools_dx.rs:692-693` (query, fallback lexical, project)
- `tools_dx.rs:704-705` (query, no-embedding, workspace)
- `tools_dx.rs:715-716` (query, no-embedding, project)
- `tools_dx.rs:1008` (query_from_chunks)
- `tools_dx.rs:1047` (query_from_chunks, project-scoped)
- `tools_risk.rs:629` (diff, file_path LIKE)
- `tools_context.rs:1569` (find_symbol_candidates)
- `tools_context.rs:1633` (find_symbol_candidates, project-scoped)

**11 occurrences** en 3 fichiers du meme pattern JOIN. Les colonnes SELECT varient mais le FROM/JOIN est identique.

#### Pattern B : `SELECT id FROM Symbol WHERE name = $sym OR id = $sym LIMIT 1`

Apparait dans :
- `tools_context.rs:67-73` (resolve_scoped_symbol_id_canonical)
- `tools_governance.rs:836-839` (semantic_clones, inline)
- `tools_governance.rs:374` (build_graph_clone_section, inline)

Le helper `resolve_scoped_symbol_id_canonical` est correctement factorise et utilise par la majorite des outils (path, inspect, impact, simulate_mutation, change_safety, bidi_trace, api_break_check). **Exceptions** : `semantic_clones` et `build_graph_clone_section` dupliquent le pattern en inline.

#### Pattern C : `process_view()` + `view.is_warm(project)` + `view.reverse_at_radius(...)` / `view.forward_at_radius(...)`

Apparait dans :
- `tools_dx.rs:1519-1534` (inspect)
- `tools_dx.rs:1858-1876` (bidi_trace)
- `tools_dx.rs:1997-2004` (api_break_check)
- `tools_risk.rs:121-134` (impact)
- `tools_risk.rs:775-787` (simulate_mutation)
- `tools_framework_anomalies.rs:67,143,668` (anomalies, 3 fois)
- `tools_governance.rs:818,1012` (semantic_clones, architectural_drift)
- `tools_framework_path.rs:117-127` (path)
- `tools_context.rs:2683-2731` (collect_structural_neighbors)

**15+ occurrences** du pattern view/is_warm/traverse. Chaque site repete la meme logique : instancier la vue, verifier si le cache est warm, executer la traversal, gerer le fallback PG.

#### Pattern D : `suggest_scoped_symbols_canonical` + "not found" envelope

Apparait dans :
- `tools_dx.rs:1321` + lignes 1322-1495 (inspect)
- `tools_dx.rs:1772` + lignes 1793-1851 (bidi_trace)
- `tools_risk.rs:422` + lignes 423-488 (impact)
- `tools_risk.rs:709` + lignes 710-767 (simulate_mutation)

Chaque site construit le meme `json!({ "content": ..., "data": { "symbol_found": false, "suggestions": ..., "next_action": ... }})`. La structure "symbol-not-found with suggestions" est un contrat UI repete 4 fois.

### 2. Fonctions partagees existantes

| Fonction | Fichier | Utilisee par |
|---|---|---|
| `resolve_scoped_symbol_id_canonical` | `tools_context.rs:60` | inspect, bidi_trace, path, impact, simulate_mutation, change_safety, api_break_check |
| `suggest_scoped_symbols_canonical` | `tools_context.rs:84` | inspect, impact, simulate_mutation (via alias `suggest_scoped_symbols` dans tools_risk) |
| `find_symbol_candidates` | `tools_context.rs:1557` | retrieve_context seulement |
| `find_file_candidates` | `tools_context.rs:1668` | retrieve_context seulement |
| `find_chunk_candidates_via_fts` | `tools_context.rs:2193` | retrieve_context seulement |
| `query_graph_r1_neighbors` | `tools_dx.rs:538` | query seulement |
| `symbol_search_predicate` | `tools_dx.rs:300` | query seulement |
| `chunk_search_predicate` | `tools_dx.rs:307` | query seulement |

**Constat** : les primitives FTS (`find_chunk_candidates_via_fts`) et semantique (`batch_embed` + cosine query) sont encapsulees dans `tools_context.rs` et **ne sont pas exposees aux autres outils**. `query` reimplemente sa propre recherche semantique en inline.

### 3. Primitives communes recommandees

#### Niveau 0 : primitives de retrieval

| Primitive | Description | Existe ? | Localisation |
|---|---|---|---|
| `resolve_symbol(name, project) -> Option<SymbolId>` | Resolution de symbole exact | OUI | `resolve_scoped_symbol_id_canonical` |
| `suggest_symbols(name, project, limit) -> Vec<Symbol>` | Suggestions fuzzy | OUI | `suggest_scoped_symbols_canonical` |
| `symbol_search(query, project, limit) -> Vec<Symbol>` | Recherche Symbol par predicate lexical | PARTIEL | `symbol_search_predicate` (query only), `find_symbol_candidates` (retrieve_context only) |
| `fts_search(query, project, limit) -> Vec<ChunkCandidate>` | FTS sur Chunk.content_tsv | OUI | `find_chunk_candidates_via_fts` (retrieve_context only) |
| `vector_search(query, project, limit) -> Vec<ChunkCandidate>` | pgvector cosine sur ChunkEmbedding | NON factorise | Inline dans `find_chunk_candidates` et `axon_query` |
| `graph_traverse(symbol_id, direction, depth, project) -> Vec<SymbolId>` | Traversal IST RAM + fallback PG | NON factorise | 15+ inline instances |
| `materialize_symbol_names(ids) -> Map<Id, Name>` | Batch Symbol lookup par id | OUI | `materialize_symbol_rows` (tools_dx.rs:30) |

#### Niveau 1 : primitives composees

| Primitive | Composition | Existe ? |
|---|---|---|
| `hybrid_search(query, project)` | symbol_search + fts_search + vector_search + RRF | PARTIEL (retrieve_context, sans RRF formel) |
| `blast_radius(symbol, depth, project)` | graph_traverse(reverse) + materialize | NON factorise (repete dans impact, simulate_mutation, api_break_check) |
| `symbol_not_found_response(symbol, project, tool)` | suggest_symbols + standard JSON envelope | NON factorise (repete 4 fois) |

#### Niveau 2 : outils MCP (consomment les primitives)

| Outil | Primitives Niveau 0/1 requises |
|---|---|
| query | symbol_search + vector_search (optionnel) |
| inspect | resolve_symbol + graph_traverse(reverse+forward, depth=1) |
| retrieve_context | hybrid_search + graph_traverse + SOLL join |
| impact | resolve_symbol + blast_radius |
| path | resolve_symbol(x2) + graph_traverse(shortest_path) |
| anomalies | graph_traverse(multiple patterns) |
| why | hybrid_search (via retrieve_context) |

### 4. Tableau de duplication -- patterns a factoriser

| Pattern | Occurrences | Fichiers | Primitive cible |
|---|---|---|---|
| `Symbol s LEFT JOIN Chunk ch ON ch.source_id = s.id` | 11 | tools_dx, tools_risk, tools_context | `symbol_search()` ou `resolve_symbol_with_uri()` |
| `process_view() + is_warm + forward/reverse_at_radius` | 15+ | tools_dx, tools_risk, tools_governance, tools_framework_anomalies, tools_framework_path, tools_context | `graph_traverse()` |
| JSON "symbol_not_found" envelope avec suggestions | 4 | tools_dx, tools_risk | `symbol_not_found_response()` |
| `batch_embed + vector_literal + cosine expr` | 2 | tools_dx:627, tools_context:1909 | `vector_search()` |
| `SELECT id FROM Symbol WHERE name=$sym OR id=$sym` | 3 | tools_context, tools_governance (x2) | `resolve_symbol()` (deja existe, pas utilise partout) |

### Priorite de refactorisation

1. **HAUTE** : `graph_traverse()` -- 15+ duplications, chaque site repete la logique warm/cold + fallback PG. Un helper generique reduirait ~300 lignes de code et uniformiserait le reporting `surfaces_used/degraded`.

2. **HAUTE** : `symbol_not_found_response()` -- 4 blocs JSON quasi-identiques (~50 lignes chacun). Un builder factorise garantirait la coherence du contrat LLM.

3. **MOYENNE** : `vector_search()` -- 2 sites seulement, mais la logique `batch_embed + vector_literal + error handling` est non-triviale et susceptible de diverger.

4. **BASSE** : `Symbol LEFT JOIN Chunk` -- les variantes SELECT sont suffisamment differentes pour que la factorisation soit plus couteuse que le gain. Mieux vaut un query builder parametre qu'un helper rigide.

---

## Verdict global

**retrieve_context** est complet tri-modal : structurel (IST RAM + PG), lexical (FTS websearch_to_tsquery), semantique (pgvector cosine). Les trois modes sont actifs en production. Le RRF formel (Cormack k=60) est implemente mais gate derriere un env var.

Les outils structurels (impact, path, bidi_trace, api_break_check, simulate_mutation) sont correctement RAM-first avec PG fallback.

Les gaps identifies sont mineurs :
- `anomalies` pourrait beneficier d'un signal semantique (clusters) -- MOYEN
- `diff` n'a pas de RAM path -- BAS, documente
- RRF non active -- BAS, planifie

La dette principale est dans la **duplication de code** (graph_traverse 15x, symbol_not_found 4x) -- pas un gap de retrieval mais un risque de divergence et de maintenance.
