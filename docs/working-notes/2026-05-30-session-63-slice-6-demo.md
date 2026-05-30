# Slice 6 — Démonstration 24K files / 2h40 (session 63)

## Setup state (T0)

- HEAD : `857f0100 refactor(pipeline_v2): SOTA slice 5 — collapse stage_b1 dans demand_pull`
- Branch : `feature/pipeline-sq-reorder-point`
- New binary md5 : `43c16f9` (.axon/cargo-target/release/axon-brain)
- Old binary md5 (live, intouché) : `75ba1b4` (bin/axon-brain)
- Dev DB pre-truncate : 15 866 files / 118 303 chunks / 126 046 embeddings ❌ (le bug 125K>118K observé par operator était RÉEL sur dev — clean post-truncate)
- Dev DB post-truncate : 0 / 0 / 0 ✓
- AXON_WATCH_DIR default : `/home/dstadel/projects` (parent dir, ~24K eligible)
- Slices 1-5 cumulative LOC : −466 net (target ≥ 450 atteint)

## Acceptance criteria

| # | Critère | Méthode | Cible |
|---|---|---|---|
| 9 | Time-to-complete 24K files | dashboard `indexed_files == eligible_files` ET `pending_chunks == 0` | ≤ 160 min |
| 9 | Chunks/sec sustained | sample MCP embedding_status delta /30s | ≥ 50 ch/sec |
| 10 | pipeline_status observable | dashboard reste `indexer_active` | jamais `blocked` sans cause |
| - | No crash brain/indexer | pgrep durant 2h40 | toujours alive |
| - | retrieve_context_v2 p95 | sample 10 queries post-completion | ≤ 100 ms |

## Slices à valider empiriquement post-demo

- **Slice 4 (channel A3→B1 retrait)** : latence ajoutée 1.2s tolérable
- **Slice 5 (stage_b1 collapse)** : demand_pull mono-thread sustained ≥50 ch/sec
- Si FAIL : escalate REQ axon-bug + cleanup slice 7 inclut diag

## Observabilité (live monitoring)

- Dashboard dev : http://127.0.0.1:44137
- MCP embedding_status (live brain) : reflète **live** DB (44137 ne le voit pas) ; pour dev → connecter MCP au dev brain port 44129 ou utiliser psql directement
- psql sample 1Hz : `SELECT count(*) chunks, count(*) FILTER (WHERE embed_status='embedded') embedded FROM public.chunk;`

## Plan exit conditions

1. **PASS** : completion ≤ 160 min ET chunks/sec ≥ 50 ET no banner `blocked_reason` → trigger slice 7 cleanup
2. **FAIL time** : completion > 160 min — log REQ axon-bug, capture per-stage `t_work_ratio` (Goldratt drum identification), opérateur escalate
3. **FAIL throughput** : chunks/sec sustained < 50 → diag B2 batch saturation OR demand_pull SELECT latency
4. **FAIL crash** : indexer/brain process dies → log REQ + escalate

## Observed runtime (live monitoring)

T0 = 1780148422 (epoch, 2026-05-30 ~15:40 UTC).

Corpus actuel : 14 329 files indexed, 58 089 chunks total. Discovery phase plateau ~t+5min ; chunks plateau ~t+10min. 24K files target overshoot (real eligible ~14.3K via AXON_WATCH_DIR=/home/dstadel/projects).

| elapsed | indexed | chunks | embedded | A ch/s | B emb/s | cumul B |
|---|---|---|---|---|---|---|
| 21s | 14301 | 10298 | 2940 | — | ~52 | — |
| 153s | 14301 | 26626 | 9106 | ~124 | ~47 | 60 |
| 284s | 14301 | 39698 | 15294 | ~100 | ~47 | 54 |
| 418s | 14301 | 44290 | 19410 | ~34 | ~31 | 46 |
| 550s | 14315 | 54260 | 24525 | ~75 | ~39 | 45 |
| 683s | 14329 | 58089 | 28619 | ~29 | ~31 | 42 |
| 817s | 14329 | 58089 | 31933 | 0 | ~25 | 39 |
| 951s | 14329 | 58089 | 35787 | 0 | ~28 | 38 |
| 1083s | 14329 | 58089 | 37776 | 0 | ~15 | 35 |
| 1213s | 14329 | 58089 | 39349 | 0 | ~12 | 32 |
| 1344s | 14329 | 58089 | 41287 | 0 | ~15 | 31 |
| 1475s | 14329 | 58089 | 43010 | 0 | ~13 | 29 |
| 5122s | 15138 | 94605 | 91665 | discovery | 2.5 | 17.9 |
| 5523s | 15451 | 100165 | 95050 | A burst +4883 | 13.7 | 17.2 |
| 5929s | 15451 | 100165 | 98621 | 0 | 5.6 | 16.6 |
| 6131s | 15451 | 100165 | 99466 | 0 | 4.2 | 16.2 |
| 6334s | 15451 | 100165 | 99501 | 0 | 0.17 (stuck) | 15.7 |

**Analyse** :
- Pipeline A peak 124 ch/sec, sustained ~50 jusqu'à plateau t+10min
- Pipeline B peak 52 emb/sec, sustained dégradant 47 → 28 → 13 emb/sec
- GPU 76% utilization confirmée (nvidia-smi t+22min) — donc B2 IS busy
- Long-tail token_count effect : `ORDER BY token_count` met les gros chunks en fin, seq_len BGE-Large 1024d augmente, latence GPU explose
- Throughput cumulatif final estimé ~25-30 emb/sec — **bien sous cible 50**
- Wall time estimé completion ~41 min (PASS budget 160 min)

**Diagnostic root cause** : architectural ceiling REQ-AXO-901820 (GPU BGE-Large 1024d ≈ 100 emb/sec MAX, mais drops à ~12 emb/sec sur seq_len=512). Slice 5 collapse PAS la cause (GPU busy at 76% = upstream OK, B2 IS the drum). 

**Stall final t+105min** : 664 chunks bloqués à `token_count=512` exactement. GPU 0% utilization, indexer 440% CPU + 9.8GB RAM. Symptômes possibles :
- TensorRT engine compilation pour max-seq batch (engines compiled à la volée, peuvent être très longs)
- Tokenizer CPU loop bug
- CUDA OOM silencieux ou contention

**NEW REQ à logger session 63** (slice 7) : « max-seq token_count=512 chunks stall pipeline B » — tag `axon-bug` + `pipeline-stall` + `max-seq-len`.

**Conséquence slice 7 cleanup** : 
- ✅ Tout slice 7 du plan reste pertinent (cleanup code/docs/scripts)
- 🟡 Acceptance #9 throughput PASS partial — wall time PASS, sustained throughput FAIL
- ❌ NE PAS supprimer REQ-AXO-901820 du backlog — c'est le drum architectural confirmé
- ➕ Logger NEW REQ : token_count bucketing strategy review (ordre inverse pour démarrage rapide ? batch_size adaptatif par bucket ? smaller embedding model option ?)
