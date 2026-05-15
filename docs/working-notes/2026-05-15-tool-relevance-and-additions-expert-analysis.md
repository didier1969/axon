# Tool Relevance & Additions — Expert Analysis (MIL-AXO-019 scope)

Pointeurs canoniques :
- Inventaire 59 tools : `docs/skills/axon-engineering-protocol/TOOL_MIGRATION_MATRIX.md`
- Concept graphe IST RAM : `docs/working-notes/2026-05-15-concept-ist-in-memory-graph.md`
- Évaluation SOLL : `docs/working-notes/2026-05-15-soll-evaluation-session-report.md`
- Routing actuel : `docs/skills/axon-engineering-protocol/SKILL.md`

Toute info structurelle (volumétrie, DAG dépendances, vagues, ROI CTE→RAM) reste dans les sources ci-dessus. Ce document ne ré-énonce que ce qui est nécessaire pour juger.

---

## Partie 1 — Pertinence des 59 tools existants

Légende :
- `intrinsic` : pertinence indépendamment du tri-modal (1=marginal, 5=indispensable produit dev-LLM).
- `tri_modal` : gain à migrer sur graphe RAM + RRF authentique tri-modal (1=aucun gain, 5=transformation).
- `redondance` : tool le plus proche fonctionnellement.
- `recommandation` ∈ {`keep`, `must_migrate`, `merge_into:X`, `deprecate`, `niche_keep`}.

### 1.1 Tier A — analyse pure (19 tools)

| # | tool | intrinsic | tri_modal | redondance | recommandation | justification courte |
|---|---|---:|---:|---|---|---|
| 1 | `query` | 5 | 4 | — | `must_migrate` | feuille de toute composition ; doit retourner ranks vector+FTS+graph natifs |
| 2 | `inspect` | 5 | 3 | query (overlap detail) | `merge_into:query` | `inspect` = `query` + `expand=full` ; un seul tool paramétré |
| 3 | `path` | 4 | 5 | bidi_trace | `must_migrate` | bidi BFS RAM = 1-3 ms ; absorbe `bidi_trace` |
| 4 | `bidi_trace` | 2 | 2 | path | `merge_into:path` | dégénère vers `path` une fois RAM (mêmes algos) |
| 5 | `impact` | 5 | 5 | api_break_check, change_safety | `must_migrate` | radius illimité + centrality-weighted blast |
| 6 | `api_break_check` | 3 | 3 | impact | `merge_into:impact` | sous-cas paramétrique de `impact` (filtre relation_type=CALLS sur public symbols) |
| 7 | `change_safety` | 4 | 4 | impact, simulate_mutation | `merge_into:simulate_mutation` | superposition utile uniquement comme verdict packagé |
| 8 | `simulate_mutation` | 4 | 5 | change_safety | `must_migrate` | reachability-diff avant/après = trivial RAM, impossible CTE |
| 9 | `architectural_drift` | 3 | 4 | anomalies | `merge_into:anomalies` | drift = sous-classe d'anomaly (delta SCC/bridges entre snapshots) |
| 10 | `anomalies` | 4 | 5 | architectural_drift | `must_migrate` | bénéficie de SCC enum + bridges + cycles complets |
| 11 | `semantic_clones` | 4 | 5 | — | `must_migrate` | vector + structural fingerprint (signature CSR sub-graph) ; aujourd'hui vector-only |
| 12 | `snapshot_diff` | 3 | 3 | diff | `merge_into:diff` | doublon à 80 % (diff IST entre snapshots) |
| 13 | `diff` | 3 | 3 | snapshot_diff | `keep` | absorbe `snapshot_diff` après merge |
| 14 | `why` | 4 | 4 | conception_view, retrieve_context | `must_migrate` | RRF tri-modal lui apporte le SOLL+chunks+graph en un seul ranking |
| 15 | `conception_view` | 2 | 2 | why, soll_query_context | `merge_into:why` | sortie déjà incluse dans `why` étendu |
| 16 | `truth_check` | 3 | 4 | retrieve_context, why | `niche_keep` | utile mais audience étroite ; redirige vers nouveau `claim_verify` (cf. P3) |
| 17 | `retrieve_context` | 5 | 5 | retrieve_context_layered | `must_migrate` | pivot RRF authentique ; absorbe `retrieve_context_layered` |
| 18 | `retrieve_context_layered` | 2 | 2 | retrieve_context | `merge_into:retrieve_context` | `layered=true` paramètre suffit (cf. P2) |
| 19 | `audit` | 3 | 4 | anomalies + change_safety | `niche_keep` | agrégateur "rapport humain" ; utile peu fréquent, garder mais ré-exprimer sur RAM |

### 1.2 Tier B — SOLL CRUD / governance (20 tools)

| # | tool | intrinsic | tri_modal | redondance | recommandation | justification |
|---|---|---:|---:|---|---|---|
| 20 | `soll_manager` | 5 | 3 | — | `keep` | core writer ; gain RAM = pre-write cycle check < 5 ms |
| 21 | `soll_query_context` | 5 | 5 | — | `must_migrate` | RRF tri-modal sur SOLL ×3 pertinence (cf. §3.2 soll-eval) |
| 22 | `soll_work_plan` | 4 | 5 | — | `must_migrate` | PageRank+decay RAM corrige REQ-AXO-91500 |
| 23 | `soll_verify_requirements` | 3 | 3 | soll_validate | `merge_into:soll_validate` | redondance verbale ; un seul tool avec mode `verify` vs `lint` |
| 24 | `soll_validate` | 3 | 3 | soll_verify_requirements | `keep` | absorbe verify ; bénéficie pre-write cycles |
| 25 | `document_intent` | 4 | 2 | soll_manager | `keep` | wrapper auto-classifier (CPT-AXO-019) — vrai UX gain |
| 26 | `refine_lattice` | 2 | 2 | soll_manager | `niche_keep` | spécialité, mais usage 1 / semaine au mieux |
| 27 | `entrench_nuance` | 2 | 2 | soll_manager | `niche_keep` | idem ; envisager merge si métrique d'usage < 1 / mois |
| 28 | `infer_soll_mutation` | 4 | 5 | — | `must_migrate` | vraie valeur : code→SOLL ; gagne RRF tri-modal pour matcher REQ existants |
| 29 | `axon_apply_guidelines` | 3 | 2 | axon_apply_methodology_bundle | `merge_into:axon_apply_methodology_bundle` | méthodologie = guideline bundle + DEC bundle ; un seul applier |
| 30 | `axon_apply_methodology_bundle` | 3 | 2 | axon_apply_guidelines | `keep` | absorbe guidelines |
| 31 | `soll_apply_plan` | 4 | 2 | soll_manager | `keep` | batch (dry_run) — irremplaçable côté workflow |
| 32 | `soll_commit_revision` | 3 | 2 | — | `keep` | CRUD nécessaire (preview/commit pattern) |
| 33 | `soll_rollback_revision` | 4 | 2 | — | `keep` | safety net mandatory (politique no-delete SOLL) |
| 34 | `soll_attach_evidence` | 4 | 2 | — | `keep` | VAL workflow |
| 35 | `soll_remove_evidence` | 3 | 2 | soll_attach_evidence | `keep` | symétrique inévitable |
| 36 | `soll_export` | 3 | 1 | — | `keep` | admin |
| 37 | `soll_generate_docs` | 3 | 4 | — | `must_migrate` | sélection des nodes via RRF tri-modal = qualité doc générée ×3 |
| 38 | `restore_soll` | 3 | 1 | — | `keep` | DR |
| 39 | `soll_relation_schema` | 5 | 2 | — | `keep` | discoverability — corriger bug REQ-AXO-91495 |

### 1.3 Tier C — runtime / admin (14 tools)

| # | tool | intrinsic | tri_modal | redondance | recommandation | justification |
|---|---|---:|---:|---|---|---|
| 40 | `status` | 5 | 2 | project_status | `keep` | truth runtime — corriger REQ-AXO-91497 + ajouter `tool_migration_status` |
| 41 | `health` | 3 | 1 | status | `merge_into:status` | sous-objet de `status mode=health` |
| 42 | `help` | 5 | 2 | — | `keep` | routing primaire |
| 43 | `job_status` | 4 | 1 | — | `keep` | async polling indispensable |
| 44 | `mcp_surface_diagnostics` | 2 | 1 | status | `merge_into:status` | `status mode=mcp_contract` |
| 45 | `project_status` | 3 | 2 | status | `merge_into:status` | `status mode=project` |
| 46 | `project_registry_lookup` | 3 | 1 | — | `keep` | bootstrap |
| 47 | `axon_init_project` | 5 | 1 | — | `keep` | bootstrap kickoff bundle |
| 48 | `axon_commit_work` | 5 | 1 | — | `keep` | UN FIX = UN COMMIT enforcer |
| 49 | `axon_pre_flight_check` | 5 | 1 | — | `keep` | gate qualité |
| 50 | `snapshot_history` | 3 | 3 | — | `keep` | support tier A (diff, anomalies) |
| 51 | `diagnose_indexing` | 4 | 2 | health | `keep` | distinct des autres `*_status` (machine-stable id recovery) |
| 52 | `embedding_status` | 3 | 2 | status | `merge_into:status` | `status mode=embedding` |
| 53 | `debug` | 2 | 1 | — | `niche_keep` | dev-only ; rarement appelé par LLM |

### 1.4 Tier D — meta / escape (6 tools)

| # | tool | intrinsic | tri_modal | redondance | recommandation | justification |
|---|---|---:|---:|---|---|---|
| 54 | `sql` | 5 | 1 | — | `keep` | escape hatch + onboarding fallback ; corriger REQ-AXO-91494 |
| 55 | `batch` | 4 | 1 | — | `keep` | composition primitives ; doit accepter ratio cache-TTL multi-call |
| 56 | `fs_read` | 3 | 1 | — | `keep` | escape pour fichiers non-IST (.toml, README, manifests) |
| 57 | `schema_overview` | 4 | 1 | sql | `keep` | prereq légitime à `sql` |
| 58 | `list_labels_tables` | 3 | 1 | schema_overview | `merge_into:schema_overview` | `mode=labels` |
| 59 | `query_examples` | 2 | 1 | help | `merge_into:help` | exemples = `help(tool=sql, examples=true)` |

### 1.5 Bilan agrégé

| Recommandation | Tools | Net après application |
|---|---:|---:|
| `keep` (incl. niche) | 28 | 28 |
| `must_migrate` (subset de keep) | 12 | — (re-codés, pas supprimés) |
| `merge_into` | 14 | -14 |
| `deprecate` | 0 | 0 |
| `niche_keep` | 5 | 5 (déjà comptés) |
| **TOTAL** | **59** | **45** |

Réduction nette : **59 → 45 tools** après consolidation (cf. Partie 2). Avant ajout des nouveaux tools (Partie 3).

---

## Partie 2 — Clusters de redondance et fusions

Chaque proposition de fusion est gouvernée par un argument concret. Sortie attendue : -14 tools.

### 2.1 Cluster retrieval (4 → 2)

| Existant | Sort |
|---|---|
| `retrieve_context` | **garde** — devient pivot RRF tri-modal |
| `retrieve_context_layered` | absorbé : `layered=true \| layers={vector,fts,graph,soll}` |
| `query` | **garde** — feuille (lookup symbole simple, pas de fusion) |
| `inspect` | absorbé dans `query` via `detail=full \| min` |

Argument : `retrieve_context_layered` = `retrieve_context` avec un paramètre `breakdown`. Aujourd'hui la duplication coûte 1 schéma, 1 entrée routing, 1 README dans `help`. `query` vs `inspect` : Cursor / Cody n'ont **qu'un** `findSymbol` ; la séparation Axon force le LLM à choisir entre 2 tools quasi-identiques, c'est un anti-pattern de surface MCP (cf. CPT-AXO-018).

### 2.2 Cluster impact (4 → 1)

| Existant | Sort |
|---|---|
| `impact` | **garde** — paramétré `mode={blast,api_break,safety_verdict}` |
| `api_break_check` | absorbé : `impact mode=api_break` |
| `change_safety` | absorbé : `impact mode=safety_verdict` |
| `simulate_mutation` | **garde** — sémantique distincte (delta-reachability vs blast) |

Argument : ces 3 tools partagent la même requête de fond (`reachability from symbol S in graph G`) avec des filtres et scorers différents. Un seul tool avec switch de mode supprime 2 entrées de routing et clarifie pour le LLM. `simulate_mutation` est conservé parce qu'il manipule un **graphe modifié** (suppression / renommage simulés), opération distincte.

### 2.3 Cluster path/trace (2 → 1)

| Existant | Sort |
|---|---|
| `path` | **garde** — bidi BFS RAM, paramètre `direction={forward,backward,bidi}` |
| `bidi_trace` | absorbé : `path direction=bidi` |

Argument : bidi BFS est un algorithme du tool `path`, pas un tool séparé. Cody le fait : un seul `xrefs`.

### 2.4 Cluster anomalies/drift (2 → 1)

| Existant | Sort |
|---|---|
| `anomalies` | **garde** — étendu : cycles + bridges + SCC + dead-cluster |
| `architectural_drift` | absorbé : `anomalies mode=drift between_snapshots=[a,b]` |

Argument : drift = diff de findings entre 2 snapshots. Pas un algorithme nouveau, juste un argument temporel.

### 2.5 Cluster snapshot_diff/diff (2 → 1)

| Existant | Sort |
|---|---|
| `diff` | **garde** — paramètre `scope={symbol,snapshot,subgraph}` |
| `snapshot_diff` | absorbé : `diff scope=snapshot` |

Argument : nom de tool différent pour la même opération paramétrée. Bruit de surface.

### 2.6 Cluster rationale (3 → 1)

| Existant | Sort |
|---|---|
| `why` | **garde** — pivot rationale, absorbe les 2 autres |
| `conception_view` | absorbé : `why depth=conception` |
| `truth_check` | conservé sous le nouveau nom `claim_verify` (Partie 3) — sémantique distincte (vérification d'assertion vs explication) |

Argument : `conception_view` est un mode "remonter au pillar" de `why`. Pas un tool. `truth_check` reste, mais le re-naming le sort de l'ambiguïté avec `why`.

### 2.7 Cluster SOLL CRUD (3 → 2)

| Existant | Sort |
|---|---|
| `soll_manager` | **garde** — unifié create/update/link |
| `soll_validate` | **garde** — absorbe verify |
| `soll_verify_requirements` | absorbé : `soll_validate mode=verify_requirements` |
| `axon_apply_guidelines` | absorbé : `axon_apply_methodology_bundle bundle_kind=guidelines` |
| `axon_apply_methodology_bundle` | **garde** |

### 2.8 Cluster runtime/status (5 → 1)

| Existant | Sort |
|---|---|
| `status` | **garde** — mode unifié : `mode={brief,health,project,mcp_contract,embedding,migration}` |
| `health` | absorbé : `status mode=health` |
| `project_status` | absorbé : `status mode=project` |
| `mcp_surface_diagnostics` | absorbé : `status mode=mcp_contract` |
| `embedding_status` | absorbé : `status mode=embedding` |

Argument : 5 tools "status flavor" forcent le LLM à savoir lequel choisir. Un seul `status mode=X` avec mode défaut `brief`. C'est exactement le pattern qu'utilisent Sourcegraph (`src status`) et Cursor (composite status panel).

### 2.9 Cluster meta (2 → 1)

| Existant | Sort |
|---|---|
| `schema_overview` | **garde** — `mode={tables,labels,examples}` |
| `list_labels_tables` | absorbé : `schema_overview mode=labels` |
| `query_examples` | absorbé : `help(tool=sql, mode=examples)` |

### 2.10 Synthèse fusions

| Avant | Après | Net |
|---|---|---:|
| 59 | 45 | -14 |

Distribution post-fusion : Tier A=14 · Tier B=17 · Tier C=10 · Tier D=4.

---

## Partie 3 — Nouveaux tools à ajouter (le levier différenciant)

Critère absolu : le tool **n'est crédible que si tri-modal (graphe RAM + vector HNSW + FTS) le rend possible à < 50 ms p99**. CTE PG actuelles le rendraient soit impossible (PageRank, VF2) soit prohibitif (radius illimité). Pour chaque tool :

- catégorie CPT-AXO-90007 ∈ {single-lookup, structural, rationale, wiring, impact-context, semantic-only, anomaly-detection, clone-detection, autre}
- surfaces ∈ {graph, vector, fts, soll, git}
- intent (1 phrase)
- exemple Q LLM
- valeur ajoutée vs marché (Cursor / Cody / Continue / Aider / Copilot / Augment)

### 3.1 Catalogue (17 tools proposés)

| # | tool | catégorie | surfaces | intent (1 phrase) | exemple Q LLM | valeur ajoutée vs marché |
|---|---|---|---|---|---|---|
| N1 | `centrality_rank` | structural | graph | Top-K symboles par PageRank / betweenness / in-degree / fanout dans un scope. | "Quels 20 symboles dois-je lire en premier pour comprendre `pipeline_v2` ?" | Aucun outil n'expose la centralité ; Cursor/Continue rankent par similarité textuelle ; Cody fait xref mais pas PageRank. |
| N2 | `bridge_symbols` | structural+anomaly | graph | Énumère les articulation points / bridges du call graph dans un scope. | "Si je supprime cette fn, est-ce que je déconnecte des sous-systèmes ?" | Aucun outil du marché (Cursor/Cody/Aider) ne fait Tarjan bridges. C'est inexprimable en SQL et en SCIP. |
| N3 | `scc_enumerate` | structural+anomaly | graph | Énumère toutes les SCC complètes (pas seulement détection de cycle). | "Liste-moi tous les modules qui se rappellent en boucle et leurs membres." | `anomalies` actuelle détecte cycles à 2 hops (REQ-AXO-91493) ; Cody et Aider ne font ni SCC ni cycles transitifs. |
| N4 | `pattern_match_graph` | structural+clone | graph+vector | Cherche les occurrences d'un sous-graphe (VF2) avec filtre embedding-similarity sur les noms. | "Trouve les autres `factory→build→validate` dans le code." | Cursor "find similar" = vector seul ; Cody = textuel ; aucun ne fait subgraph match. C'est la valeur unique d'avoir VF2 + embeddings. |
| N5 | `bug_pattern_search` | clone+anomaly | graph+vector+fts | À partir d'un fix git ou d'un symbole "buggy", retrouve les sites présentant la même topologie + sémantique. | "Trouve les autres usages avec ce même pattern de race condition." | Approche unique : VF2 sur structure + vector sur descriptifs + FTS sur signature. Cody/Aider/Cursor ne corrèlent pas. |
| N6 | `dead_or_weak_cluster` | structural+anomaly | graph | Composantes orphelines (aucun edge entrant depuis le main), sub-graphes faiblement connectés. | "Quelles parties du code peuvent-elles être supprimées ?" | Cursor/Cody ne calculent pas la réachabilité depuis un entry-set. |
| N7 | `coupling_score` | structural | graph | Score de couplage module↔module (afferent/efferent + instability de Martin + min-cut). | "Module A et B sont-ils trop couplés ?" | Sourcegraph affiche dépendances ; aucun n'agrège metric instability + min-cut quantitatif. |
| N8 | `min_cut_decouple` | structural+impact | graph | Trouve les N edges minimaux à supprimer pour déconnecter A de B (max-flow / Karger). | "Pour découpler ce module, par où couper en priorité ?" | Inexprimable dans tous les outils du marché. |
| N9 | `reading_order` | structural+rationale | graph+soll | Donne un ordre topologique pondéré PageRank pour onboarder sur un scope ; couple à SOLL pour titrer les "stations". | "Donne-moi un parcours minimal pour comprendre ce module." | Aider/Continue génèrent un repo-map plat ; aucun ne propose un ordre de lecture optimisé. |
| N10 | `risk_zone_map` | anomaly+structural | graph+git+soll | Croise centralité + hot-spots git (churn, bug-fixes commits) + REQ-AXO tagged `axon-bug`. | "Quelle est la zone du code la plus risquée ?" | Cody Bug Insights existe mais n'agrège pas avec call-graph centrality. Combo unique. |
| N11 | `claim_verify` | rationale | graph+vector+fts+soll | Vérifie qu'une assertion ("X appelle Y") est vraie en confrontant graphe + extraits FTS/vector + SOLL. | "Est-ce que `axon-brain` appelle directement `pgvector` ?" | Re-naming + extension de `truth_check`. Aucun outil marché ne fait grounded fact-check structurel. |
| N12 | `cohort_retrieve` | semantic-only+impact | graph+vector | Retrieval guidé par anchor : candidats vector/FTS **filtrés** par reachability depuis une ancre du graphe. | "Donne-moi les 30 chunks sémantiquement proches **et** atteignables depuis `pipeline_v2::run`." | Le fix du bruit RAG actuel. Continue/Cursor renvoient des chunks vector-proches mais structurellement déconnectés. |
| N13 | `test_blast` | impact-context | graph+fts | À partir d'un fn modifié, retrouve les tests qui devraient changer ou devenir trompeurs (graphe vers tests + FTS sur noms/descriptions). | "Si je renomme cette fonction, quels tests vont devenir trompeurs ?" | Aider fait test-discovery basique ; aucun ne croise call-graph + FTS sur snapshots tests. |
| N14 | `api_surface_diff` | impact-context | graph+git | Différence d'API publique entre deux refs git + score de breaking-change par centralité du symbole touché. | "Quels callers externes vont casser entre v1 et v2 ?" | Existe partiellement chez Aider (`/diff`) mais pondération par centralité = unique. |
| N15 | `abstraction_under_use` | structural+anomaly | graph | Symboles publics / traits / interfaces avec in-degree très en-dessous de la moyenne du même `kind`. | "Quelles abstractions sont sous-utilisées ?" | Sourcegraph Insights → manuel ; aucun outil n'auto-détecte under-use. |
| N16 | `path_alternatives` | structural | graph | K-shortest paths (Yen) entre deux symboles, avec annotation par module/file. | "Donne-moi 3 chemins différents par lesquels A peut appeler B." | Inexprimable en CTE. Aider/Cody font 1 chemin. |
| N17 | `query_planner` | autre (meta-tool) | graph+vector+fts | Étant donné une question NL, propose la composition de tools à exécuter (chain-of-tool). | "Comment dois-je m'y prendre pour répondre à 'pourquoi cette fonction est lente' ?" | Pas un re-implémentation d'agent ; un dispatcher déterministe basé sur la question. Réduit les round-trips LLM. Aucun outil marché ne l'expose. |

### 3.2 Mapping (question → tool) — vérification de couverture

Reprend les exemples canoniques de la mission + 4 ajoutés :

| Question LLM | Tool primaire | Tool de support |
|---|---|---|
| Trouve les patterns de bug similaires à celui-ci. | N5 `bug_pattern_search` | N4 `pattern_match_graph` |
| Si je renomme cette fonction, quels tests deviennent trompeurs ? | N13 `test_blast` | `impact` migré |
| Quelle est la zone du code la plus à risque ? | N10 `risk_zone_map` | N1 `centrality_rank` |
| Quelles abstractions sont sous-utilisées ? | N15 `abstraction_under_use` | N1 `centrality_rank` |
| Parcours minimal pour comprendre ce module. | N9 `reading_order` | N1 `centrality_rank` |
| Si je supprime X, qui devient orphelin ? | N6 `dead_or_weak_cluster` | `simulate_mutation` migré |
| Comment cette requête HTTP arrive-t-elle à pgvector ? | `path` (migré) | N16 `path_alternatives` |
| Donne-moi 30 chunks vector-proches **et** atteignables. | N12 `cohort_retrieve` | `retrieve_context` migré |
| Y a-t-il un cycle module↔module caché ? | N3 `scc_enumerate` | N7 `coupling_score` |
| Quelles edges couper pour découpler A et B ? | N8 `min_cut_decouple` | N7 `coupling_score` |
| Est-ce que `brain` appelle vraiment `pgvector` directement ? | N11 `claim_verify` | `retrieve_context` migré |
| Donne-moi 3 chemins alternatifs A→B. | N16 `path_alternatives` | `path` migré |
| Quel diff d'API publique entre `main` et `feat/x` ? | N14 `api_surface_diff` | `impact` migré |
| Quels symboles sont architecturalement centraux ? | N1 `centrality_rank` | N2 `bridge_symbols` |
| Comment répondre à "pourquoi c'est lent" ? | N17 `query_planner` | — |

### 3.3 Coûts de calcul attendus (cible 1 M nodes / 1.5 M edges)

Référence : §5 du concept IST-RAM. Latences estimées en RAM (single-thread, CSR + reverse).

| Tool | Algo cœur | Latence cible p99 | Pré-warm requis |
|---|---|---:|---|
| N1 `centrality_rank` | PageRank 30 itér. | < 200 ms (full) / < 20 ms (scoped) | optionnel cache |
| N2 `bridge_symbols` | Tarjan bridges DFS | < 80 ms | non |
| N3 `scc_enumerate` | Tarjan SCC | < 100 ms | non |
| N4 `pattern_match_graph` | VF2 sur sous-graphe ≤ 10 nodes | < 50 ms par occurrence | non |
| N5 `bug_pattern_search` | VF2 + vector cosine sur N candidats | < 150 ms | non |
| N6 `dead_or_weak_cluster` | reverse BFS depuis entry-set + SCC | < 80 ms | entry-set défini par config |
| N7 `coupling_score` | comptage edges + min-cut approx | < 30 ms par paire | non |
| N8 `min_cut_decouple` | Karger / Edmonds-Karp | < 500 ms (graphe complet), < 50 ms (sous-scope) | non |
| N9 `reading_order` | PageRank restreint + topo sort | < 80 ms | dépend de N1 cache |
| N10 `risk_zone_map` | join graph centrality + git churn + SOLL tags | < 100 ms (snapshot mensuel) | snapshot pré-calculé |
| N11 `claim_verify` | path + RRF tri-modal | < 60 ms | non |
| N12 `cohort_retrieve` | reverse-reachability ∩ pgvector top-K | < 80 ms | non |
| N13 `test_blast` | k-hop forward sur tests + FTS | < 80 ms | annotation `is_test` requise |
| N14 `api_surface_diff` | diff graph (snapshot A vs B) + centrality | < 200 ms | snapshots des deux refs |
| N15 `abstraction_under_use` | in-degree stats par `kind` | < 50 ms | non |
| N16 `path_alternatives` | Yen K-shortest (K ≤ 5) | < 100 ms | non |
| N17 `query_planner` | classifier (rule-based + embedding sur question) | < 30 ms | dispatcher table requise |

### 3.4 Surfaces déclarées par tool — vérification tri-modal réelle

Critère : un tool **doit** utiliser ≥ 2 surfaces parmi {graph, vector, fts} pour justifier "tri-modal" ; sinon il est "graph-only" et reste légitime mais ne paie pas le bénéfice RRF.

| Tool | graph | vector | fts | soll | git | tri-modal effectif |
|---|:---:|:---:|:---:|:---:|:---:|:---|
| N1 | ✓ | | | | | non (graph-only, OK) |
| N2 | ✓ | | | | | non |
| N3 | ✓ | | | | | non |
| N4 | ✓ | ✓ | | | | **oui (2/3)** |
| N5 | ✓ | ✓ | ✓ | | ✓ | **oui (3/3 + git)** |
| N6 | ✓ | | | | | non |
| N7 | ✓ | | | | | non |
| N8 | ✓ | | | | | non |
| N9 | ✓ | | | ✓ | | non (graph+soll) |
| N10 | ✓ | | | ✓ | ✓ | **oui (graph+soll+git)** |
| N11 | ✓ | ✓ | ✓ | ✓ | | **oui (3/3 + soll)** |
| N12 | ✓ | ✓ | | | | **oui (2/3)** |
| N13 | ✓ | | ✓ | | | **oui (2/3)** |
| N14 | ✓ | | | | ✓ | non (graph+git) |
| N15 | ✓ | | | | | non |
| N16 | ✓ | | | | | non |
| N17 | ✓ | ✓ | ✓ | | | **oui (3/3 routing) — meta** |

Résultat : **8/17 tools tri-modaux explicites** ; les 9 autres sont graph-RAM-only (qui est lui-même nouveau, donc valeur supérieure). Le rapport tri-modal/graph-only est sain : on évite l'over-claim "tout est RRF".

### 3.5 Vérification — différenciation vs marché

| Capacité | Cursor | Cody | Continue | Aider | Copilot | Augment | Axon (post-MIL-019) |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Vector retrieval | ✓ | ✓ | ✓ | partiel | ✓ | ✓ | ✓ |
| FTS | partiel | ✓ | ✓ | ✓ | partiel | ✓ | ✓ |
| Call graph (SCIP/AST) | partiel | ✓ | partiel | ✓ | partiel | ✓ | ✓ |
| **Authentique RRF tri-modal** | ✗ | ✗ | ✗ | ✗ | ✗ | partiel | **✓** |
| **PageRank / centrality** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓** N1 |
| **Bridges / articulation** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓** N2 |
| **SCC complet** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓** N3 |
| **Subgraph match (VF2)** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓** N4/N5 |
| **Min-cut decouple** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓** N8 |
| **Reachability-filtered retrieval** | ✗ | partiel | ✗ | ✗ | ✗ | partiel | **✓** N12 |
| **Risk map (centrality+git+SOLL)** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓** N10 |
| **Reading order** | ✗ | ✗ | ✗ | repo-map plat | ✗ | partiel | **✓** N9 |
| **K-shortest paths** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓** N16 |
| **Persistent design rationale (SOLL)** | ✗ | ✗ | ✗ | ✗ | ✗ | partiel | **✓** |

Résumé : **9 capacités structurelles totalement absentes du marché**, **3 partielles transformées en majeures**, plus l'unicité SOLL. Le delta produit est concentré sur le graphe analytique en RAM — qui n'existe nulle part ailleurs au moment de la rédaction.

### 3.6 Conventions de nommage appliquées

Pattern Axon : `verbe_nom` ou `nom_qualifier` (snake_case, ≤ 3 tokens).

| Conformité | Tools |
|---|---|
| `verbe_nom` | `claim_verify`, `cohort_retrieve`, `pattern_match_graph`, `scc_enumerate`, `query_planner` |
| `nom_qualifier` | `centrality_rank`, `bridge_symbols`, `reading_order`, `risk_zone_map`, `coupling_score`, `min_cut_decouple`, `bug_pattern_search`, `test_blast`, `api_surface_diff`, `abstraction_under_use`, `path_alternatives`, `dead_or_weak_cluster` |

---

## Partie 4 — Synthèse opératoire

### 4.1 Diff produit MIL-AXO-019

| | Avant | Après merges | Après ajouts |
|---|---:|---:|---:|
| Tools totaux | 59 | 45 | **62** |
| Tier A (analyse) | 19 | 14 | **31** (+17) |
| Tier B (SOLL) | 20 | 17 | 17 |
| Tier C (runtime) | 14 | 10 | 10 |
| Tier D (meta) | 6 | 4 | 4 |

### 4.2 Vagues d'introduction recommandées (ordonné par ROI / dépendances)

| Vague | Tools nouveaux | Prérequis |
|---|---|---|
| V1 | N1 `centrality_rank`, N2 `bridge_symbols`, N3 `scc_enumerate`, N6 `dead_or_weak_cluster`, N15 `abstraction_under_use`, N16 `path_alternatives` | graphe RAM slice 1 |
| V2 | N4 `pattern_match_graph`, N12 `cohort_retrieve`, N13 `test_blast` | V1 + vector index pré-existant |
| V3 | N5 `bug_pattern_search`, N10 `risk_zone_map`, N14 `api_surface_diff` | V2 + git annotation pipeline |
| V4 | N7 `coupling_score`, N8 `min_cut_decouple`, N9 `reading_order` | V1 |
| V5 | N11 `claim_verify` (rename truth_check), N17 `query_planner` | V1..V4 |

### 4.3 Risques produit

| Risque | Sévérité | Mitigation |
|---|---|---|
| Couverture call-graph Rust = 0 (constat §8.3 du concept) | **élevée** | Slice 0 obligatoire : auditer + réparer extracteur Rust ; sinon N1..N17 vides sur AXO lui-même |
| Surface 62 tools dépasse charge cognitive LLM | moyenne | N17 `query_planner` + groupement `help mode=routing` ; help affiche 6 catégories au plus |
| Doublon entre `retrieve_context` migré et N12 `cohort_retrieve` | moyenne | N12 = filtre anchor obligatoire ; `retrieve_context` = question NL ; documenter la frontière |
| Sub-agent forbidden étend à tools structurels | faible | maintenir CPT-AXO-018 / GUI-PRO-027, déjà inscrit dans MEMORY.md et CLAUDE.md |
| Cache PageRank stale durant burst d'indexation | faible | LSM overlay (concept §4.2) ; status expose `centrality_lag_ms` |

### 4.4 Critères acceptation pour la phase nouveaux tools

| Critère | Seuil |
|---|---:|
| Tools graph-only latence p99 | < 200 ms |
| Tools tri-modaux latence p99 | < 250 ms |
| Couverture Rust call-graph avant V1 | ≥ 90 % |
| Documentation auto (`help(tool=X)`) | 100 % des 17 |
| Tests intégration par tool | ≥ 3 (positif / négatif / scope-vide) |
| Conformité GUI-PRO-100 sur descriptions tool | 100 % |
| Tool `query_planner` accuracy classification | ≥ 80 % sur jeu test 50 questions |

---

## Annexe — Tools NON proposés et pourquoi

Pour démontrer la discipline (mission : viser 10-20, pas plus). Tools écartés :

| Tool écarté | Raison du rejet |
|---|---|
| `complexity_score` (McCabe, etc.) | déjà couvert par linters externes ; pas tri-modal |
| `comment_density` | métrique cosmétique, sans valeur LLM |
| `git_blame_aggregator` | doublons GitHub native ; sans plus-value structurelle |
| `import_graph_only` | sous-cas de `path` / `impact` |
| `style_drift` | scope linter, hors Axon |
| `license_audit` | hors-périmètre dev-LLM |
| `readme_extractor` | `fs_read` suffit |
| `embedding_drift_alert` | déjà couvert par `embedding_status` |
| `tool_usage_stats` | sous-cas `status mode=mcp_contract` |
| `naming_consistency` | nice-to-have, pas tri-modal différenciant |

10 tools écartés explicitement pour rester sous la cible 20.
