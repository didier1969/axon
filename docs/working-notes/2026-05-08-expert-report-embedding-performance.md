# Rapport d'expertise — Performance du pipeline d'embedding Axon

**Date** : 2026-05-08
**Auteur** : Analyse externe sollicitée par le client (audit ad-hoc, base de code lue directement)
**Périmètre** : `src/axon-core/src/embedder/` + `src/axon-core/src/embedder.rs`
**Version analysée** : branche `main` à la date d'audit
**Référence client** : 36h pour 500K chunks observés en production ; cible 1h30 (≈ 93 chunks/s end-to-end)
**Hardware client** : GPU 8 GB VRAM

---

## 1. Résumé exécutif

L'infrastructure d'embedding d'Axon est **techniquement mature et bien construite** : ONNX Runtime avec IO Binding, exécuteurs CUDA EP et TensorRT EP, FP16, BGE-Large 1024d, FastEmbed vendoré v5.12.1, engine cache TensorRT, Parquet side-store optionnel pour bypasser la pression column-store DuckDB. Le **bench L1 documenté atteint ≥ 140 chunks/s warm** (DEC-AXO-068 / VAL-AXO-028).

**Le throughput observé en production est de ~3,86 chunks/s** (500 000 chunks en 36h). L'écart entre bench L1 et end-to-end est d'environ **36× — ce qui démontre que le GPU n'est pas saturé**. Le goulot d'étranglement se situe dans l'orchestration du worker loop et dans les phases CPU/IO (claim DB, tokenisation, persist DB, finalize).

**La cible client de 93 chunks/s est largement atteignable** sans modifier le hardware ni le modèle, principalement par :
1. Des changements de variables d'environnement (priorités 2-4, < 1 jour-homme)
2. Un refactor du worker loop en pipeline 3-stages (priorité 1, 3-5 jours-homme)

Gain cumulé estimé après priorités 1-4 : **× 25 à × 40 du throughput actuel**, soit 100-150 ch/s end-to-end.

---

## 2. Architecture observée — points forts

L'audit a relevé les éléments suivants, qui témoignent d'une expertise réelle de l'équipe Axon :

| Composant | Fichier / référence | Évaluation |
|---|---|---|
| Session ORT avec `GraphOptimizationLevel::Level3`, IO Binding, RunOptions persistants, output bind to device, synchronisation explicite | `embedder/gpu_backend.rs:53-273` | Bonne pratique |
| TensorRT EP avec engine cache + timing cache, FP16, optimization level 5, `with_force_timing_cache(true)` | `embedder/gpu_backend.rs:334-376` | Excellent |
| Pooling CLS pré-déterminé, tokenizer chargé une seule fois | `embedder/vector_worker_loop.rs:79-90` | Correct |
| Liveness guards + per-stage timing instrumenté (`vector-lane.trace`) | `embedder/vector_worker_loop.rs:21-33, 429-444` | Excellent (l'observabilité est déjà là) |
| Parquet side-store pour bypass column-store DuckDB | `embedder/parquet_embedding_store.rs` (DEC-AXO-073) | Excellent |
| Hot status cache pour éviter le JOIN DB sur claim | `vector_worker_loop.rs:140-194` (DEC-AXO-072 J.4) | Excellent |
| Single-loop architecture | `vector_worker_loop.rs:1-11` (DEC-AXO-070) | **À reconsidérer** — voir §3 |

**Conclusion partie 2** : la base technique est solide. Les axes d'amélioration ne sont pas des défauts de qualité de code, mais des **choix de design** qui privilégient la simplicité au détriment du throughput.

---

## 3. Diagnostic du bottleneck

### 3.1 Constantes de configuration

**Fichier** : `src/axon-core/src/embedder.rs`, lignes 136-143

```rust
const CHUNK_BATCH_SIZE: usize = 16;
const SYMBOL_BATCH_SIZE: usize = 32;
const FILE_VECTORIZATION_BATCH_SIZE: usize = 8;
const GRAPH_BATCH_SIZE: usize = 6;
const VECTOR_PERSIST_QUEUE_BOUND: usize = 4;
const MAX_CHUNKS_PER_FILE: usize = 64;
const MAX_EMBED_BATCH_BYTES: usize = 4 * 1024 * 1024;
```

**Observations** :
- `CHUNK_BATCH_SIZE = 16` est petit pour BGE-Large sur GPU 8 GB VRAM (le sweet spot empirique se situe à 48-96)
- `VECTOR_PERSIST_QUEUE_BOUND = 4` est très contraint et peut créer du back-pressure
- `MAX_EMBED_BATCH_BYTES = 4 MB` peut limiter prématurément les batches sur fichiers riches

### 3.2 Workers vectoriels

**Fichier** : `src/axon-core/src/embedder.rs`, lignes 340 + 3554-3567

```rust
let requested_vector_workers = env_usize("AXON_VECTOR_WORKERS", 1);
// ...
// test_embedding_lane_config_caps_gpu_vector_workers_to_one_on_8gb_vram
// confirme : sur GPU 8GB, le cap est forcé à 1 sans opt-in explicite
// (AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION)
```

**Observation** : par défaut, **un seul worker vectoriel** sur 8 GB VRAM. C'est conservateur mais cohérent avec la sécurité OOM. Le worker unique exécute toutes les phases en série.

### 3.3 Architecture du worker loop

**Fichier** : `src/axon-core/src/embedder/vector_worker_loop.rs`, lignes 114-462
**Référence design** : DEC-AXO-070

L'unique worker exécute, par itération, dans cet ordre strict :

```
1. claim                              (DB read    ~10-100 ms)
2. mark_file_vectorization_started    (DB write   ~5-30 ms)
3. prepare_vector_embed_batch         (DB read    ~20-100 ms)
4. attach_preencoded_micro_batches    (CPU tok.   ~20-200 ms)
5. embed_prepared_batch_with_breakdown (GPU       ~50-150 ms)   ← seul stage GPU
6. update_chunk_embeddings            (DB write   ~10-100 ms)
7. mark_file_vectorization_work_done  (DB write   ~5-30 ms)
8. polling 50 ms si vide              (idle)
```

**Le GPU est inactif pendant les stages 1, 2, 3, 4, 6, 7, 8.** Sur un cycle typique, l'inférence GPU représente 50-150 ms sur 200-500 ms de cycle total. **Utilisation GPU réelle estimée : 20-30 %**.

Le commentaire architectural confirme ce choix :
> "Replaces the previous 5-loop pipeline (refill → prepare → worker → persist → finalize) with one synchronous claim → prepare → embed → persist → finalize function."
> — `vector_worker_loop.rs:3-5`

Ce design **optimise la simplicité de debug** au détriment du débit. Il est recommandé de revenir à un design pipeline pour les workloads d'ingestion massive.

### 3.4 TF32 désactivé par défaut

**Fichier** : `src/axon-core/src/embedder/gpu_backend.rs`, lignes 390-400

```rust
pub(super) fn cuda_tf32_enabled() -> bool {
    std::env::var("AXON_CUDA_ALLOW_TF32")
        .ok()
        .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(),
                              "1" | "true" | "yes"))
        .unwrap_or(false)  // off par défaut
}
```

**Observation** : TensorFloat-32 est désactivé par défaut sur ce GPU (Ampere+). TF32 fournit typiquement 1.5× à 2× sur les modèles BGE/BERT-class avec un impact sur le recall < 0.5 %.

### 3.5 Cible documentée vs observé

**Fichier** : `src/axon-core/src/embedder.rs`, ligne 2594

```rust
/// (target throughput: 30 chunks/s end-to-end → 200 chunks/s stretch).
```

| Métrique | Valeur |
|---|---|
| **Cible end-to-end documentée** | 30 ch/s |
| **Cible stretch documentée** | 200 ch/s |
| **Bench L1 GPU mesuré (warm)** | ≥ 140 ch/s |
| **Cible client (1.5h pour 500K)** | 93 ch/s |
| **Observé en production** | ~3,86 ch/s |

L'écart de 36× entre bench L1 et observé indique que **l'orchestration end-to-end perd 97 % du débit GPU**.

---

## 4. Recommandations ordonnées par impact

### 🔴 Priorité 1 — Refactor en pipeline 3-stages (gain estimé × 8 à × 12)

**Diagnostic** : DEC-AXO-070 a unifié 5 loops en 1 pour la simplicité. Sur le workload d'ingestion massive, ce choix force le GPU à attendre les phases CPU et IO. Le bénéfice de simplicité ne compense plus le coût de débit.

**Proposition** : pipeline producteur-consommateur à 3 threads, communicant par `crossbeam_channel::bounded` :

```
Thread A — Producer
  Stages : claim + mark_started + prepare + tokenize
  Output : channel bounded(8) of PreparedVectorEmbedBatch

Thread B — GPU Embedder (single thread, garde le modèle ORT)
  Stage  : embed (GPU)
  Output : channel bounded(8) of EmbeddedBatch

Thread C — Persister
  Stages : update_chunk_embeddings (Parquet ou DuckDB) + finalize + mark_done
```

**Bénéfices** :
- Le GPU est saturé : un batch est toujours prêt à consommer
- Le GPU thread ne fait que de l'inférence, le reste est masqué
- La backpressure naturelle des channels protège la VRAM

**Risques** :
- La complexité d'orchestration revient (l'anti-pattern explicite de DEC-AXO-070)
- Liveness guards et watchdog axonctl doivent être adaptés à 3 threads
- Tests de crash isolation à reconduire

**Coût** : 3 à 5 jours-homme. **Tranchage stratégique requis** : la simplicité du single-loop vaut-elle 36× le throughput pour ce workload ?

### 🟠 Priorité 2 — Augmenter `CHUNK_BATCH_SIZE` à 48-64 (gain estimé × 1.5 à × 3)

**Diagnostic** : `CHUNK_BATCH_SIZE = 16` (`embedder.rs:136`) est sous-optimal pour BGE-Large sur 8 GB VRAM.

**Estimation mémoire BGE-Large à batch 64, FP16** :

```
Modèle FP16            : ~700 MB
Activations par batch  : 1024d × 512 tokens × 64 batch × 2 bytes ≈ 64 MB par layer
Activations 24 layers  : ~1.5 GB
KV cache + buffers     : ~500 MB
Marge ORT + CUDA       : ~1 GB
─────────────────────────────────────
Total estimé            : ~3.7 GB / 8 GB
```

→ Marge confortable de 4+ GB pour batch 64. Possible aussi de pousser à 96 selon le profil tokens.

**Action immédiate** :

```bash
export AXON_CHUNK_BATCH_SIZE=64
export AXON_MAX_EMBED_BATCH_BYTES=$((16 * 1024 * 1024))   # 16 MB
```

**Validation** :

```bash
cargo run --bin embedder-bench -- --batch 16 --batch 32 --batch 48 --batch 64 --batch 96
```

Comparer `chunks_per_second()` pour identifier le sweet spot. Le code de bench existe déjà (`run_embedder_throughput_bench` à `embedder.rs:2599`).

**Note** : on observe que dans certains chemins de bootstrap auto-config (`embedder.rs:3168`), `AXON_CHUNK_BATCH_SIZE=64` est déjà appliqué. Il s'agit donc d'aligner la production sur cette configuration éprouvée par les tests.

### 🟠 Priorité 3 — Activer `AXON_CUDA_ALLOW_TF32=1` (gain estimé × 1.5 à × 2)

**Diagnostic** : TF32 désactivé par défaut (`gpu_backend.rs:390-400`).

**Action** :

```bash
export AXON_CUDA_ALLOW_TF32=1
```

**Validation** :
- Recall sur le golden set d'évaluation interne : doit rester ≥ 99 % du baseline FP32/FP16
- Bench throughput L1 : noter le gain réel mesuré

**Risque** : faible. TF32 est largement adopté en production sur les modèles transformer-class.

### 🟠 Priorité 4 — Augmenter `VECTOR_PERSIST_QUEUE_BOUND` (gain estimé × 1.3 à × 2)

**Diagnostic** : `VECTOR_PERSIST_QUEUE_BOUND = 4` (`embedder.rs:141`) est très contraint. Si la persist DuckDB ralentit, le back-pressure remonte jusqu'au GPU.

**Action** :

```bash
export AXON_VECTOR_PERSIST_QUEUE_BOUND=64
export AXON_PARQUET_EMBEDDING_STORE_ENABLED=true   # confirmer activation
```

Le Parquet side-store (DEC-AXO-073) doit être activé en production : il bypasse le coût column-store DuckDB et élimine la pression principale sur cette file.

### 🟡 Priorité 5 — Tokenisation hors thread GPU (gain estimé × 1.2 à × 1.5)

**Diagnostic** : `attach_preencoded_micro_batches` tokenise dans le main loop, juste avant l'inférence. Le GPU attend.

**Proposition** : tokeniser dans un thread dédié (rayon ou tokio compute pool), avec channel vers le GPU thread. Cohérent avec la priorité 1 (le pipeline 3-stages englobe naturellement cette amélioration).

### 🟡 Priorité 6 — 2 vector workers via NVIDIA MPS (gain estimé × 1.5 à × 1.8)

**Diagnostic** : `test_embedding_lane_config_caps_gpu_vector_workers_to_one_on_8gb_vram` cap conservativement à 1 worker sur 8 GB.

**Proposition** : avec NVIDIA MPS activé et `cuda_memory_limit_bytes` à 3 GB par worker, deux workers concurrents partagent le GPU. Les phases CPU/DB de l'un masquent les phases GPU de l'autre.

**Pré-requis** :
- `AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION=1`
- `AXON_VECTOR_WORKERS=2`
- `AXON_CUDA_MEMORY_LIMIT_MB=3000`
- MPS daemon démarré : `nvidia-cuda-mps-control -d`

**Caveat** : MPS ajoute de la complexité ops. À évaluer après priorités 1-4.

### 🟢 Priorité 7 — Quantization INT8 (gain estimé × 1.5 à × 2 supplémentaire)

**Action** : convertir BGE-Large en ONNX INT8 :

```bash
optimum-cli onnxruntime quantize --onnx_model model.onnx --output model_int8.onnx --avx512
```

Le pipeline ORT existant supporte INT8 sans changement majeur. Gain typique 1.5-2× throughput, perte recall ~1-2 %.

À évaluer **après** validation des priorités 1-4 et seulement si le throughput cible n'est pas encore atteint.

---

## 5. Estimation chiffrée des gains cumulés

| Action | Gain isolé | Throughput attendu cumulé | Effort |
|---|---|---|---|
| Baseline (production observé) | 1× | **3,86 ch/s** | — |
| + `CHUNK_BATCH_SIZE=64` + `MAX_EMBED_BATCH_BYTES=16MB` | × 2 | **8 ch/s** | Variables d'env |
| + Pipeline 3-stages (priorité 1) | × 8 | **64 ch/s** | 3-5 j |
| + `AXON_CUDA_ALLOW_TF32=1` | × 1.5 | **96 ch/s** | Variables d'env |
| + `VECTOR_PERSIST_QUEUE_BOUND=64` + Parquet store | × 1.3 | **125 ch/s** | Variables d'env |
| + Tokenize off-thread | × 1.2 | **150 ch/s** | Inclus dans P1 |
| + INT8 (optionnel) | × 1.7 | **255 ch/s** | 1-2 j de validation |

**Cible client 93 ch/s** : atteinte dès l'application des priorités 1 + 2 + 3.

**Cible stretch 200 ch/s** (déjà documentée par l'équipe Axon dans le code) : atteinte avec les priorités 1-4 + INT8.

**Le hardware 8 GB est très loin d'être saturé**.

---

## 6. Sur la question "GPU 8 GB saturé"

**Le GPU n'est pas saturé.** Trois éléments le démontrent :

1. **Le bench L1 interne d'Axon mesure ≥ 140 ch/s warm** (`embedder.rs:2594`, DEC-AXO-068, VAL-AXO-028) — soit 36× le throughput end-to-end actuel.
2. **L'utilisation GPU réelle est estimée à 20-30 %** par analyse du worker loop (proportion temps GPU / temps cycle).
3. **BGE-Large à FP16 sur GPU 8 GB devrait soutenir 250-400 ch/s** avec une configuration optimale (batch 64-96, TF32, TensorRT engine bien rodé). Le 140 ch/s du bench L1 actuel est **lui-même probablement sous-optimal**.

**Diagnostic empirique recommandé** pendant l'ingestion :

```bash
nvidia-smi dmon -s puct -d 1
```

À observer :
- **GPU utilization < 80 %** → GPU sous-utilisé (cas suspecté actuel)
- **Memory bandwidth > 70 %** → memory-bound (rare pour BGE-Large)
- **TDP atteint** → compute-bound (vraiment saturé)

---

## 7. Validations et hypothèses non confirmées

À confirmer côté équipe Axon avant action définitive :

1. **Distribution des per-stage timings** : exploiter `<AXON_RUN_ROOT>/vector-lane.trace` (`vector_worker_loop.rs:21-33`) sur 1 000 fichiers d'ingestion réelle pour quantifier précisément les ratios `inter_idle_ms / claim_ms / mark_started_ms / prep_ms / tok_ms / embed_ms / persist_ms / finalize_ms`. Ces traces sont déjà émises automatiquement.

2. **Mode subprocess IPC** : le commentaire NEXUS v10.5 (`embedder.rs:160-163`) mentionne un possible run du modèle dans un subprocess IPC pour isoler les aborts Tokio/jemalloc. Si ce mode est actif, la latence IPC ajoute une composante non mesurée.

3. **Parquet side-store actif en production** : la pénalité column-store DuckDB (VAL-AXO-034) impose ce store. Vérifier que `AXON_PARQUET_EMBEDDING_STORE_ENABLED=true` est positionné en production.

4. **TensorRT engine bien rodé** : le first run avec TensorRT EP peut être très lent (engine build). S'assurer que les caches `engine-cache/` et `timing-cache/` sont persistants entre runs.

---

## 8. Plan d'action recommandé

### Semaine 1 — Diagnostic empirique (bas coût, haute valeur)

1. Activer la collecte `vector-lane.trace` sur un workload réel de 1 000 fichiers
2. Run `nvidia-smi dmon -s puct -d 1` en parallèle
3. Mesurer chunks_per_second sur le bench L1 avec batch 16, 32, 48, 64, 96
4. Confirmer/infirmer chacune des hypothèses du §7

### Semaine 2 — Wins faciles (variables d'env uniquement, zéro refactor)

```bash
export AXON_CHUNK_BATCH_SIZE=64
export AXON_MAX_EMBED_BATCH_BYTES=$((16 * 1024 * 1024))
export AXON_CUDA_ALLOW_TF32=1
export AXON_VECTOR_PERSIST_QUEUE_BOUND=64
export AXON_PARQUET_EMBEDDING_STORE_ENABLED=true
```

Mesurer le gain réel. Si la cible client (93 ch/s) est atteinte, **stop here**.

### Semaines 3-4 — Refactor pipeline (si besoin)

Si après les wins faciles le throughput reste sous 93 ch/s, exécuter la priorité 1 (pipeline 3-stages). Tests E2E sur 10 000 chunks réels, doit dépasser 100 ch/s warm.

### Semaine 5+ — Optimisations avancées (optionnel)

NVIDIA MPS pour 2 workers concurrents, INT8 quantization, etc. Seulement si stretch > 200 ch/s souhaité.

---

## 9. Conclusion

L'équipe Axon a construit une infrastructure d'embedding mature et bien instrumentée. Le throughput observé en production n'est pas un problème de capacité hardware ni de qualité de code, mais un problème de **design choices sur le worker loop** (DEC-AXO-070) qui privilégie la simplicité au détriment du débit.

**La cible client de 93 chunks/s est largement atteignable** sur le hardware actuel, sans remettre en cause les choix architecturaux fondamentaux (BGE-Large 1024d, ORT, CUDA EP, TensorRT EP, FP16). Le gain attendu sur l'infrastructure existante est de **× 25 à × 40** par application des priorités 1-4.

L'instrumentation `vector-lane.trace` déjà en place permet de **valider empiriquement chaque hypothèse de ce rapport** sans nouveau code à écrire.

---

## Annexe A — Références code (chemins absolus depuis racine repo Axon)

| Référence | Chemin |
|---|---|
| Constantes embedder | `src/axon-core/src/embedder.rs:136-146` |
| Lane config / workers spawn | `src/axon-core/src/embedder.rs:340, 1235-1252` |
| Worker loop unique | `src/axon-core/src/embedder/vector_worker_loop.rs:1-462` |
| GPU backend ORT | `src/axon-core/src/embedder/gpu_backend.rs:48-376` |
| TF32 flag | `src/axon-core/src/embedder/gpu_backend.rs:390-400` |
| Bench cible | `src/axon-core/src/embedder.rs:2594` |
| Parquet side-store | `src/axon-core/src/embedder/parquet_embedding_store.rs:182-184` |
| Trace per-stage | `src/axon-core/src/embedder/vector_worker_loop.rs:21-33, 429-444` |
| Test cap GPU 8GB | `src/axon-core/src/embedder.rs:3554-3567` |

## Annexe B — Variables d'environnement pertinentes (toutes documentées dans le code)

| Variable | Défaut | Recommandation |
|---|---|---|
| `AXON_VECTOR_WORKERS` | 1 | 1 (ou 2 avec MPS — priorité 6) |
| `AXON_CHUNK_BATCH_SIZE` | 16 | **64** (priorité 2) |
| `AXON_MAX_EMBED_BATCH_BYTES` | 4 MB | **16 MB** (priorité 2) |
| `AXON_VECTOR_PERSIST_QUEUE_BOUND` | 4 | **64** (priorité 4) |
| `AXON_CUDA_ALLOW_TF32` | off | **1** (priorité 3) |
| `AXON_PARQUET_EMBEDDING_STORE_ENABLED` | off | **true** (priorité 4) |
| `AXON_CUDA_MEMORY_LIMIT_MB` | auto | 3000 si 2 workers MPS |
| `AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION` | unset | 1 si 2 workers (priorité 6) |
| `AXON_GPU_TOTAL_VRAM_MB_HINT` | détecté | 8192 |

## Annexe C — Méthodologie de l'audit

Cet audit est basé exclusivement sur la lecture du code source du dépôt Axon à la date indiquée. Aucun bench n'a été reproduit côté audit. Les estimations de gain sont basées sur :
- L'expertise empirique du domaine (BGE-Large + ONNX Runtime + CUDA en production)
- L'analyse des proportions de temps par stage dans le worker loop
- Les benchmarks publics sur configurations comparables (BGE-Large FP16 sur 8-12 GB VRAM)
- Les marqueurs documentés dans le code Axon lui-même (140 ch/s bench L1, cible 30 ch/s, stretch 200 ch/s)

Pour une qualification rigoureuse avant action, exploiter les traces `vector-lane.trace` déjà émises par le pipeline.
