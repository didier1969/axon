# Audit qualité de la surface MCP — état actuel (MAJ 2026-06-14, post REQ-AXO-901952 + 901977 + GUI-AXO-1027)

**Périmètre** : 71 entrées du catalogue canonique (`catalog.rs`) = 70 publiques + `resume_vectorization` (non-publique en `brain_only`).

**⚠️ Déploiement** : les correctifs de cette session (901952 reliquat + 901977 classe + alignement petgraph) sont **commités sur `main` local, validés en dev (brain `g82eaa8c1`), NON promus en live et NON poussés**. Les latences/taux d'erreur ci-dessous proviennent de la télémétrie live (fenêtre 720 h) = **ancien binaire** ; ex. `query` 3453 ms/33 % err reflète l'ancien code, pas le `query` tri-modal corrigé.

**Colonnes** :
- **Latence** : avg ms réel (`mcp_telemetry_report`), `m`=mesuré (ancien binaire live), `e`=estimé (jamais appelé dans la fenêtre). Avg sémantiques gonflés par cold-starts (max ~60 s).
- **Qualité/Token** : première passe heuristique (à raffiner une-à-une).
- **RAM / FTS / Vec** : ✅ canonique/optimal · ➕ serait mieux · ❗ requis(manquant) · — N/A.
- **PG-graphe** : traversée PostgreSQL résiduelle en prod — **∅** aucune · **métrique** count(\*) agrégé conservé (pas une traversée) · **—** N/A.

## État des invariants (acceptance REQ-AXO-901952)
- ✅ **0 traversée de graphe IST en PG** (`WITH RECURSIVE`/`ist.path`/`callers_of`) dans les commandes structurelles — vérifié par scan.
- ✅ **Graphe SOLL** (cycles/ancestry/work_plan) = snapshot RAM petgraph (`reaches_via_relations`, `cycle_sets`, `count_descendants_in`, `soll_work_plan`).
- ✅ **Toggle `AXON_IST_RAM_ENABLED` supprimé** (RAM inconditionnel) ; cold RAM = erreur bruyante (jamais 0 silencieux).
- ✅ **Classe `Symbol.embedding` morte fermée** : `query` + `semantic_clones` rankent via chunk-embeddings ; `retrieve_context` l'était déjà (901937).
- ✅ **Alignement figé** : `GUI-AXO-1027` (SOLL=petgraph toujours / IST=CSR-léger+petgraph-lourd).
- **PG-graphe résiduel** = uniquement des **count(\*) agrégés** (health/truth_check/debug/sql) — métriques canoniques, pas des traversées ; `truth_check` en a besoin pour réconcilier PG↔RAM.

| # | Outil | Cat | Qualité | Latence | Token | RAM | FTS | Vec | PG-graphe | Modalité | Note |
| 1 | `query` | DX | 82% | 22% (3453ms m) | 85% | ✅ | ✅ | ✅ | ∅ | RAM+FTS+Vec | tri-modal (901977 : bras Symbol.embedding mort retiré). Latence/err = ANCIEN binaire (fix non promu) |
| 2 | `inspect` | DX | 95% | 88% (97ms m) | 90% | ✅ | — | ➕ | ∅ | RAM | RAM-only (901952) : fallback PG edge_counts retiré, froid=erreur bruyante |
| 3 | `impact` | RISK | 96% | 95% (38ms m) | 90% | ✅ | — | — | ∅ | RAM | RAM-only snapshot (901952) ; concept bridge_name retiré |
| 4 | `path` | DX | 95% | 95% (36ms m) | 90% | ✅ | — | — | ∅ | RAM | RAM-only bfs_shortest_path (901952) ; ist.path PG retiré |
| 5 | `why` | DX+SOLL | 82% | 12% (24763ms m) | 75% | ✅ | ➕ | ✅ | ∅ | RAM+SOLL+Vec | LENT 24.7s (embedding requête CPU + traversal). Frontière latence sémantique |
| 6 | `anomalies` | GOV | 92% | 80% (158ms m) | 88% | ✅ | — | ➕ | ∅ | RAM | cycles via Tarjan SCC RAM (901952) ; bridge edges retirés |
| 7 | `bidi_trace` | DX | 92% | 95% (35ms m) | 88% | ✅ | — | — | ∅ | RAM | RAM-only (901952), froid=erreur bruyante |
| 8 | `change_safety` | SYS/DX/SOLL | 90% | 95% (23ms m) | 90% | ✅ | — | ➕ | ∅ | RAM+SOLL | RAM-only tested+traçabilité (901952 gap B) |
| 9 | `conception_view` | SYS/DX | 90% | 100% (2ms m) | 88% | ✅ | — | — | ∅ | RAM+SOLL | 2ms |
| 10 | `semantic_clones` | GOV | 86% | 55% (1500ms e) | 82% | ✅ | — | ✅ | ∅ | RAM+Vec | CORRIGÉ (901977) : était MORT (Symbol.embedding), now chunk-ANN + exclusion même-nom |
| 11 | `architectural_drift` | GOV | 85% | 80% (300ms e) | 82% | ✅ | — | ➕ | ∅ | RAM+SOLL | RAM + intent SOLL |
| 12 | `audit` | GOV | 88% | 40% (1886ms m) | 80% | ✅ | — | — | ∅ | RAM | 1.9s gouvernance lourde |
| 13 | `health` | GOV | 90% | 95% (50ms m) | 88% | ✅ | — | — | métrique | RAM | count(*) ist.Edge = agrégat métrique (gardé, pas une traversée) |
| 14 | `diff` | RISK | 90% | 88% (90ms e) | 88% | ✅ | — | — | ∅ | RAM | diff snapshot N-1/N |
| 15 | `snapshot_diff` | SYS | 90% | 100% (0ms m) | 88% | ✅ | — | — | ∅ | RAM |  |
| 16 | `snapshot_history` | SYS | 92% | 100% (0ms m) | 90% | — | — | — | — | PG-meta | métadonnées snapshots |
| 17 | `ist_snapshot_warm` | IST | 90% | 70% (400ms e) | 92% | ✅ | — | — | ∅ | RAM | préchauffe le CSR RAM |
| 18 | `ist_centrality_pagerank` | IST | 90% | 80% (250ms e) | 88% | ✅ | — | — | ∅ | RAM(petgraph) | CSR→petgraph (to_petgraph) pour algo lourd — conforme GUI-AXO-1027 |
| 19 | `ist_structural_sccs` | IST | 92% | 88% (150ms e) | 88% | ✅ | — | — | ∅ | RAM(petgraph) | SCC via petgraph sur CSR converti — conforme GUI-AXO-1027 |
| 20 | `ist_shortest_path` | IST | 95% | 95% (30ms e) | 90% | ✅ | — | — | ∅ | RAM | traversée légère CSR maison — conforme GUI-AXO-1027 |
| 21 | `api_break_check` | RISK | 90% | 88% (120ms e) | 88% | ✅ | — | — | ∅ | RAM | RAM-only consumer surface (901952) |
| 22 | `simulate_mutation` | RISK | 88% | 88% (150ms e) | 86% | ✅ | — | — | ∅ | RAM | RAM-only blast-radius (901952), dernier ist.callers_of retiré |
| 23 | `retrieve_context` | DX | 88% | 12% (10299ms m) | 68% | ✅ | ✅ | ✅ | ∅ | RAM+FTS+Vec | tri-modal ; CONTAINS/file_path RAM (901952 gap D). LENT 10.3s (embedding CPU) |
| 24 | `retrieve_context_layered` | DX | 88% | 80% (200ms e) | 68% | ✅ | ✅ | ✅ | ∅ | RAM+FTS+Vec | tri-modal par couches |
| 25 | `soll_query_context` | SOLL | 88% | 88% (111ms m) | 82% | — | ➕ | ➕ | — | SOLL-PG | FTS/Vec amélioreraient le rappel intent |
| 26 | `soll_manager` | SOLL | 86% | 80% (276ms m) | 85% | — | — | — | ∅ | SOLL-RAM(petgraph) | garde de cycle link via SOLL RAM petgraph (901952) ; WITH RECURSIVE retiré. 11% err = statuts legacy (atténué 901962) |
| 27 | `soll_apply_plan` | SOLL | 85% | 55% (1129ms m) | 80% | — | — | — | — | SOLL-PG | 1.1s application plan |
| 28 | `soll_commit_revision` | SOLL | 88% | 88% (150ms e) | 85% | — | — | — | — | SOLL-PG |  |
| 29 | `soll_rollback_revision` | SOLL | 88% | 80% (200ms e) | 85% | — | — | — | — | SOLL-PG |  |
| 30 | `soll_validate` | SOLL | 92% | 88% (75ms m) | 86% | — | — | — | — | SOLL-PG | invariants graphe intent |
| 31 | `soll_acyclic_audit` | SOLL | 90% | 88% (120ms e) | 86% | — | — | — | ∅ | SOLL-RAM(petgraph) | cycle_sets via SollSnapshot (tarjan_scc) — déjà RAM |
| 32 | `soll_verify_requirements` | SOLL | 90% | 88% (97ms m) | 84% | — | — | — | — | SOLL-PG |  |
| 33 | `soll_attach_evidence` | SOLL | 88% | 95% (28ms m) | 88% | — | — | — | — | SOLL-PG | 14% err (format artefacts) |
| 34 | `soll_remove_evidence` | SOLL | 88% | 95% (25ms e) | 88% | — | — | — | — | SOLL-PG |  |
| 35 | `soll_relation_schema` | SOLL | 95% | 100% (1ms m) | 92% | — | — | — | — | statique | 0.9ms |
| 36 | `soll_export` | SOLL | 88% | 95% (42ms m) | 60% | — | — | — | — | SOLL-PG | export volumineux |
| 37 | `soll_generate_docs` | SOLL | 85% | 55% (800ms e) | 62% | — | — | — | — | SOLL-PG |  |
| 38 | `restore_soll` | SOLL | 82% | 40% (2000ms e) | 70% | — | — | — | — | SOLL-PG | async |
| 39 | `document_intent` | DX/SOLL | 86% | 55% (658ms m) | 80% | ➕ | — | ✅ | — | SOLL-PG+Vec | lie intent↔code via Vec |
| 40 | `infer_soll_mutation` | SOLL | 84% | 95% (8ms m) | 82% | — | ➕ | ✅ | — | Vec+SOLL-PG | similarité sémantique |
| 41 | `entrench_nuance` | SOLL | 82% | 70% (500ms e) | 80% | — | — | ✅ | — | Vec+SOLL-PG |  |
| 42 | `re_anchor` | SOLL | 85% | 95% (14ms m) | 85% | — | — | ➕ | — | Vec+SOLL-PG |  |
| 43 | `project_status` | SYS/SOLL | 80% | 12% (17402ms m) | 78% | — | — | — | — | SOLL-PG+Runtime | LENT 17.4s — agrégation lourde, candidat optimisation |
| 44 | `project_registry_lookup` | SYS/SOLL | 90% | 80% (189ms m) | 88% | — | — | — | — | PG-registry |  |
| 45 | `axon_init_project` | DX/SOLL | 85% | 55% (610ms m) | 78% | — | — | — | — | SOLL-PG+FS | bootstrap projet |
| 46 | `axon_apply_guidelines` | DX/SOLL | 88% | 80% (300ms e) | 82% | — | — | — | — | SOLL-PG |  |
| 47 | `axon_apply_methodology_bundle` | DX/SOLL | 85% | 55% (1200ms e) | 78% | — | — | — | — | SOLL-PG+FS |  |
| 48 | `axon_commit_work` | DX/SOLL | 92% | 88% (58ms m) | 85% | — | — | ➕ | — | FS+SOLL-PG | valide diff vs guidelines |
| 49 | `axon_pre_flight_check` | DX/SOLL | 92% | 95% (25ms m) | 86% | — | — | ➕ | — | FS+SOLL-PG | dry-run pré-commit |
| 50 | `status` | SYS | 90% | 88% (138ms m) | 80% | — | — | — | — | Runtime | brief existe, payload riche |
| 51 | `job_status` | SYS | 92% | 95% (10ms e) | 90% | — | — | — | — | Runtime |  |
| 52 | `diagnose_indexing` | SYS | 88% | 80% (200ms e) | 82% | — | — | — | — | Runtime |  |
| 53 | `embedding_status` | SYS | 90% | 80% (161ms m) | 86% | — | — | — | — | Runtime |  |
| 54 | `truth_check` | SYS | 92% | 70% (339ms m) | 88% | — | — | — | métrique | Runtime+PG | count(*) ist.Edge VOLONTAIRE = réconcilie PG vs RAM (sa raison d être) |
| 55 | `batch` | SYS | 85% | 95% (50ms e) | 82% | — | — | — | — | Runtime |  |
| 56 | `debug` | SYS | 85% | 88% (80ms e) | 80% | — | — | — | métrique | Runtime | count diagnostique ist.Edge (agrégat) |
| 57 | `rescan_project` | SYS | 85% | 88% (100ms e) | 82% | — | — | — | — | Runtime |  |
| 58 | `resume_vectorization` | SYS | 85% | 95% (50ms e) | 82% | — | — | — | — | Runtime | non-public en brain_only |
| 59 | `mcp_surface_diagnostics` | SYS | 92% | 100% (1ms m) | 88% | — | — | — | — | statique |  |
| 60 | `mcp_telemetry_report` | SYS | 92% | 95% (40ms e) | 88% | — | — | — | — | PG-rollup |  |
| 61 | `mcp_friction_report` | SYS/SOLL | 92% | 95% (40ms e) | 88% | — | — | — | — | PG-rollup |  |
| 62 | `mcp_feedback` | SYS | 90% | 95% (9ms m) | 90% | — | — | — | — | PG |  |
| 63 | `sql` | LLM/ADV | 88% | 100% (5ms e) | 88% | — | — | — | métrique | PG-raw | SELECT read-only ; hint Cypher pointe vers outils RAM (901952). 8% err SQL |
| 64 | `schema_overview` | LLM/ADV | 95% | 95% (26ms m) | 88% | — | — | — | — | statique |  |
| 65 | `query_examples` | LLM/ADV | 95% | 100% (2ms e) | 90% | — | — | — | — | statique |  |
| 66 | `help` | LLM | 98% | 100% (1ms m) | 95% | — | — | — | — | statique | 0.7ms |
| 67 | `skill_list` | SOLL/SKI | 95% | 100% (2ms e) | 92% | — | — | — | — | statique |  |
| 68 | `skill_invoke` | SOLL/SKI | 90% | 95% (30ms e) | 88% | — | — | — | — | SOLL-PG |  |
| 69 | `prompt_template_get` | SOLL/PRT | 95% | 100% (2ms e) | 92% | — | — | — | — | statique |  |
| 70 | `fs_read` | DX | 95% | 100% (0ms m) | 88% | — | — | — | — | Filesystem | lecture fichier par URI |
| 71 | `soll_work_plan` | SOLL | 88% | 80% (213ms m) | 84% | — | — | — | ∅ | SOLL-RAM(petgraph) | EdgeFiltered+tarjan_scc sur SollSnapshot — conforme GUI-AXO-1027 |

## Reste (hors périmètre de cette session — pour l'évaluation une-à-une)
- **Latence sémantique** (`why` 24,7 s · `project_status` 17,4 s · `retrieve_context` 10,3 s · `query` ancien 3,5 s) : embedder live en CPU → coût d'embedding de requête. À traiter (option LLM activable, ou réduction latence) — décision opérateur en attente.
- **➕ enrichissement Vec/FTS** (non bloquant) : `inspect`, `anomalies`, `change_safety`, `architectural_drift`, `soll_query_context`, `document_intent`, `axon_commit_work`/`pre_flight_check`.
- **Option C** (`GUI-AXO-1027`) : traits `petgraph::visit::*` natifs sur `IstGraph` (~200 LOC, sans copie) — non décidée.
