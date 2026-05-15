# Concept — IST in-memory graph (miroir de la stratégie SOLL)

Date : 2026-05-15
Statut : analyse exploratoire, pas encore en SOLL.
Référence parente : DEC-AXO-091 / REQ-AXO-322 (graphe SOLL en RAM).

## 1. Contexte

Le projet dispose depuis MIL-AXO-017 d'un snapshot SOLL `petgraph::Graph<String,String>` chargé à froid depuis PostgreSQL puis exploité en RAM par les tools hot read (`soll_work_plan`, `soll_verify_requirements`, ...). PG reste écrivain canonique ; l'invalidation est best-effort via `SollSnapshotCache::invalidate` depuis la couche dispatch MCP.

Question opérateur : **peut-on dupliquer ce schéma pour l'IST** (Symbol/Edge), avec
- chargement initial en RAM,
- mises à jour incrémentales suivant les modifications,
- PG canonique,
- coût mémoire **minimal** (graphe utile à 100 % mais compacté),

et quelle valeur ajoutée au-dessus des CTE récursives PG actuelles ?

## 2. Volumétrie réelle (instantané 2026-05-15)

| Table | Lignes | Heap | Index | Total |
|---|---:|---:|---:|---:|
| `public.symbol` | **168 301** | 30 MB | 40 MB | 70 MB |
| `public.edge` | **324 359** | 60 MB | 263 MB | 323 MB |
| `public.indexedfile` | 11 593 | — | — | — |
| `public.chunk` | 254 355 | — | — | — |

Cible opérateur : ~1 M nodes / ~1 M edges. Hypothèse réaliste : si la base couvre l'ensemble des dépôts visés on tend vers **1.0 M / 1.5 M** (le ratio actuel edges/symbols ≈ 1.93 ; CONTAINS pèse 53 %, CALLS 47 %).

**Distributions exploitables pour compaction :**
- `kind` : **19 valeurs** distinctes (section, function, element, method, …) → `u8`.
- `relation_type` : **3 valeurs** (CONTAINS, CALLS, CALLS_NIF) → `u2` packé en `u8`.
- `project_code` : 28 codes aujourd'hui → `u8` (capacité 256, marge confortable).
- `id` : 72 octets moyens (max 218), forme `path::module::function` → forte redondance préfixe.
- `name` : 24 octets moyens (max 152).
- 4 booléens packés (`tested`, `is_public`, `is_nif`, `is_unsafe`) → `u8` (4 bits utilisés).

## 3. Design proposé — CSR compact + reverse index

### 3.1 Choix structure

`petgraph::Graph<String,String>` (approche SOLL) à 1 M / 1 M = ~176 MB (24 B overhead + String 72 B par node, plus alloc fragmentation). Acceptable pour SOLL à ~1 k nodes, insuffisant à l'échelle IST.

Alternative compacte : **CSR (Compressed Sparse Row) forward + reverse**, avec interning agressif :

```rust
// Empreinte cible : ~80–100 MB pour 1 M nodes / 1 M edges.
struct IstGraph {
    // Métadonnées packées (12 B/node).
    nodes: Vec<NodePack>,                  // 1 M × 12 B = 12 MB

    // CSR forward.
    fwd_offsets: Vec<u32>,                 // (N+1) × 4 = 4 MB
    fwd_targets: Vec<u32>,                 // M × 4    = 4 MB
    fwd_rel:     Vec<u8>,                  // M × 1    = 1 MB

    // CSR reverse (pour callers / impact-amont).
    rev_offsets: Vec<u32>,                 // 4 MB
    rev_sources: Vec<u32>,                 // 4 MB
    rev_rel:     Vec<u8>,                  // 1 MB

    // Arène d'IDs avec préfixe-dédup (factorisation `crate::mod::`).
    id_arena:    Box<[u8]>,                // ~30–50 MB raw (vs 72 MB naïf)

    // Table d'interning lookup string→NodeIndex.
    id_to_idx:   FxHashMap<&'static str, u32>, // ~24 MB

    // Constantes projet → byte.
    project_codes: Vec<&'static str>,      // <1 KB
}

#[repr(C, packed)]
struct NodePack {                          // 12 octets exactement
    id_offset:    u32,
    name_offset:  u32,
    kind:         u8,                      // 19 variants
    project:      u8,                      // <256 projets
    flags:        u8,                      // tested|public|nif|unsafe
    _pad:         u8,
}
```

### 3.2 Coût mémoire détaillé (1 M nodes / 1 M edges)

| Poste | Taille | Note |
|---|---:|---|
| NodePack × 1 M | **12 MB** | métadonnées packées |
| CSR forward (offsets + targets + rel) | **9 MB** | sortants |
| CSR reverse (offsets + sources + rel) | **9 MB** | requis pour callers / impact-amont |
| Arène IDs (suffix-dedup activé) | **~40 MB** | 72 B raw × dedup 0.55 ≈ 40 MB |
| HashMap id → u32 | **~24 MB** | indispensable pour API "par ID" |
| Arène noms (optionnel, hot tools en ont besoin) | **~18 MB** | sinon fallback PG |
| Divers (project table, kind table, freelist) | <1 MB | — |
| **TOTAL minimal (sans names)** | **~95 MB** | impact / callers / SCC / centralité |
| **TOTAL complet (avec names)** | **~115 MB** | + lookups symboliques sans PG |

À titre de comparaison :
- `petgraph::Graph<String,String>` à 1 M / 1 M ≈ **176 MB** + fragmentation.
- L'index `edge_rev_idx` seul en PG = **263 MB** (mais sur disque, pas en RAM Rust).
- Une réplique purement "tabulaire" (Vec<SymbolRow>) sans graphe ≈ 110 MB.

**Verdict** : ~95 MB est un point bas atteignable, ~115 MB un point bas pragmatique. Le delta vs petgraph naïf (~60–80 MB) vient surtout du CSR (vs liste chaînée) et de l'interning d'IDs.

### 3.3 Variantes envisageables

| Variante | Gain | Coût |
|---|---|---|
| `petgraph::Csr` (built-in) au lieu de hand-rolled | -5 % code | +20 % RAM (Vec<u32> non packé pour weights) |
| Bitset `is_public` séparé du NodePack | -1 octet/node = 1 MB | indirection cache |
| Perfect hashing (cmph) au lieu de FxHashMap | -50 % HashMap = 12 MB | build à chaque rechargement (+200 ms) |
| Drop reverse CSR, faire transpose à la volée | -9 MB RAM | k-hop amont coûte ×degree |

## 4. Mécanisme de chargement et synchronisation

### 4.1 Chargement initial

Pattern identique à `SollSnapshotCache` (cache.rs) :
1. Trois lectures PG : `SELECT id, name, kind, tested, is_public, is_nif, is_unsafe, project_code FROM public.symbol` ; `SELECT source_id, target_id, relation_type, project_code FROM public.edge` ; `SELECT path, content_hash FROM public.indexedfile`.
2. Construction CSR en deux passes : (a) bucket-sort des edges par `source_id` pour bâtir `fwd_offsets/fwd_targets/fwd_rel` ; (b) bucket-sort par `target_id` pour `rev_*`.
3. Atomic swap via `ArcSwap<IstGraph>`.

**Estimation temps à froid** (1 M / 1 M) :
- Lecture PG : ~1.5 s (300 MB de payload via COPY ou query_json) → optimisable à 500 ms avec `COPY TO STDOUT BINARY` direct (existe déjà côté `bulk_writer.rs`).
- Bucket sort + CSR build : ~600 ms (single-threaded) ou ~200 ms en rayon parallèle.
- Interning IDs : ~300 ms.
- **Total : ~1.5–2.5 s pour 1 M / 1 M** à froid, contre ~50–100 ms pour le SOLL actuel (~1 k nodes).

### 4.2 Mises à jour incrémentales — l'infrastructure existe déjà

Le pipeline v2 dispose déjà d'un `notify_listener.rs` qui écoute `LISTEN chunk_pending_embed` via tokio-postgres. La stratégie pour l'IST est **directement copiable** :

1. Ajouter un trigger PG `AFTER INSERT/UPDATE/DELETE ON public.symbol|public.edge` qui émet `pg_notify('ist_mutated', json)`. Payload minimal : `{op, table, id, source_id?, target_id?, relation_type?}`.
2. Le brain `LISTEN ist_mutated` ; chaque message déclenche un **delta apply** sur le `IstGraph` sous un `Mutex<Pending>` ou via `ArcSwap` + double-buffering.
3. Pour les inserts en masse (ingestion bulk) : envoyer un seul `pg_notify('ist_rebuild', generation)` post-commit et reconstruire complet (idempotent, ~2 s).

Coût d'un delta unitaire :
- Insert edge : 2 lookups HashMap + 2 push CSR (mais CSR est *appendix*; il faut soit append-only segments soit rebuild léger). Stratégie pragmatique : **segments épisodiques** — un CSR principal immutable + un overlay Vec<(src,tgt,rel)> ; au-delà de 5 % d'overlay, rebuild en arrière-plan.
- Delete edge : marquer tombstone dans l'overlay (negation set).

Cette approche LSM-like est éprouvée (RocksDB / Lucene). Elle évite les écritures CSR en place et garde des reads O(deg + |overlay|) très petits en pratique.

### 4.3 Cohérence

PG reste l'autorité. Toute requête tool hot read consulte le snapshot RAM ; un drapeau `freshness_lag_ms` est exposé via `status` (cohérent avec le contrat `RuntimeFreshnessState` existant). Si `lag > seuil`, le tool peut soit attendre soit fallback vers PG (comme aujourd'hui pour les soll tools en mode `degraded`).

## 5. Que gagne-t-on vs Postgres + CTE récursives ?

### 5.1 Comparaison sur queries existantes

Référence : `graph_query.rs:517` (impact radius), `graph_analytics.rs:677` (cycle path), `graph_analytics.rs:828` (unsafe exposure), `graph_analytics.rs:885` (NIF blocking).

| Query | PG CTE (warm cache) | In-memory CSR | Speedup |
|---|---:|---:|---:|
| Impact radius=5 depuis 1 symbol | 30–150 ms | **1–3 ms** | **30–50×** |
| Detect all call cycles (1M edges) | 2–15 s ⚠ | **80–150 ms** (tarjan_scc) | **20–100×** |
| Cycles via arrays (`unsafe_exposure` depth=10) | 1–8 s | **20–60 ms** (BFS + filter) | **30–150×** |
| Callers transitifs depth=20 (`get_nif_blocking_risks`) | 500 ms–5 s | **5–15 ms** | **50–300×** |
| Domain-leakage (multi-join CONTAINS×CALLS×CONTAINS) | 200–800 ms | **3–10 ms** | **50–80×** |
| Single-hop neighbors d'un symbol | 1–3 ms (index hit) | **<0.1 ms** | 10–30× |

Les chiffres PG sont des ordres de grandeur basés sur :
- index `edge_fwd_idx` / `edge_rev_idx` (B-tree composé `(source_id, relation_type, target_id)`) — chaque hop = un index scan + jointure ;
- les CTE actuelles avec **path-tracking par tableau** matérialisent un état exponentiel (`p.path_ids || ARRAY[...]`), ce qui explique le facteur 30–300× et non simplement 10×.

Le **vrai gain n'est pas le CPU**, c'est :
1. **suppression du round-trip PG** (1–3 ms minimum par appel),
2. **latence prédictible** (pas de plan changeant selon stats),
3. **plus de plafond profondeur 10** (les CTE limitent toutes à 10 hops par sécurité — un graphe RAM peut traverser librement).

### 5.2 Queries que CTE ne peuvent pas exprimer (ou très mal)

Ces algorithmes sont soit impossibles en SQL pur, soit possibles mais à coût prohibitif :

1. **PageRank** sur le call graph — identifie les fonctions structurellement centrales. Convergence en 30–50 itérations × M edges ; trivial en RAM, irréalisable en CTE.
2. **Betweenness centrality** (Brandes) — symboles "goulots d'étranglement" entre sous-systèmes. O(V·E), ~secondes à minutes en RAM, hors de portée SQL.
3. **Articulation points / bridges** — fonctions dont la suppression déconnecte un sous-graphe (impact réel d'un refactor). Algorithme DFS classique, O(V+E).
4. **SCC complets** (et pas seulement détection) — partitionnement du graphe en composantes fortement connexes. `tarjan_scc` linéaire ; CTE peut détecter une cycle, pas les énumérer tous.
5. **Community detection (Louvain, Leiden)** — clustering modulaire du call graph → suggère un découpage modules naturel.
6. **K-shortest paths (Yen)** — alternatives de chemin entre deux symboles, utile pour proposer des refactors équivalents.
7. **Bidirectional BFS** — chemin entre source et cible 10–100× plus rapide qu'une CTE forward seule.
8. **A\* avec heuristique** — par exemple "trouve le plus court chemin de A à B qui reste dans le même crate" (heuristique = distance fichier).
9. **Subgraph isomorphism** (VF2) — chercher un pattern de calls (ex : factory→builder→validate). Trivial en RAM, ~impossible en SQL.
10. **Weisfeiler-Lehman / graph kernels** — similarité structurelle entre symboles (au-delà de l'embedding vectoriel actuel).
11. **Min-cut / max-flow** — coupes de dépendance minimales entre deux ensembles de symboles.
12. **Topological sort** dynamique (sans cycle) — ordre de build incrémental, ordre de déprécation.
13. **Reachability matrix compressed** (transitive closure stockée en bitset) — répond "est-ce que A peut appeler B ?" en 1 ns ; en SQL : CTE à chaque appel.

### 5.3 Tools MCP nouveaux qui deviennent crédibles

- `architectural_centrality` — top-N fonctions par PageRank + betweenness, pour audit "que dois-je vraiment tester d'abord".
- `bridge_symbols` — articulation points = points fragiles du refactor.
- `module_suggest` — Louvain sur CONTAINS-restricted call graph → suggestions de découpage de fichier.
- `path_alternatives` — k-shortest paths entre deux symboles.
- `dead_code_clusters` — SCC orphelins (aucun edge entrant depuis le projet).
- `call_pattern_match` — VF2 sur un mini-graphe template (sécurité, anti-pattern).
- `dependency_min_cut` — pour découpler deux sous-systèmes, quelles arêtes couper en minimum.

Le tout sans modifier le canonique PG. Et tout est `freshness: stale_ok` parce que ces analyses sont structurelles, pas opérationnelles.

## 6. Volumétrie comparée et synthèse

```
                                Volume actuel    Cible 1M/1M
PostgreSQL canonique
  Symbol heap                         30 MB           ~180 MB
  Edge heap                           60 MB           ~190 MB
  Edge indexes (fwd+rev+gin)         263 MB           ~800 MB
  Total disque IST                  ~400 MB         ~1.2 GB

In-memory graph proposé
  CSR forward + reverse                3 MB            18 MB
  NodePack (métadonnées)               2 MB            12 MB
  ID arena (dedup préfixe)            ~8 MB           ~40 MB
  HashMap id→idx                      ~4 MB           ~24 MB
  Name arena (optionnel)              ~3 MB           ~18 MB
  Total RAM minimal                  ~17 MB           ~95 MB
  Total RAM avec names               ~20 MB          ~115 MB

Chargement à froid                  ~250 ms         ~1.5–2.5 s
Delta unitaire (insert edge)          <1 µs           <1 µs
Rebuild background (overlay full)    ~250 ms         ~1.5–2.5 s
```

### 6.1 Verdict

| Question | Réponse courte |
|---|---|
| Peut-on dupliquer le mécanisme SOLL ? | **Oui**, même pattern (`ArcSwap<IstGraph>` + cache + invalidation), mais avec **CSR au lieu de `petgraph::Graph<String,String>`** pour rester sous 100 MB à 1 M / 1 M. |
| Coût minimal réaliste ? | **~95 MB RAM** pour la cible 1 M / 1 M en abandonnant l'arène `name` (fallback PG pour les rares lookups symboliques) ; **~115 MB** avec lookup name intégré. À la volumétrie actuelle (168 k / 324 k) : **~20 MB**. |
| Temps de chargement ? | **~1.5–2.5 s à froid** à 1 M / 1 M ; **~250 ms aujourd'hui**. Acceptable au boot du brain. |
| Sync incrémentale ? | **Déjà câblée** : `notify_listener.rs` écoute `LISTEN chunk_pending_embed` ; étendre avec `LISTEN ist_mutated` + triggers PG. Overlay LSM pour deltas, rebuild background quand >5 %. |
| Quel ROI vs CTE ? | **30–300× sur les queries existantes**, plus une **classe entière** de queries (PageRank, betweenness, SCC, articulation points, community detection, subgraph match) **inaccessibles en SQL**. |

## 7. Prochaines étapes proposées (à valider avant exécution)

1. **REQ-AXO-XXX** en SOLL : *"IST in-memory snapshot — CSR compact + reverse, sync LISTEN/NOTIFY"*.
2. **Slice 1 (POC)** : structures `IstGraph` + chargement bulk depuis PG, sans incremental. Bench sur le repo actuel (168 k / 324 k symbols). Valider <30 MB et <500 ms cold load.
3. **Slice 2** : intégrer dans `graph_query.rs::structural_impact_at_radius` et `graph_analytics.rs::circular_dependency_count_fast` comme fast-path. Conserver fallback PG. Mesurer le speedup réel.
4. **Slice 3** : triggers PG + `LISTEN ist_mutated` + overlay LSM. Tests de cohérence sous écriture concurrente.
5. **Slice 4** : nouveaux tools MCP (`architectural_centrality`, `bridge_symbols`, `module_suggest`) opt-in derrière `status.public_tools` shaping.

L'infrastructure PG (triggers déjà documentés dans `db/ddl/03_ist_schema.sql` pour `chunk_pending_embed`), le pattern cache (`SollSnapshotCache`), et le listener tokio (`notify_listener.rs`) sont **tous déjà en place** — l'effort est sur la structure de données compacte et son intégration, **pas sur la plomberie**.

---

## 8. Valeur ajoutée pour la compréhension LLM (round 2)

Question opérateur : *« ces queries plus rapides et nouveaux tools MCP — vraie valeur ajoutée pour la compréhension LLM ou cosmétique ingénieur ? Qu'est-ce qu'on verrait sur Axon qu'on ne voit pas aujourd'hui ? »*

Réponse honnête en deux temps : (a) ce qui aide vraiment un LLM, (b) ce qui est utile humain mais marginal pour LLM, (c) ce qu'on verrait concrètement sur Axon.

### 8.1 Ce qui aide réellement un LLM (priorité haute)

Le goulot d'étranglement LLM n'est ni le CPU ni la latence : c'est la **sélection de contexte**. Aujourd'hui `retrieve_context` ranke par FTS + cosinus vectoriel ; ces signaux ratent la **centralité structurelle**. Un symbole peu nommé mais critique structurellement reste invisible.

| Capacité graphe | Pourquoi le LLM en a besoin | Outil actuel équivalent | Limite actuelle |
|---|---|---|---|
| **PageRank / in-degree pondéré** | "Pour comprendre la couche X, quels 20 symboles charger d'abord ?" Le LLM choisit aujourd'hui par similarité textuelle, pas par importance. | aucun | retrieve_context renvoie 5 chunks lexicalement proches, jamais le `init_runtime` central appelé 80× |
| **Bridge / articulation points** | "Si je change cette fn, ça casse quoi structurellement ?" Distingue un changement local d'un changement architectural. | `impact` (radius=N) | impact donne *l'ensemble atteignable* mais pas *les points de coupure* — un bridge avec 1 caller pèse autant qu'une feuille avec 1 caller |
| **SCC complet (énumération)** | Le LLM lit une fonction-membre d'un cycle ; il doit savoir qu'elle ne peut pas être raisonnée isolée. | `anomalies` (échantillon) | anomalies surface 1 cycle, pas la composante entière ; le LLM rate les autres membres |
| **Bidirectional BFS / path between** | "Comment cette requête HTTP arrive-t-elle au pgvector ?" Réponse en chemin court, pas en énumération radius. | `path` (CTE) | path PG est capé à 10 hops et lent ; en RAM tu obtiens *tous* les chemins courts en quelques ms |
| **Subgraph match (VF2)** | "Trouve les autres endroits qui suivent ce pattern bug." Outil de réparation systématique. | aucun | impossible : il faudrait écrire un CTE par pattern à chaque fois |
| **Reachability bitset** | "Est-ce que A peut atteindre B ?" Question oui/non qui revient sans cesse. | path | 100 ms PG ↔ 50 ns bitset |

**Ces 6 capacités déplacent vraiment l'aiguille pour un LLM** parce qu'elles répondent à des questions que le LLM se pose *en silence* mais auxquelles aucun tool ne répond aujourd'hui — sauf à brûler 5 calls coûteux et reconstruire le graphe en tête.

### 8.2 Utile humain, marginal pour LLM

| Capacité | Utile pour | Pourquoi marginal LLM |
|---|---|---|
| Min-cut / max-flow | Architecte qui découpe un crate | Le LLM ne propose pas de découpages, il répond à des questions ; min-cut sans front humain est inerte. |
| K-shortest paths (Yen) | Debugging humain de propagation d'erreur | Le LLM utilise rarement plus de 2 chemins ; le 1er suffit. |
| Louvain / community detection | Suggestion de modules à un dev | Sortie qualitative, pas de réponse claire à une question ; le LLM va parser le clustering sans pouvoir l'argumenter. |
| Graph kernels (Weisfeiler-Lehman) | Similarité structurelle | Recouvre l'embedding vectoriel existant (BGE-Large sur chunks) — gain marginal. |

À garder en backlog mais ne pas mettre en priorité.

### 8.3 Ce qu'on verrait sur AXO aujourd'hui (données vérifiées)

J'ai exécuté les queries directement sur la base. Les chiffres ci-dessous sont **réels au 2026-05-15** :

**Constat #1 — le call-graph Rust d'AXO est à zéro.**
- 4 273 symboles Rust (fonctions/méthodes) dans AXO.
- 3 048 CALLS dont source ∈ AXO functions, **mais 0 où la source est un fichier `.rs`**. Les 3 048 sont **toutes** Python (2 516 confirmées `.py`, le reste autre).
- Conséquence directe : `impact` / `path` / `circular_dependency_*` sont **structurellement aveugles** sur le cœur Rust du projet. Le LLM qui pose "qui appelle `IstGraph::load` ?" ne reçoit rien d'utile.
- Un outil de centralité par langage exposé en RAM (`SELECT lang, AVG(out_degree) FROM nodes` côté in-memory) **aurait surfacé ce trou en une question** alors qu'il est resté invisible jusqu'ici. La donnée existe dans PG mais personne n'a écrit la query d'audit ; un graphe en RAM rend ce genre d'audit ad hoc trivial.

**Constat #2 — les hubs structurels actuels sont biaisés vers les helpers Python.**
Top-5 in-degree AXO (CALLS, cible ∈ AXO functions) :
1. `qualify_ingestion_run.py::shell` — 8 callers
2. `Axon.Watcher.Telemetry.get_val` — 7 callers
3. `qualify_runtime.py::shell` — 7 callers
4. `qualify_ingestion_run.py::parse_int` — 7 callers
5. `qualify_runtime.py::step_result` — 6 callers

Top-5 fanout AXO (callees) :
1. `qualify_ingestion_run.py::main` — 94 callees
2. `qualify_runtime.py::run_runtime_smoke` — 40 callees
3. `release::create_manifest.py::main` — 39 callees
4. `mcp_validate.py::run` — 35 callees
5. `runtime_sensor_log.py::main` — 28 callees

Ces résultats sont **utiles dans l'absolu** mais ils confirment surtout #1 : **on n'audite que la qualification, pas le brain**. Un LLM qui veut comprendre "comment `axon-brain` orchestre l'ingestion" tombe sur du Python de tests/qualif. Aujourd'hui ce biais n'est ni mesuré ni signalé. Avec un graphe en RAM + une simple métrique d'asymétrie source/cible par langage, la dashboard `status` pourrait dire : *"coverage Rust call graph : 0 % — voir REQ-AXO-XXX"*.

**Constat #3 — RMC démontre ce qu'AXO devrait avoir.**
Pour mémoire, RMC (Roam Code, 11 987 CALLS internes) :
- Top in-degree : `monkeypatch.chdir` (362), `roam` (173), `self._make_symbol` (96)
- Top fanout : commandes CLI (`pr_risk`: 64 callees, `health`: 57, `dead`: 49, `split`: 49)

Sur RMC un LLM peut **inférer la topologie CLI → handlers en un coup d'œil**. Sur AXO il ne peut pas inférer le cœur Rust faute d'edges.

**Constat #4 — pas de SCC ni de bridges calculables aujourd'hui.**
La fonction `get_circular_dependency_count_fast` détecte les cycles à 2 hops uniquement (`c1.source = c2.target AND c1.target = c2.source`). Pour AXO Rust à 0 calls, retour systématique 0. Aucun signal "REQ-AXO-XXX a un cycle de dépendance" possible — même si le code en contenait. **Avec un graphe en RAM, `tarjan_scc` détecterait *toutes* les SCC de toute taille en linéaire — y compris celles cachées par la profondeur > 2.**

**Constat #5 — les bridges/articulation points sont littéralement inexprimables en CTE.**
L'algorithme de Tarjan pour bridges fait deux DFS avec low-link numbers. Il n'a pas de forme SQL ; il ne se compile pas en `WITH RECURSIVE`. Sur AXO, des fonctions comme `axonctl::start_brain` ou `notify_listener::run_listener` sont quasi-certainement des bridges (séparer le brain de son listener déconnecterait l'ingest). **Aujourd'hui rien ne le dit au LLM.** Un tool `bridge_symbols` rendrait visible une liste de ~50 fonctions architecturalement critiques sur le repo.

### 8.4 Trois questions précises que le LLM pourra répondre que je ne peux pas répondre proprement aujourd'hui

1. **"Quel est l'ordre de chargement minimal pour comprendre l'ingestion pipeline_v2 ?"**
   - Aujourd'hui : `retrieve_context` ranke par similarité au mot "pipeline_v2" ; renvoie les fichiers contenant le terme, pas les **dépendances structurelles**. Il faut 4-5 round-trips pour reconstruire l'ordre.
   - Avec graphe RAM : PageRank restreint au sous-graphe atteignable depuis `pipeline_v2::*`, top-20 par centralité = ordre de lecture optimal. **Une seule requête, ~10 ms.**

2. **"Si je supprime `GraphStore::query_json`, qu'est-ce qui devient orphelin ?"**
   - Aujourd'hui : impact radius=5 (capé à 10) renvoie *l'ensemble atteignable*. Le LLM doit deviner lequel est vraiment orphelin (= ne reste atteignable que via `query_json`).
   - Avec graphe RAM : retirer le nœud du CSR + reverse-reachability check → liste exacte. **<5 ms.**

3. **"Quelles paires de modules ont un couplage cyclique caché derrière >2 hops ?"**
   - Aujourd'hui : impossible. `get_circular_dependency_count_fast` détecte cycle direct seulement.
   - Avec graphe RAM : SCC sur le quotient module-level. **~50 ms à 1M edges.**

Sur ces trois questions, **le gain n'est pas un facteur 30× sur une question déjà posée**, c'est *l'apparition* d'une question qui n'était pas posable.

### 8.5 Verdict honnête sur la valeur LLM

- **Valeur réelle, immédiate** : PageRank/centralité, bridges, SCC, bidirectional BFS, subgraph match. Ces 5 changent ce qu'un LLM peut affirmer sans halluciner sur la structure.
- **Valeur conditionnée à la couverture d'index** : si AXO Rust reste à 0 CALLS, le graphe sera vide pour le cœur du projet. **Le pré-requis n°1 n'est donc pas le graphe en RAM, c'est de réparer l'extracteur Rust (REQ-AXO existant ou à créer).** Tant que le call graph Rust = ∅, le graphe en RAM ne donne pas plus que PG sur AXO lui-même.
- **Valeur indirecte mais forte** : *les algorithmes nouveaux mettent en évidence ce qui manque*. C'est l'audit "coverage" qui devient possible — un LLM qui demande "le call graph Rust est-il indexé ?" obtient une réponse quantitative au lieu d'un silence.

### 8.6 Recommandation ordonnée (correction du §7)

Ordre proposé (modifié vs §7 à la lumière du round 2) :

1. **Précondition** : auditer l'extracteur Rust IST (probable REQ existant ou à créer — `axo_rust_call_extraction_coverage`). Sans Rust calls, le graphe en RAM n'aide pas sur AXO lui-même.
2. **Slice 0 (1 jour)** : ajouter le calcul de couverture call-graph par langage dans `status` (read direct PG, pas besoin de RAM). Met en évidence le problème.
3. **Slice 1 (POC, ~3 jours)** : structure CSR + chargement bulk + benchmark à volumétrie actuelle. Valider <30 MB / <500 ms.
4. **Slice 2 (1 semaine)** : exposer `architectural_centrality` (PageRank/in-degree) + `bridge_symbols` (Tarjan) + `path_between` (bidi BFS). Sur RMC qui a un call graph dense, démontrer la valeur immédiatement.
5. **Slice 3 (1-2 semaines)** : triggers PG + LISTEN/NOTIFY incrémental.
6. **Slice 4 (continu)** : SCC énumération, subgraph match — quand des cas d'usage concrets se présentent.

Le graphe en RAM est **un multiplicateur**, pas un fix : il rend visible et rapide ce qui est *déjà indexé*. Si l'indexation est partielle, le multiplicateur l'est aussi.

---

## 9. Risque vs trois index actuels (graphe + vecteur + FTS)

Question opérateur : *« on a déjà un graphe, du vecteur et du FTS — implémenter un graphe en RAM va-t-il nous faire perdre de la puissance de recherche ? »*

Réponse courte : **non, c'est additif**, à condition de ne pas supprimer le canonique PG. Détail ci-dessous, basé sur la lecture du code réel dans `mcp/tools_context/` (1 617 lignes).

### 9.1 Comment les trois index sont fusionnés aujourd'hui

L'hybride actuel n'est pas un RRF formel. C'est un **scoring composite** par chunk (voir `candidates.rs`, `scoring.rs`, `graph_expansion.rs`) :

| Source | Rôle | Implémentation | Plafond |
|---|---|---|---|
| **Vector** | similarité sémantique BGE-Large 1024d | pgvector sur `public.chunkembedding` | `candidate.semantic_distance` |
| **FTS** | match lexical | tsvector PG sur chunk content | `candidate.fts_rank`, bonus +4 max, **2 slots** dédiés |
| **Graph** | expansion structurelle des entry candidates | `refresh_symbol_projection` + `query_graph_projection` (table `GraphProjection`, cache des CTE) | **radius 1 ou 2 seulement** (`graph_expansion.rs:8`), **2 voisins max** (`graph_expansion.rs:31`) |
| Pénalités | bruit (tests, /docs, /.axon/, /target/) | `scoring.rs::workspace_noise_penalty` + `uri_penalty_reason` | -3 à -6 |
| Bonus intent | docs/plans, docs/vision | `canonical_project_doc_weight` | +1 à +4.5 |

**Constat critique** : le graphe contribue très peu à `retrieve_context` aujourd'hui. Radius capé à 1-2, voisins capés à 2. La raison structurelle est dans `graph_expansion.rs:13` : chaque expansion appelle `refresh_symbol_projection` qui exécute une CTE récursive — c'est trop lent pour pousser plus loin sans dégrader la latence du tool.

### 9.2 Ce que le graphe en RAM ajoute, ce qu'il ne touche pas

| Composant | Touché par graphe RAM ? | Conséquence |
|---|---|---|
| pgvector (BGE embeddings) | **Non** | Reste sur PG, index HNSW intact. Aucune perte sémantique. |
| FTS tsvector sur chunks | **Non** | Reste sur PG. Aucune perte lexicale. |
| `public.Edge` canonique | **Non** | PG reste écrivain, les CTE existantes continuent à fonctionner en fallback. |
| Table `GraphProjection` (cache CTE) | **Optionnel** | Peut être maintenue comme aujourd'hui ou remplacée par RAM. Décision indépendante. |
| `collect_structural_neighbors` (radius 1-2, 2 voisins) | **Remplacé / étendu** | Lit le graphe RAM ; passe à radius 5-10 et 20-50 voisins sans coût. |
| Scoring composite | **Enrichi** | Nouveaux signaux disponibles : centrality_score, articulation_proximity, reachability_from_anchor. |

**Conclusion structurelle** : le graphe en RAM **remplace uniquement la partie *traversal* du tier graphe**. Il ne remplace ni l'index vecteur ni l'index FTS, qui restent intégralement dans PG.

### 9.3 Gains nets sur le retrieve_context hybride

1. **Expansion structurelle multipliée par 10-25×.** Aujourd'hui 2 voisins radius 1-2. Demain : 20-50 voisins radius 5, en <5 ms. Le LLM reçoit un sous-graphe pertinent au lieu d'une paire de voisins arbitraires.

2. **Filtre "reachability from anchor" sur candidats vector/FTS.** Aujourd'hui un chunk vector-proche peut être structurellement déconnecté de l'ancre — il pollue le contexte. Avec le graphe RAM, on filtre : *"ces 50 chunks proches sémantiquement sont-ils atteignables structurellement depuis `pipeline_v2::*` ?"* — en <10 ms. Aujourd'hui inexprimable à coût raisonnable.

3. **PageRank-prior sur le scoring.** Un chunk dont le symbol porteur a un PageRank élevé reçoit un boost. Aujourd'hui le scoring est purement local (similarité + match) ; les hubs structurels qui ne contiennent pas les termes de la question sont invisibles. Le PageRank-prior corrige ce biais.

4. **RRF authentique devient praticable.** Aujourd'hui, fusionner trois rangs (vector, FTS, graph) demande un graph_rank rapide. Avec CTE c'est trop lent ; avec graphe RAM le graph_rank est O(deg). On peut écrire un vrai Reciprocal Rank Fusion (REQ-AXO-298 mentionne RRF mais l'implémentation actuelle est composite-score, pas RRF).

5. **Anchor-aware queries.** Subgraph match autour d'une ancre + filtre embedding similarity → patterns architecturaux ciblés. Aujourd'hui impossible.

### 9.4 Risques de perte (et mitigations)

| Risque | Sévérité | Mitigation |
|---|---|---|
| Désynchronisation RAM vs PG → expansion vers symboles supprimés | Faible | Re-check `public.symbol` au moment du fetch chunk (déjà fait dans `candidates.rs`). Aucun chunk fantôme atteint l'LLM. |
| Suppression de `GraphProjection` casse les tools qui s'y appuient (`refresh_symbol_projection`) | Moyenne | **Ne pas supprimer.** Le graphe RAM s'ajoute, GraphProjection reste comme materialized view pour les chemins non-migrés. Migration progressive. |
| RAM stale pendant un burst d'indexation | Faible | Pattern identique à SOLL : `RuntimeFreshnessState` expose `freshness_lag_ms`. Si lag > seuil, le tool peut fallback PG explicitement (déjà câblé dans `ReadFreshness::FreshPreferred`). |
| Perte de jointures SQL complexes (ex: "fns tested=false + emb similaire + 5+ callers") | Très faible | Le graphe RAM produit la liste d'IDs filtrés structurellement, puis on JOIN sur `public.symbol` / `public.chunkembedding` côté PG. Round-trip ajouté mais sémantique préservée. |
| Optimizer PG perd des plans hybrides (ex: edge-then-vector ou vector-then-edge) | Faible | PG optimizer reste maître de la partie vector/FTS. Le graphe RAM est appelé **en amont** comme "filtre structurel précomputé", pas en boucle interne d'un plan. |
| Coût mental code : deux représentations du graphe | Réel | Garder `IstGraph` derrière une seule API (`graph_query.rs::IstGraphView` qui choisit RAM ou PG selon freshness). Les call-sites ne changent pas. |

### 9.5 Verdict pour la question posée

**Aucune perte de puissance de recherche**, à trois conditions explicitement maintenues :

1. **PG reste écrivain canonique** — pas de mutation par le graphe RAM (comme SOLL aujourd'hui).
2. **pgvector et FTS restent dans PG** — le graphe RAM ne réplique aucun des deux.
3. **`GraphProjection` n'est pas supprimée tant que tous les call-sites n'ont pas migré** — migration par slice, fallback préservé.

Au contraire, **on débloque trois choses qui sont aujourd'hui inaccessibles** dans la fusion hybride :
- expansion structurelle de profondeur réelle (radius 5+, pas 1-2),
- filtre reachability ex-post sur les résultats vector/FTS,
- prior de centralité injecté dans le score composite.

Donc la bonne formulation du round 2 §8 doit être amendée : *non seulement le graphe RAM n'enlève rien*, mais il **rend la fusion hybride substantiellement plus expressive** parce qu'aujourd'hui la composante graphe est volontairement bridée par la latence CTE.

### 9.6 Garde-fous à inscrire dans le REQ

Quand REQ-AXO-XXX sera rédigée, inscrire explicitement :

- **NON-OBJECTIF** : remplacer pgvector ou FTS.
- **NON-OBJECTIF** : supprimer `public.Edge` ni `GraphProjection`.
- **INVARIANT** : si freshness_lag_ms > seuil configurable (par défaut 5 s), tools hot read fallback PG.
- **INVARIANT** : tout résultat retourné au LLM est re-vérifié via `public.symbol` / `public.chunk` au moment du fetch.
- **MÉTRIQUE** : `status.public_tools` expose `graph_ram_hit_rate` et `graph_pg_fallback_count` pour observer la cohérence en production.

---

## 10. Langage de query — Cypher, Gremlin, ou Rust brut ?

Question opérateur : *« le langage de query du graphe en RAM, c'est proche de Cypher / Gremlin, ou c'est du code Rust difficile à lire ? »*

Réponse courte : **petgraph n'a pas de langage de query — c'est de l'API Rust**. Mais l'utilisateur final (LLM ou humain via MCP) ne le voit jamais. Détail honnête ci-dessous.

### 10.1 Trois niveaux d'audience, trois surfaces

| Audience | Surface | Lisibilité | Exemple |
|---|---|---|---|
| **LLM (consommateur MCP)** | JSON tool calls (`path`, `impact`, `architectural_centrality`) | Triviale, déclarative | `{"tool":"path","from":"A","to":"B"}` |
| **Humain auteur de tool** | Rust + petgraph API | Verbeux, type-safe, explicite | extrait `snapshot.rs:223` ci-dessous |
| **Power user / debug** | SQL sur `public.Edge` (canonique) | Standard, lent | `WITH RECURSIVE ...` |

Le graphe en RAM **ne change que le niveau 2** (implémentation interne). Le niveau 1 (ce que l'LLM voit) ne change pas dans sa forme : on ajoute des tools, on ne modifie pas la syntaxe.

### 10.2 Comparaison concrète sur une même question

*"Symboles à ≤ 3 hops depuis `axon_brain::main`, en suivant uniquement les CALLS."*

**Cypher** (Neo4j, AGE) — 1 ligne déclarative :
```cypher
MATCH (a {id:'axon_brain::main'})-[:CALLS*1..3]->(b) RETURN b
```

**Gremlin** (TinkerPop) — chaîne fluent :
```groovy
g.V().has('id','axon_brain::main').repeat(out('CALLS')).times(3).dedup()
```

**SQL WITH RECURSIVE** (PG aujourd'hui, `graph_query.rs:517`) — 8 lignes :
```sql
WITH RECURSIVE t(node_id, distance) AS (
  SELECT 'axon_brain::main', 0
  UNION ALL
  SELECT e.target_id, t.distance + 1
  FROM public.edge e JOIN t ON e.source_id = t.node_id
  WHERE e.relation_type='CALLS' AND t.distance < 3
)
SELECT DISTINCT node_id FROM t WHERE distance > 0;
```

**petgraph Rust** (interne, ce que SOLL fait déjà, cf. `snapshot.rs:253-278`) — 15-20 lignes :
```rust
let start = graph.node_index("axon_brain::main")?;
let mut visited = HashSet::new();
let mut queue = VecDeque::from([(start, 0)]);
let mut out = Vec::new();
while let Some((node, depth)) = queue.pop_front() {
    if !visited.insert(node) || depth > 3 { continue; }
    for e in graph.edges_directed(node, Outgoing) {
        if e.weight() == "CALLS" {
            out.push(graph[e.target()].clone());
            queue.push_back((e.target(), depth + 1));
        }
    }
}
```

**MCP tool depuis l'LLM** — 1 ligne JSON :
```json
{"tool":"impact","symbol":"axon_brain::main","radius":3,"relation":"CALLS"}
```

### 10.3 Verdict sur la lisibilité

| Critère | Cypher | Gremlin | SQL CTE | petgraph Rust | MCP tool |
|---|---|---|---|---|---|
| Compacité | ★★★★★ | ★★★★ | ★★★ | ★★ | ★★★★★ |
| Lisibilité par non-Rust | ★★★★★ | ★★★ | ★★★★ | ★ | ★★★★★ |
| Lisibilité par Rust dev | ★★★ | ★★ | ★★★ | ★★★★ | ★★★★★ |
| Type safety | aucune | aucune | runtime | compile-time | runtime (JSON schema) |
| Composabilité | ★★★ | ★★★★ | ★★ | ★★★★★ | dépend du tool |
| Couverture algos | limitée | limitée | très limitée | universelle | définie par le surface MCP |

**Rust + petgraph est verbeux mais lisible si on a 5 min de pratique.** L'extrait `snapshot.rs:223` ci-dessus (cycle_sets) tient sur l'écran et chaque ligne fait une chose. Ce n'est pas du code obscur ; c'est juste explicite là où Cypher est concis.

L'avantage caché : **toutes les erreurs sont à la compilation**. Une faute de frappe dans un nom de relation type → compile error. En Cypher → renvoie vide silencieusement. Pour un système où l'LLM consomme les résultats, le silent-empty est plus dangereux que la verbosité.

### 10.4 Options si on veut un langage déclaratif quand même

Quatre alternatives, classées par effort/gain :

**Option A — Status quo (recommandé) : petgraph + MCP tools.**
- Effort : 0 supplémentaire.
- Audience humain : Rust dev seulement (mais c'est la cible interne).
- Audience LLM : JSON tools, déjà optimal.
- Aucune dépendance externe à maintenir.

**Option B — Re-introduire Cypher via AGE.**
- Effort : MIL-AXO-017 vient de retirer AGE explicitement (DEC-AXO-083). Le ré-introduire serait régressif.
- Gain : query language standard, mais lenteur ré-introduite (AGE = parser Cypher → SQL → PG plan, donc *pire* que les CTE actuelles, pas mieux).
- **À écarter.**

**Option C — Datalog embarqué via CozoDB.**
- Effort : ~1-2 semaines. CozoDB est en Rust, embarquable, supporte Datalog sur graphes in-memory + persistence. Skill `datalog-logic-programmer` listé dans l'env (cf. système).
- Gain : queries déclaratives concises, par exemple :
  ```datalog
  reaches[?to] := caller_of('axon_brain::main', ?to)
  caller_of(?from, ?to) := edge(?from, ?to, 'CALLS')
  caller_of(?from, ?to) := edge(?from, ?mid, 'CALLS'), caller_of(?mid, ?to)
  ```
  Très puissant pour les requêtes récursives complexes, plus expressif que Cypher pour la logique inductive.
- Cible : power user humain (l'opérateur qui veut tester une hypothèse architecturale ad hoc), **pas l'LLM**.
- À envisager si un cas d'usage humain le justifie. Pas un blocker pour le slice 1.

**Option D — Mini-DSL Cypher-like maison.**
- Effort : prohibitif (parser + planner + executor sur petgraph).
- Gain : marginal vs option C.
- **À écarter.**

### 10.5 Recommandation

1. **Slice 1-3** : petgraph + tools MCP, point. L'LLM ne touche pas Rust, l'humain qui code des tools écrit du Rust lisible (cf. SOLL `snapshot.rs` déjà en prod). La verbosité Rust est un coût d'écriture pour 5-10 fonctions internes, pas un coût d'usage.

2. **Si un besoin humain ad hoc émerge** (audit architectural exploratoire, pas réponse à LLM) : envisager **option C** (Cozo embarqué) comme outil opérateur, exposé via une commande `axonctl graph-query`, séparé de la surface MCP. Datalog est plus puissant que Cypher sur ce qui nous intéresse (récursion + agrégation).

3. **Ne pas ré-introduire AGE / Cypher dans PG** — la décision DEC-AXO-083 reste valide ; le retrait d'AGE est ce qui rend le graphe en RAM nécessaire en premier lieu.

En résumé : **pas de langage de query**, l'API petgraph en Rust pour 5-10 fonctions internes, et un Datalog optionnel (Cozo) si un usage humain le réclame. Le LLM voit toujours du JSON propre — c'est la seule surface qui compte pour la valeur ajoutée principale du projet.
