# Session 27 — Pipeline B perf arc (DEC-AXO-086 follow-up shipped end-to-end)

**Date** : 2026-05-13 / 2026-05-14
**Build delivered** : `v0.8.0-422-gb511e7a7`
**Canonical session_pointer** : `CPT-AXO-052`

## Arc shipped

10 commits, all push origin/main, all promoted live :

| Commit | Theme | SOLL |
|---|---|---|
| `fc916375` | `embedding_status` MCP verb (slice 2 minimum) | DEC-AXO-090 |
| `0661c4e0` | REQ-AXO-314 cascade kill (output_rx drain) | REQ-AXO-314 |
| `56af8f43` | B1 batched_worker (architectural) | REQ-AXO-314 |
| `8d568369` | `AXON_B1_COLDSTART_BATCH_SIZE` dead env knob | REQ-AXO-318 |
| `94d084ac` | Dev/live DB URL isolation (2 code paths) | REQ-AXO-315 |
| `8b2efbf3` | Length-proxy bucket-batching + verb enriched | DEC-AXO-090 |
| `7f5c938d` | `token_count INTEGER` column + plumbing | DEC-AXO-089 |
| `0c8ed7e5` | Option-b bucket-then-batch (token-sorted fetch + pool 4×) | DEC-AXO-089 |
| `07f0e4bf` | B3 batch_size 64→256 + timeout 10→200ms | REQ-AXO-316 |
| `b511e7a7` | Verb fallbacks read channels.rs constants | — |

Plus uncommitted (this handoff) :
- `scripts/lib/start-indexer.sh` + `start-split.sh` : default `indexer_full --tensorrt` for live (DEC-AXO-088)

## Bench A/B (dev, axon_dev, full /home/dstadel/projects scope)

| Phase | Rate B-side | GPU SM avg | Note |
|---|---|---|---|
| Pre-bucket (mixed-length batch=64) | ~30 emb/s | bursty 27-97% | baseline |
| Length-proxy bucket-sort | 47 emb/s sustained | 64% | × 1.57 |
| Token_count exact (single-axis) | ~37 emb/s | (mesure foireuse, B rattrapé A) | inconclusive |
| **Option-b (bucket-then-batch + 4× pool + B3 fix)** | **~108 emb/s burst, 50 emb/s sustained** | **64% avg, 97% peak** | **× 1.7-2.4** vs pre |

GPU 1-3% point-snapshots étaient des artefacts (samples 1s tombés entre bursts). `nvidia-smi dmon -s u -c 30 -d 1` window averaging révèle 64% SM sustained — gold standard pour cette mesure.

## Bugs discovered & fixed in flight

1. **REQ-AXO-314 cascade kill** : `_handles_b` dropped → B3 output_rx closed → B3 returns on 1st `tx.send(receipt)` → drops `b2_to_b3_rx` → B2 exits → drops `b1_to_b2_rx` → B1 workers all exit → `b1_inbox_rx` drops → NOTIFY listener + cold-start poll fail with "b1_inbox closed". Repro signature : exactly 1 batch embedded post-boot, then plateau.

2. **REQ-AXO-315 DB URL leak** : dev indexer (instance_kind=dev) writing to axon_live. 2 code paths (`bulk_writer.rs::resolve_database_url` + `graph_bootstrap.rs::resolve_pg_database_url`) tested `AXON_LIVE_DATABASE_URL` before `AXON_DEV_DATABASE_URL` without checking `AXON_INSTANCE_KIND`. Repro signature : axon_dev stays at 0 rows while axon_live grows +28k embeddings under dev-mode indexer over 30 min.

3. **REQ-AXO-316 B3 dead-batching** : `B3_BATCH_TIMEOUT_MS_DEFAULT=10` copy-paste from A3 (where 10 ms is operator-mandated for FTS latency). At realistic B2 arrival rates 100-300/s, B3 tick fires every 10 ms with ~1 chunk in buffer → flushed batch_size=1 → effective B3 was single-row HNSW UPSERT despite `batch_size=64` param.

4. **REQ-AXO-318 cold-start dead env knob** : `pipeline_v2_runtime.rs:46` hardcoded `B1_COLDSTART_BATCH_SIZE: 256` instead of reading `caps.b1_coldstart_batch_size`. Documented env override `AXON_B1_COLDSTART_BATCH_SIZE` was therefore ignored.

## Architectural decisions

- **DEC-AXO-088** : live indexer default = `indexer_full --tensorrt` (operator directive 2026-05-14). Was `indexer_graph` (CPU only) → every promote_live restart broke embedding until manual `--indexer-full`.
- **DEC-AXO-089** : bucket-then-batch pipeline B (option b) — canonical HF/Triton pattern. token_count column + ORDER BY in cold-start + ORDER BY in fetch + pool 4× B2.
- **DEC-AXO-090** : `embedding_status` MCP verb = canonical operator snapshot (Storage table + Pipeline A/B config). Single entrypoint replacing fragmented `status` / `debug` / `health` views.

## Operator interactions worth noting

- **GPU util misreading** : operator caught me using point-snapshot `nvidia-smi --query-gpu=utilization.gpu` which samples ~1s and lands between bursts → false low readings. `nvidia-smi dmon -s u -c 30 -d 1` window averaging is the right tool.
- **Dev/live isolation** : operator's "Je pense que l'indexeur live n'est pas arrêté" diagnostic was the entrypoint to REQ-AXO-315. Reality : live indexer WAS stopped, but dev indexer was writing to axon_live.
- **No scope reduction for dev** : operator explicit "Je ne veux pas que tu réduises le scope" — dev workload must mirror prod, no AXON_WATCH_DIR shortcuts. Memory saved as `feedback_no_scope_reduction_for_dev.md`.
- **Token-budget vs bucket-batch** : operator asked theoretical question about batching strategy. Documented in DEC-AXO-089 : option b (bucket-then-batch) wins over option a (pure token-budget continuous batching) for encoder embedding workload (seq 11-512, already-clustered distribution). vLLM-style continuous batching better-fit for LLM decoding.

## Post-session state

- Live brain `v0.8.0-422-gb511e7a7` running, MCP up on 44129
- Live indexer in `indexer_full --tensorrt`, IST wipé 23:33 + re-bootstrap en cours (~29k chunks / 19k embs at handoff time, growing)
- Coverage will reach steady-state at ~150k chunks / proportional embs once bootstrap completes
- SOLL : 920+ nodes, 0 violations after cleanup (DEC-AXO-087 and REQ-AXO-317 archived as dup-superseded)

## Next session — concrete first actions

1. **Commit scripts/lib/start-indexer.sh + start-split.sh** (DEC-AXO-088 plumbing, uncommitted at handoff)
2. **Validate option-b gain on live** : `mcp__axon__embedding_status` snapshot after bootstrap settles, compare embed rate vs ~30 emb/s baseline pre-fixes
3. **(operator-gated)** drop HNSW index on `public.ChunkEmbedding` pendant bulk drain + recreate → gain potentiel × 5-10 sur B3 si embedding throughput devient le plafond
4. **bench-pipeline-v2 --noop hang** : debug pourquoi le bench hung sur init graph store même en `--noop` mode (separate REQ if not already logged)
