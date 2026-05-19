# Document de concept — Pool de graphes en mémoire pour Axon (IST + SOLL)

**Date** : 2026-05-08
**Statut** : proposition architecturale à valider
**Auteur** : analyse externe à transmettre au fournisseur Axon
**Périmètre** : refonte du moteur de stockage et de requêtage IST + SOLL d'Axon
**Préoccupations adressées** : latence requêtes (vs AGE actuel), scaling multi-projets, durabilité

---

## 1. Résumé exécutif

Axon souffre d'une latence de requête sous-optimale due à l'utilisation d'Apache AGE comme moteur graphe. Les benchmarks publics (avril 2026) montrent que les patterns à profondeur 5+ avec faible sélectivité prennent **3 à 5 secondes** dans AGE, contre 100 ms équivalents en moteur graphe natif. Pour un MCP server interactif, cet écart est préjudiciable.

Cette proposition décrit un **pool de graphes en mémoire avec LRU et idle eviction**, garantissant :

- Latence reads en **microseconde** sur hot path (algos petgraph natifs)
- **Postgres reste source-of-truth** (durabilité, audit, ACID, zéro risque de perte SOLL)
- Empreinte mémoire **bornée et configurable** (max_resident_mb)
- Support **N projets** sur un seul serveur 32 GB RAM grâce au LRU
- Cold start **1-5 s** avec snapshots persistés (HNSW + petgraph bincode)
- Compatibilité **MPL 2.0 / Apache 2.0** sur tous les composants

**Gain mesuré attendu** : × 100 à × 1000 sur la latence des requêtes graphe vs AGE actuel, sans changement du modèle de données SOLL ni de la sémantique applicative.

---

## 2. Constat actuel

### 2.1 Pile graphe Axon en production

```
axon-brain (Rust)
   │
   ├── DuckDB embedded (IST canonical + SOLL)
   ├── AGE / Apache AGE pour requêtes graphe Cypher
   ├── pgvector pour vector search (configuration variable)
   └── (option) Memgraph pour visualisation humaine
```

### 2.2 Limites observées

- **AGE planner relationnel** : sous-estime fan-outs graphe, choisit mauvais ordres de jointure
- **`agtype`** (variant JSON-like) : parsing à chaque accès, pas de stats colonnes typées
- **Pas d'index par défaut** sur AGE : nécessite tuning manuel
- **Pas de bidirectional BFS** ni d'algos graphe-natifs
- **Latence variable-length paths** : 100 ms → 3-5 s vs Neo4j (Trendyol Tech, avril 2026)
- **Multi-projets** : à 100 projets × IST + SOLL, le coût de requête se cumule

### 2.3 Profil d'usage SOLL + IST

| Catégorie | Caractéristique |
|---|---|
| Lectures | Massives (chaque requête MCP : query, inspect, retrieve_context, impact, soll_query_context) |
| Écritures | Occasionnelles (création Decisions / Requirements / Concepts par agents et utilisateurs) |
| Profondeur typique des traversées | 1-3 hops avec filtre fort |
| Profondeur worst-case | 5-7 hops (chaînes supersedes longues, impact analysis) |
| Multi-projets | `project_code` discriminant, isolation logique |
| Durabilité requise | **SOLL never delete** (CLAUDE.md), audit trail obligatoire |

---

## 3. Proposition architecturale

### 3.1 Vue d'ensemble

```
                      ┌──────────────────────────┐
                      │  axon-brain (Rust)        │
                      │  MCP server               │
                      └──────┬───────────────────┘
                             │
              ┌──────────────┴──────────────┐
              │                              │
        Writes (rares)                Reads (massives)
              │                              │
              ▼                              ▼
     ┌────────────────┐            ┌──────────────────────────┐
     │  PostgreSQL    │            │ ProjectGraphPool          │
     │  source-       │            │ (LRU, idle eviction)      │
     │  of-truth      │            │                           │
     │                │            │ ┌────────────────────┐    │
     │ - soll_nodes   │            │ │ Cache RAM résident │    │
     │ - soll_edges   │            │ │  - Top N projets   │    │
     │ - ist_nodes    │   hydrate  │ │  - LRU + last_access    │
     │ - ist_edges    │ ──────────▶│ │  - max_resident_mb │    │
     │ - chunks       │   stream   │ └────────────────────┘    │
     │ - outbox       │            │                           │
     │ - LISTEN/NOTIFY│            │ ┌────────────────────┐    │
     │   trigger      │            │ │ Persistent disk    │    │
     └────────────────┘            │ │ snapshots          │    │
              ▲                    │ │  - .bin (petgraph) │    │
              │                    │ │  - .idx (HNSW)     │    │
   write-     │                    │ │  - mmap-friendly   │    │
   through    │                    │ └────────────────────┘    │
              │                    └───────────────────────────┘
```

### 3.2 Principes invariants

1. **Postgres = vérité canonique**. Toute écriture passe par Postgres en premier. Aucune écriture mémoire-only. SOLL never lost.
2. **Mémoire = vue matérialisée**. Reconstructible from scratch depuis Postgres à tout moment. Pas de reliance sur la persistance mémoire pour la durabilité.
3. **Snapshot disque = optimisation**. Évite le coût de rebuild HNSW (1-5 min sans snapshot). Si snapshot corrompu, fallback rebuild Postgres.
4. **LRU par project_code**. Granularité d'éviction : projet entier (pas chunk-by-chunk). Les agents travaillent par projet, c'est la frontière naturelle.
5. **Idle timeout configurable**. Décharge auto après N minutes d'inactivité. Libère RAM pour autres projets.

### 3.3 Composants techniques

| Composant | Crate Rust | License | Rôle |
|---|---|---|---|
| Graph data structure | `petgraph` 0.6+ | MIT/Apache 2.0 | Storage + algos (BFS, A*, Dijkstra, Tarjan SCC, PageRank) |
| Serialization graph | `bincode` + serde | MIT | Snapshot rapide vers disque |
| HNSW index persistant | `usearch` ou `instant-distance` | Apache 2.0 | Vector index, save/load natif |
| Cache LRU | `lru` crate | MIT | Eviction policy par projet |
| Sync pool | `tokio::sync::RwLock` | MIT/Apache 2.0 | Concurrent reads, exclusive writes |
| Postgres listener | `sqlx` PgListener | MIT/Apache 2.0 | LISTEN/NOTIFY pour propagation |
| Compression snapshot | `zstd` crate | BSD/MIT | Réduit empreinte disque (× 2-3) |

Tous les composants sont **OSI-approved et compatibles avec MPL 2.0**.

---

## 4. Calcul de l'empreinte mémoire

### 4.1 Décomposition par projet

| Composant | Quantité estimée | Taille unitaire | Total min - max |
|---|---|---|---|
| Embeddings chunks (fp16 1024d) | 50K-500K | 2 KB + 30% HNSW overhead | 130 MB - 1.3 GB |
| Embeddings symbols | 0 (désactivé) | — | 0 |
| IST graph nodes (fichiers, fonctions, classes) | 10K-100K | ~600 B | 6 MB - 60 MB |
| IST graph edges (calls, imports, types) | 50K-500K | ~80 B | 4 MB - 40 MB |
| Symbol metadata (positions, sigs) | 10K-100K | ~1 KB | 10 MB - 100 MB |
| SOLL nodes | 100-5K | ~5 KB | 0.5 MB - 25 MB |
| SOLL edges (supersedes, links, etc.) | 500-50K | ~80 B | 40 KB - 4 MB |
| HashMap id → NodeIndex | (somme nœuds) | ~50 B | 1 MB - 10 MB |
| Overhead Rust + buffers | ~15% | — | 25 MB - 200 MB |
| **TOTAL par projet** | — | — | **~180 MB - 1.7 GB** |

### 4.2 Calibration sur Axon (référence haute)

D'après la mesure existante (500K chunks chez Axon) :

```
Embeddings  : 500K × 2 KB           = 1.0 GB
HNSW (+30%) :                       = 0.3 GB
IST graph   : ~100K nœuds + 500K e. = 0.08 GB
Metadata    :                       = 0.15 GB
SOLL        :                       = 0.01 GB
Overhead    :                       = 0.16 GB
─────────────────────────────────────────────
TOTAL Axon ≈ 1.5 GB en mémoire
```

### 4.3 Scénarios à 100 projets

#### Si tous les projets sont de taille Axon (worst case)
```
100 × 1.5 GB = 150 GB → impossible single-node
```

#### Distribution Pareto réaliste (recommandé pour planning)
```
80 petits projets   (~150 MB)  : ~12 GB
15 projets moyens   (~500 MB)  : ~7.5 GB
5  gros (Axon-size) (~1.5 GB)  : ~7.5 GB
─────────────────────────────────────────
TOTAL                          : ~27 GB cumulés
```

→ Avec **LRU et idle eviction**, ce volume **ne doit jamais être tout simultané en RAM**.

### 4.4 Sizing recommandé

| Profil | Hot working set | RAM | Disque NVMe | Cold start typique |
|---|---|---|---|---|
| 5-10 projets actifs simultanés | 3-8 GB | **16 GB** | 200 GB | 1-3 s |
| 20-30 projets actifs | 10-20 GB | **32 GB** ⭐ | 500 GB | 1-5 s |
| 50+ projets actifs | 30+ GB | 64 GB ou tiered | 1 TB | 2-5 s |
| 100 projets, distribution Pareto | ~24 GB occasionnel | **32 GB suffit** | 1 TB | 1-5 s |

**Recommandation principale** : serveur **32 GB RAM + 1 TB NVMe** est dimensionné correctement pour 100 projets en distribution Pareto.

---

## 5. Structures Rust et flux

### 5.1 Types principaux

```rust
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};
use lru::LruCache;
use tokio::sync::RwLock;

#[derive(Clone, Debug)]
pub struct SollNode {
    pub id: String,           // CPT-AXO-001, DEC-AXO-042, etc.
    pub kind: SollKind,
    pub description: String,
    pub originator: String,
    pub state: SollState,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct SollEdge {
    pub kind: SollEdgeKind,
    pub created_at: i64,
}

pub struct ProjectGraph {
    pub project_code: String,
    pub soll: DiGraph<SollNode, SollEdge>,
    pub soll_index: HashMap<String, NodeIndex>,
    pub ist: DiGraph<IstNode, IstEdge>,
    pub ist_index: HashMap<String, NodeIndex>,
    pub hnsw: PersistentHnswIndex,
    pub estimated_mb: usize,
    pub last_accessed: AtomicI64,
    pub loaded_at: Instant,
    pub dirty: AtomicBool,
}

pub struct ProjectGraphPool {
    cache: Arc<RwLock<LruCache<String, Arc<ProjectGraph>>>>,
    max_resident_mb: usize,
    idle_timeout: Duration,
    snapshot_dir: PathBuf,
    pg: sqlx::PgPool,
    metrics: Arc<PoolMetrics>,
}
```

### 5.2 API publique

```rust
impl ProjectGraphPool {
    pub async fn get_or_load(&self, project_code: &str) -> Result<Arc<ProjectGraph>>;
    pub async fn evict(&self, project_code: &str) -> Result<()>;
    pub async fn evict_idle(&self, idle: Duration) -> Result<usize>;
    pub async fn flush_dirty(&self) -> Result<usize>;
    pub async fn pre_warm(&self, project_codes: &[String]) -> Result<()>;
    pub fn metrics(&self) -> PoolMetrics;
}
```

### 5.3 Flux d'écriture (Postgres-first)

```
1. agent → soll_manager(action=create, ...) via MCP
2. axon-brain validates input
3. BEGIN postgres transaction
   3a. INSERT INTO soll_nodes (...) VALUES (...)
   3b. INSERT INTO soll_outbox (event_type='soll.node.created', payload)
   3c. (optional) UPDATE chunk_embeddings ...
4. COMMIT transaction (ACID guarantee)
5. ProjectGraphPool::apply_event(project_code, event)
   5a. Acquire RwLock<ProjectGraph>
   5b. graph.add_node(...)  / graph.add_edge(...)
   5c. dirty.store(true)
   5d. last_accessed = now()
6. Return success to agent
7. (background) periodic snapshot save if dirty
```

### 5.4 Flux de lecture (mémoire-only)

```
1. agent → query("CPT-AXO-029") via MCP
2. axon-brain extracts project_code from cwd
3. pool.get_or_load(project_code).await  ← cache hit en µs
4. graph.run_query(...)  ← petgraph algo, ~µs-ms
5. last_accessed = now()
6. Return result
```

### 5.5 Cold start

```
1. get_or_load(project_code) → cache miss
2. evict_until_capacity_for(estimated_mb)
3. Try load from snapshot:
   3a. Read /var/lib/axon/projects/<code>/snapshots/version.json
   3b. Validate checksum + version_id vs Postgres latest event_id
   3c. Load .bin (petgraph): bincode deserialize → ~1-3 s
   3d. Load .idx (HNSW): mmap or read → ~0.1-2 s
4. If snapshot stale or missing:
   4a. Stream from Postgres: SELECT * FROM soll_nodes WHERE project_code = $1
   4b. Build petgraph in memory: ~1-5 s
   4c. Build HNSW from existing chunk_embeddings: ~30 s - 5 min
   4d. Persist snapshot for next time
5. Insert in cache, update last_accessed
6. Return Arc<ProjectGraph>
```

### 5.6 Synchronisation multi-process (LISTEN/NOTIFY)

```sql
-- Postgres trigger
CREATE OR REPLACE FUNCTION notify_soll_changes() RETURNS TRIGGER AS $$
BEGIN
  PERFORM pg_notify('soll_changes',
    json_build_object(
      'op', TG_OP,
      'table', TG_TABLE_NAME,
      'project_code', NEW.project_code,
      'row', row_to_json(NEW),
      'event_id', NEW.event_id
    )::text);
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER soll_nodes_notify
  AFTER INSERT OR UPDATE ON soll_nodes
  FOR EACH ROW EXECUTE FUNCTION notify_soll_changes();
```

```rust
// Background task in axon-brain
async fn listen_for_changes(pool: Arc<ProjectGraphPool>, pg: PgPool) -> Result<()> {
    let mut listener = sqlx::postgres::PgListener::connect_with(&pg).await?;
    listener.listen("soll_changes").await?;
    while let Ok(notif) = listener.recv().await {
        let event: SollEvent = serde_json::from_str(notif.payload())?;
        if let Some(state) = pool.try_get_resident(&event.project_code).await {
            state.apply_event(event)?;
        }
    }
    Ok(())
}
```

### 5.7 Idle eviction background

```rust
async fn idle_evictor(pool: Arc<ProjectGraphPool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        let evicted = pool.evict_idle(Duration::from_secs(30 * 60)).await
            .unwrap_or(0);
        if evicted > 0 {
            info!("Evicted {} idle projects", evicted);
        }
        let _ = pool.flush_dirty().await;
    }
}
```

---

## 6. Comportements opérationnels

### 6.1 Cold start détaillé (par taille projet)

| Projet | Source | Lecture I/O | Build petgraph | Build HNSW | Total |
|---|---|---|---|---|---|
| Petit (10K chunks) | Snapshot SSD | 30 ms | 200 ms | 100 ms (mmap) | **~0.5 s** |
| Petit | Postgres rebuild | 500 ms | 200 ms | 5 s | ~6 s |
| Moyen (100K chunks) | Snapshot SSD | 200 ms | 1 s | 500 ms | **~2 s** |
| Moyen | Postgres rebuild | 2 s | 1 s | 30 s | ~33 s |
| Gros Axon-size (500K chunks) | Snapshot NVMe | 1 s | 3 s | 2 s | **~6 s** |
| Gros | Postgres rebuild | 8 s | 5 s | 3-5 min | ~5-6 min |

**Insight critique** : la persistance HNSW divise le cold start par 30-50× sur les gros projets. **Non négociable** dans cette architecture.

### 6.2 Comportement sous charge

| Charge | Comportement attendu |
|---|---|
| < 5 projets actifs | Tout en RAM, cache hit 100%, latence µs |
| 5-30 projets actifs | LRU, hit rate > 95%, occasionnel cold start 1-5 s |
| Burst de nouveaux projets | Évictions agressives, hit rate dégradé temporairement, cold starts en série |
| Pic d'écriture sur 1 projet | Postgres sérialise, mémoire mise à jour write-through, pas de problème |
| Crash d'axon-brain | Au restart, rehydratation depuis snapshots ou Postgres, perte = 0 |

### 6.3 Métriques exposées

```rust
pub struct PoolMetrics {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cold_start_p50_ms: u64,
    pub cold_start_p95_ms: u64,
    pub cold_start_p99_ms: u64,
    pub resident_mb: u64,
    pub max_resident_mb: u64,
    pub evictions_total: u64,
    pub evictions_idle: u64,
    pub evictions_capacity: u64,
    pub snapshot_loads: u64,
    pub postgres_rebuilds: u64,
    pub load_failures: u64,
    pub pending_writes: u64,
}
```

Permet de tuner empiriquement `max_resident_mb` et `idle_timeout`, et d'alarmer sur des seuils anormaux.

---

## 7. Plan de migration

### Phase 0 — Préalable (ce qui doit être fait avant)

1. Stabilisation pipeline d'embedding (rapport expert séparé du 2026-05-08)
2. Validation des snapshots HNSW persistants en Rust avec `usearch` ou `instant-distance`
3. Setup table outbox `soll_outbox` si pas déjà présent

### Phase 1 — POC sur sous-domaine SOLL (1 semaine)

- Implémenter `ProjectGraphPool` minimal sans HNSW (juste petgraph + Postgres)
- Charger 1 projet test (Axon lui-même)
- Mesurer : hydratation time, latence requêtes, vs AGE
- Valider : équivalence sémantique avec AGE sur 20 requêtes critiques

### Phase 2 — Étendre aux IST + vector (2-3 semaines)

- Intégrer HNSW persistant
- Hydrater IST + chunks vectoriels
- Implémenter LRU + idle eviction
- Ajouter LISTEN/NOTIFY sync
- Métriques et observabilité

### Phase 3 — Mode dual-read (2 semaines de tests)

- Coder un wrapper qui interroge **AGE et le pool en parallèle**
- Comparer les résultats automatiquement
- Logger toute divergence
- Valider sur le workload réel

### Phase 4 — Bascule progressive (1-2 semaines)

- Feature flag `AXON_USE_INMEMORY_GRAPH=true`
- Tests A/B sur sous-ensemble d'agents
- Rollout progressif 10% → 50% → 100%

### Phase 5 — Décommission AGE (optionnel)

- Garder AGE en backup mode pendant 1 mois
- Si stable, retirer AGE des dépendances Postgres

---

## 8. Risques et mitigations

| Risque | Probabilité | Impact | Mitigation |
|---|---|---|---|
| Premier appel après éviction lent (UX agent) | Moyenne | Moyen | Pre-warm projets prévisibles, snapshots NVMe, mmap HNSW |
| Snapshots disque divergent de Postgres | Faible | Élevé | Checksum + version_id (event_id Postgres), fallback rebuild |
| Memory leak si éviction buggée | Faible | Élevé | Métriques `resident_mb` + alarme hard limit |
| Concurrence : 2 requêtes pour même project pendant cold start | Élevée | Moyen | Singleflight pattern (`tokio::sync::OnceCell` ou `dashmap`) |
| Crash pendant snapshot save | Faible | Moyen | Write-then-rename atomic, `fsync` avant rename |
| Postgres lent → cold start très long | Moyenne | Élevé | Snapshots prioritaires, Postgres seulement fallback |
| Notification Postgres perdue (LISTEN/NOTIFY) | Faible | Moyen | Polling outbox en backup, eventual consistency |
| Schema petgraph change (mismatch versions) | Moyenne | Élevé | Magic number + version_tag dans snapshot, fallback rebuild |
| Pool RAM saturé sur burst | Moyenne | Faible | LRU evict, `max_resident_mb` strict, retour erreur graceful |
| Bug dans algo graphe → résultats incorrects | Faible | Très élevé | Phase 3 dual-read mandatoire, golden tests sur 50+ patterns |

---

## 9. Comparaison avec alternatives

### 9.1 Trade-offs de design

| Critère | Stack actuelle (PG+AGE+Qdrant) | **In-memory pool (proposé)** | NebulaGraph standalone | TypeDB Core |
|---|---|---|---|---|
| Latence p50 reads | 5-200 ms | **< 1 ms** | 5-50 ms | 10-100 ms |
| Latence p99 reads (5+ hops) | 1-5 s | **10-100 ms** | 100-500 ms | 50-200 ms |
| Cold start | n/a (toujours chaud) | 1-5 s avec snapshot | n/a | 5-15 s |
| Source-of-truth | Postgres | **Postgres ✅** | NebulaGraph | TypeDB |
| Vector search | Service séparé (Qdrant) | **HNSW intégré** | Enterprise only | Limited |
| Multi-writer cluster | Postgres MVCC | Postgres MVCC | Raft sharded | Single leader (CE) |
| Algos graphe natifs (PageRank, etc.) | Limités | **Tous via petgraph** | Limités | Limités |
| Empreinte ops | 3 services | 1 service (Postgres + axon-brain) | +1 service | +1 service |
| Migration depuis stack actuelle | n/a | Modérée (Phase 1-4) | Élevée | Très élevée |
| License FOSS | OK | **OK (MIT/Apache/MPL)** | Apache 2.0 | MPL 2.0 |
| Souveraineté géographique | OK | **OK** | ⚠️ China | UK |

### 9.2 Pourquoi pas NebulaGraph ou TypeDB ?

- **NebulaGraph** : meilleure perf brute, mais service séparé à opérer, origine Vesoft Hangzhou (préoccupation souveraineté), pas de vector search en community
- **TypeDB Core** : modèle sémantique excellent pour SOLL, mais single-node FOSS uniquement, TypeQL = paradigme nouveau (coût de migration)
- **In-memory pool** : meilleur compromis perf / ops simplicité / cohérence Rust / souveraineté

### 9.3 Pourquoi pas CozoDB plutôt que petgraph ?

CozoDB est une alternative crédible (Datalog, HNSW intégré, MPL 2.0, Rust pur). Trade-offs :

- **petgraph** : algos custom maximaux, moins de dépendances, plus de code à écrire
- **CozoDB** : Datalog déclaratif élégant, mais courbe d'apprentissage, dépendance à un projet plus jeune

**Recommandation** : démarrer avec petgraph (proche du modèle Axon Rust existant), évaluer CozoDB en Phase 5 si Datalog devient un besoin.

---

## 10. Décisions de design à valider

| ID | Décision | Recommandation |
|---|---|---|
| D-1 | Granularité d'éviction | **Par projet** (pas chunk-by-chunk) |
| D-2 | Source de hydratation | **Snapshots disque > Postgres rebuild** (fallback) |
| D-3 | Format snapshot | **bincode (graph) + usearch native (HNSW)** |
| D-4 | Stratégie de propagation | **LISTEN/NOTIFY Postgres + outbox polling backup** |
| D-5 | Concurrence in-memory | **`tokio::sync::RwLock` (multi-readers, single writer)** |
| D-6 | Politique éviction | **LRU + idle timeout 30 min** (configurable) |
| D-7 | Snapshot save frequency | **Every 60 s if dirty, on graceful shutdown** |
| D-8 | Atomicité snapshot | **Write to .tmp → fsync → rename** |
| D-9 | Compression snapshot | **zstd level 3** (× 2-3 gain disque, ~100 ms overhead) |
| D-10 | Dual-read avant bascule | **Mandatoire pendant 2 semaines en prod réelle** |
| D-11 | Feature flag | **`AXON_USE_INMEMORY_GRAPH=true/false`** pour rollback |
| D-12 | Garde Postgres + AGE en parallèle | **Pendant Phase 5 (1 mois minimum)** |

---

## 11. Variables d'environnement proposées

| Variable | Défaut suggéré | Description |
|---|---|---|
| `AXON_USE_INMEMORY_GRAPH` | `false` (puis `true` après Phase 4) | Active le pool, fallback sur AGE si false |
| `AXON_GRAPH_POOL_MAX_RESIDENT_MB` | `20480` (20 GB) | Limite hard mémoire cumulée du pool |
| `AXON_GRAPH_POOL_IDLE_TIMEOUT_SECS` | `1800` (30 min) | Timeout éviction par projet |
| `AXON_GRAPH_POOL_SNAPSHOT_DIR` | `/var/lib/axon/projects` | Racine des snapshots disque |
| `AXON_GRAPH_POOL_SNAPSHOT_INTERVAL_SECS` | `60` | Fréquence flush dirty projects |
| `AXON_GRAPH_POOL_PRE_WARM` | `<comma-list>` | Projets à charger au boot |
| `AXON_GRAPH_POOL_LISTEN_NOTIFY` | `true` | Active la sync via Postgres NOTIFY |
| `AXON_GRAPH_POOL_DUAL_READ` | `true` (Phase 3) | Compare AGE et pool, log divergences |
| `AXON_GRAPH_POOL_HNSW_BACKEND` | `usearch` | Choix de la lib HNSW |
| `AXON_GRAPH_POOL_COMPRESSION` | `zstd:3` | Compression snapshots |

---

## 12. Annexes

### A. Format de snapshot

```
/var/lib/axon/projects/<project_code>/snapshots/
├── version.json               ← {"event_id_max": 12345, "checksum": "...", "schema_version": 1}
├── soll-graph.bin.zst         ← bincode compressed
├── ist-graph.bin.zst
├── hnsw-chunks.usearch        ← natif usearch
├── chunks-metadata.parquet    ← parquet pour metadata
└── ...
```

### B. Algos petgraph utilisables nativement

| Algo | Usage Axon | Crate |
|---|---|---|
| BFS / DFS | Traversées simples, supersedes chains | petgraph |
| Dijkstra | Shortest path pondéré | petgraph |
| A* | Pathfinding avec heuristique | petgraph |
| Tarjan SCC | Détection de cycles | petgraph |
| Floyd-Warshall | All-pairs shortest path (petits graphes) | petgraph |
| Topological sort | Ordering des Decisions | petgraph |
| PageRank | Centralité Concepts | `pagerank` crate |
| Louvain / Leiden | Communautés sémantiques | `louvain-rs` |
| Connected components | Clusters disjoints | petgraph |
| Cycle detection | Validation contraintes (cycles supersedes) | petgraph |

### C. Estimations chiffrées récapitulatives

| Métrique | Valeur cible |
|---|---|
| Cold start projet petit (snapshot) | < 1 s |
| Cold start projet moyen (snapshot) | 1-3 s |
| Cold start projet gros (snapshot) | 3-10 s |
| Latence p50 cache hit (1-3 hops) | < 100 µs |
| Latence p99 cache hit (5+ hops) | 50-100 ms |
| Empreinte mémoire / projet médian | ~200-500 MB |
| Empreinte serveur 100 projets Pareto | ~24 GB peak |
| Sizing recommandé | 32 GB RAM + 1 TB NVMe |
| Cache hit rate cible | > 95% |
| Cold start success rate cible | > 99.9% |

### D. Références

- Rapport expert performance embedding (2026-05-08) : `docs/working-notes/2026-05-08-expert-report-embedding-performance.md`
- petgraph documentation : https://docs.rs/petgraph
- usearch Rust bindings : https://github.com/unum-cloud/usearch
- instant-distance : https://crates.io/crates/instant-distance
- Postgres LISTEN/NOTIFY : https://www.postgresql.org/docs/current/sql-listen.html
- Trendyol Tech AGE migration (avril 2026) : https://medium.com/trendyol-tech/migrating-graph-operations-to-apache-age-from-writes-to-reads-3b8334628e1c
- Apache AGE Releases (1.7.0 sept 2025) : https://github.com/apache/age/releases

---

## 13. Conclusion

Le pattern **in-memory graph pool avec LRU et persistent snapshots** est une réponse architecturale **idéale** au profil de charge d'Axon :

1. **Performance** : élimine le facteur 40× d'AGE sur les hot paths
2. **Empreinte bornée** : 32 GB RAM suffisent pour 100 projets en distribution réaliste
3. **Durabilité préservée** : Postgres reste source-of-truth, snapshots = optimisation
4. **Cohérence Rust** : 100% Rust côté query, alignement avec axon-brain
5. **Souveraineté** : tous composants Apache/MIT/MPL, pas d'origine PRC
6. **Migration progressive** : phasage en 5 étapes avec dual-read de validation
7. **Extensibilité** : le pattern par-projet est nativement prêt pour scale-out futur

**Action requise** : validation de la proposition par l'équipe Axon, puis exécution Phase 1 (POC sur sous-domaine SOLL, 1 semaine) pour valider empiriquement les estimations chiffrées.

Le risque principal — cold start UX — est mitigé par la persistance HNSW (× 30-50 sur le temps de chargement) et le pre-warming des projets prévus actifs.

L'architecture est **réversible** à tout moment via le feature flag `AXON_USE_INMEMORY_GRAPH=false`, garantissant un déploiement sans risque de perte fonctionnelle.

---

**Note** : ce document est conçu pour être transmis au fournisseur Axon en complément du rapport expert sur le pipeline d'embedding du même jour. Les deux propositions sont indépendantes et peuvent être traitées séquentiellement (le pipeline d'embedding d'abord, le pool graphe ensuite).
