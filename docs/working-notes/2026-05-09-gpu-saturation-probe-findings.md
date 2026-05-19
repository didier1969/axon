# GPU saturation probe — findings (proto/gpu-saturation-probe)

| | |
|---|---|
| **Date** | 2026-05-09 |
| **Statut** | Évidence empirique — à transformer en VAL/REQ SOLL |
| **Branche** | `proto/gpu-saturation-probe` (worktree `~/projects/axon-proto-gpu-saturation/`) |
| **Origine** | Hypothèse opérateur : "indexer plafonne à 30 ch/s à cause des accès disque ; GPU peut faire 200+" |

## TL;DR

**Hypothèse opérateur falsifiée.** Le GPU pur (sans DB, sans pipeline producer/consumer, juste l'embedder) plafonne à **~105 ch/s** à batch=128 sur cette config (BGE-Large + TensorRT + 8GB VRAM). Le pipeline producer/consumer ajoute ~10% d'overhead → **~95 ch/s**. La queue ne sature jamais (`queue_high_water=1` sur 64), donc 4 producteurs peuvent suivre.

**Conclusion** : le GPU/modèle est le ceiling, pas le disque. Les 30 ch/s actuels en prod indexer = ~30% du ceiling GPU pur. Il y a 3× de gain potentiel si on retire la DB, mais **pas 7× pour atteindre 200**. Pour 200+, il faut changer le modèle (plus petit), le batching (plus large + TensorRT recompile), ou le HW (GPU plus grand).

---

## Mesures

Toutes mesures : warmup 30-60s + steady-state 60s, BGE-Large via ONNX Runtime + TensorRT EP, GPU 8GB VRAM, sustained avec `--force-gpu`.

| Phase | Config | mean ch/s | rolling_10s_min | max ch/s | queue_high_water |
|---|---|---|---|---|---|
| **Prod indexer** (référence) | full pipeline + DuckDB + Parquet + FVQ | **~30** | n/a | n/a | n/a |
| Phase 1 GPU pur | batch=64 | 82.13 | 64.00 | 384 | n/a |
| Phase 1 GPU pur | batch=128 | **104.53** | 76.80 | 384 | n/a |
| Phase 1 GPU pur | batch=256 | (TensorRT compile stuck >44 min, killed) | | | |
| **Phase 2 pipelined** | producers=4, channel=64, batch=128 | **95.18** | 77.60 | 352 | **1/64** |

CSV bruts : `dev-bench-sustained-20260509T115634Z.csv`, `dev-bench-sustained-20260509T120000Z-b128.csv`, `dev-bench-pipeline-20260509T125130Z.csv` (au root du worktree).

## Lectures

1. **GPU ceiling ≈ 105 ch/s à batch=128**. Au-delà, batch=256 nécessite recompile TensorRT (>44 min sur cette machine, abandonné). Dans l'état actuel du engine cache, on ne peut pas dépasser ~105 ch/s sur un single-shot.

2. **Pipeline overhead = ~10%**. Phase 2 (95) vs Phase 1 (105) à batch=128 → channel + producer threads coûtent ~10 ch/s. Très minoritaire.

3. **Queue jamais saturée**. `queue_high_water=1` sur capacité 64. Confirme que la GPU consomme aussi vite (ou plus vite) que 4 producers combinés peuvent fournir. **Le bottleneck n'est PAS l'alimentation du GPU.**

4. **Pattern bursty** dans tous les samples per-second : 0/0/0/256/64/0/0/256… → micro-batch interne accumule plusieurs appels avant d'émettre un résultat GPU. Moyenne stable mais peu de granularité fine.

5. **Bottleneck en prod = pipeline d'écriture**. Prod 30 ch/s vs Phase 2 95 ch/s → le DB + FVQ + vector_ready update + Parquet archiver coûte **~70 ch/s (≈ 65% de la capacité)**.

## Implications pour REQ-AXO-193 direction E

L'hypothèse "in-RAM writer suffit pour atteindre 200 ch/s" est **partiellement validée et partiellement fausse** :
- ✅ **Vrai** : retirer le DB write side donnerait 30 → ~95 ch/s (3× gain)
- ❌ **Faux** : on n'atteindra pas 200 ch/s sur cette config sans changer modèle/HW

**REQ-AXO-193 direction E reste pertinent** : 3× de gain throughput est utile. Mais l'objectif chiffré 200 ch/s n'est pas réaliste avec BGE-Large + TensorRT + 8GB VRAM sur batch=128.

## Pistes pour aller au-delà de 105 ch/s

1. **Modèle plus petit** : BGE-base (384 dim) au lieu de Large (1024). Probable 2-3× speedup. Coût : qualité retrieval réduite.
2. **Batch=256 avec TensorRT engine pre-compiled** : prouver que la shape donne du gain. Engine compile = one-time cost (45+ min sur cette machine). Si gain ≥40%, paie pour les longues runs.
3. **2 workers parallèles** (déjà supporté par embedder-bench --workers) : 2 modèles en VRAM = 1.4GB, fit dans 8GB. Aggregate throughput théorique ~210 ch/s.
4. **FP16 / INT8 quantization** : réduction VRAM + speedup. Coût : qualité.

## Artifacts

- Worktree : `/home/dstadel/projects/axon-proto-gpu-saturation/` (branche `proto/gpu-saturation-probe`, basée sur `84baed8`)
- Binaires release : `embedder-bench` (extension `--sustained-secs`) + `axon-bench-pipeline` (nouveau)
- Code touché :
  - `src/axon-core/src/embedder.rs` : `+EmbeddingSustainedBench`, `+run_embedder_sustained_bench`, `+run_embedder_pipeline_bench`
  - `src/axon-core/src/bin/embedder-bench.rs` : `+--sustained-secs --warmup-secs --batch`, `+run_sustained()`
  - `src/axon-core/src/bin/axon-bench-pipeline.rs` : nouveau binaire
  - `src/axon-core/Cargo.toml` : `+[[bin]] axon-bench-pipeline`
- CSV : 3 fichiers `dev-bench-*.csv` au root du worktree
- Cargo target : 43 GB hardlinkés depuis main, isolés (write opérations en cargo créent de nouveaux inodes)

## Actions opérateur recommandées (à faire)

1. **Log VAL-AXO-XXX** : *GPU saturation probe — empirical ceiling at batch=128 is 105 ch/s, not 200*. Réfute partiellement l'hypothèse du DeepSeek note.
2. **Refines REQ-AXO-193** : direction E est pertinente (3× gain encore disponible), mais l'objectif 200 ch/s nécessite REQ séparé sur modèle/batching/HW.
3. **Backlog REQ-AXO-XXX** : *Investigate model alternatives* (BGE-base, FP16, multi-worker) si l'opérateur veut viser >150 ch/s.
4. **Cleanup éventuel** : worktree garde sa cargo-target hardlinkée (43GB metadata, ~6.7GB delta réel après build incrémental). Supprimer worktree = `git worktree remove ../axon-proto-gpu-saturation`.

## Décisions opérateur intervenues durant la session

- Plan **C puis B inconditionnel** : Phase 1 d'abord (extension embedder-bench), puis Phase 2 (worktree + binaire pipeline) quoi qu'il arrive.
- Critère de succès **strict** : mean ≥ 200 ET min(10s rolling) ≥ 150.
- Source data **a2 + b1** : 60k chunks pré-générés, single file × idx-prepended cycling.
- Architecture Phase 2 : 1 consumer GPU, 4 producers max-speed, sweep depth {16, 64, 1024} → simplifié à 1 cellule (queue_high_water=1 prouve que le sweep est inutile).
- **Build via `devenv shell`** obligatoire (linking Nix libstdc++ vs system libstdc++ → dlopen libonnxruntime fail). Reportable comme leçon transférable pour tout build worktree.
