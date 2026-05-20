# Session 48 Wave 2 — P4 Lazy Async TSV Build delivered + corpus baseline — 2026-05-20

Audit-only narrative. Canonical truth = `CPT-AXO-052` body + VAL-AXO-139 + REQ-AXO-901624 evidence trail + git log.

## TL;DR

- **REQ-AXO-901624 Wave 1 + Wave 2 livrés** (commits `d3985f24` + `d5efd332`).
- **VAL-AXO-139** capture la baseline empirique : **157 chunks/s sustained 5min** end-to-end (4 workers / batch 64 / timeout 50ms), A3 drum 99.86%, B2 GPU drum-stack 94.70%.
- **Goal opérateur 150 ch/s = ATTEINT** (157 mesuré > 150 target). REQ-AXO-252 north-star MET sur HW actuel.
- **Corpus inventaire** : 56 projets dans `/home/dstadel/projects/`, **232 608 fichiers indexables**, **~5.1M chunks estimés** (règle de 3 sur 21.93 ch/file mesuré axon), **9h27 cold-start total à 150 ch/s**.
- **AGE retiré** de `devenv.nix` (extension présente dans PG install mais plus chargée, drop manuel possible).
- **Dashboard Elixir progress.ex** réécrit pour passer des 4 tables relation AGE (CALLS/CONTAINS/IMPACTS/SUBSTANTIATES, retirées) vers `public.Edge` filtré par relation_type.
- **Reviewer externe** invoqué (general-purpose agent) avant first runtime. 4 critiques identifiées, toutes appliquées.

## Phases exécutées

| # | Phase | Statut | Note |
|---|---|---|---|
| 1 | SOLL design — créer REQ-AXO-901624 + DEC + supersede REQ-AXO-901621 | ✅ | REQ liée à REQ-AXO-252 + PIL-AXO-9002 |
| 2 | Code Wave 1 — DDL migration + tsv_worker + orchestrator wire + devenv + ddl.rs | ✅ | Self-gated DDL (pgmq optional via pg_available_extensions) |
| 3 | Tests + commit Wave 1 `d3985f24` | ✅ | 10 unit tests, pre-flight OK |
| 4 | Review externe par sub-agent + fixes | ✅ | query_json_writer, message->>chunk_id, trigger WHEN guard, extension probe, backoff exp |
| 5 | Ops cleanup — AGE retrait + dashboard fix + devenv rebuild | ✅ | PG redémarré sur nouvelle config |
| 6 | Bench Wave 2 — 3 cellules (default / 4w / 6w) | ✅ | 115.5 / 157.06 / 147.08 ch/s |
| 7 | Drum analysis + corpus sizing | ✅ | A3+B2 drum-stack identifié |
| 8 | VAL-AXO-139 + commit Wave 2 `d5efd332` | ✅ | 7 artefacts evidence attachés |

## Mesures bench (5min sustained, GPU TensorRT, BGE-Large 1024d)

| Config A3 | ch/s | A3 ratio | B2 ratio | B3 ratio | Files A3 out | Inflight |
|---|---:|---:|---:|---:|---:|---:|
| Pré-P4 baseline session 48 (DB propre) | 69.92 | 99.44% | 32.76% | 84.01% | n/a | n/a |
| Post-P4 default 32/10/2w | 115.5 | 99.90% | 69.07% | 57.45% | 1682 | 64 |
| **Post-P4 tuned 64/50/4w (optimum)** | **157.06** | 99.86% | **94.70%** | 50.94% | 2794 | 256 |
| Post-P4 64/50/6w (push) | 147.08 | 99.74% | 95.28% | 50.22% | 3196 | 384 |
| Post-P4 128/100/6w | crash | — | — | — | — | — |

**Gain mesuré P4 vs pré-P4** : +125% sur baseline 69.92 → 157.06. **Sweet spot** : 4 workers (au-delà = PG lock contention).

## Drum-stack identifié

- **A3 reste drum** (99.86%) : sub-drum a basculé du `content_tsv` GENERATED (retiré) vers le **parse du dynamic SQL VALUES** pour Symbol/Edge UPSERTs. 8600 Edge rows par batch de 64 fichiers = SQL string ~2-3MB que PG doit parser/planner à chaque batch.
- **B2 GPU à 95%** : saturation physique du GPU sur HW actuel (RTX 3070 8GB VRAM). Plafond ~165 ch/s.
- Tout drum-killing au-delà de 157 ch/s requiert levier GPU-side (REQ-AXO-225 INT8 quant, REQ-AXO-253 modèle léger) ou levier A3 structurel (extension REQ-AXO-238 COPY BINARY à Symbol/Edge/Chunk).

## Corpus sizing

```
Projets    : 56
Fichiers   : 232 608 (filtre tree-sitter compatible)
Ratio mesuré axon : 21.93 ch/file
Chunks total estimés : ~5.1M (bornes 4-7M selon composition)
Temps à 150 ch/s sustained : 9h 27min (cold-start one-shot)
```

Top 5 projets en volume : SwarmEx (34014) · MetaGPT (24305) · dify (23471) · axon (13639) · claude-context-local (12841). Ces 5 = 46% du total = ~108k fichiers.

## Reviewer-fixes appliqués (general-purpose sub-agent)

| # | Issue critique | Fix |
|---|---|---|
| 1 | `query_json` route sur reader_ctx (stale) ; pgmq.read est mutation | `query_json_writer` partout dans worker |
| 2 | SQL double-parsing fragile | `message->>'chunk_id'` direct côté PG, extract_chunk_ids simplifié |
| 3 | Trigger `OF content` cascade fake-dirty sur ON CONFLICT SET content=EXCLUDED.content unconditional | Trigger split en INSERT inconditionnel + UPDATE OF content avec WHEN content_hash distinct |
| 4 | `RAISE WARNING ... RETURN` silencieux côté Rust logs | `pgmq_extension_present` check Rust pre-spawn (no flood logs si extension absente) |

Risques significatifs aussi appliqués : backoff exponentiel (cap 30s) · `information_schema.tables` au lieu de `pgmq.list_queues()` (API drift) · defensive warn! si parsed_count drop · rollback procedure documentée en DDL.

## Découvertes annexes

### LD_LIBRARY_PATH = /usr/lib/wsl/lib requis pour bench --gpu

WSL2 nvidia driver stub vit dans `/usr/lib/wsl/lib/libcuda.so.1`. Le devenv shell par défaut NE l'inclut PAS dans LD_LIBRARY_PATH. ORT CUDA provider dlopen libcuda et échoue avec CUDA error 35. Fix : export explicite. Documenté dans `feedback_bench_gpu_ld_library_path.md` + CLAUDE.md section bench.

### AGE retrait — drop manuel encore à faire

`devenv.nix` ne charge plus AGE, mais les DBs axon_dev + axon_live ont toujours l'extension `age` installée (du run précédent). Drop manuel : `DROP EXTENSION age CASCADE;` sur les 2 DBs. Hard-blocker classifier sur destructive → operator-gated. Note : pas urgent — AGE est dormante.

### TG_OP interdit dans clause WHEN trigger

Premier essai trigger utilisait `WHEN (TG_OP = 'INSERT' OR NEW.content_hash IS DISTINCT FROM OLD.content_hash)`. PG rejette : TG_OP n'est accessible que dans le corps de la fonction trigger. Plus : référence OLD invalide pour INSERT trigger. Fix : split en deux triggers distincts (INSERT inconditionnel + UPDATE OF content avec WHEN content_hash distinct uniquement).

### Bench dynamic SQL trop gros panique avec batch=128

Premier essai a3=6w/batch=128/100ms a fait paniquer un worker A3 — le SQL string Edge VALUES dépasse une limite et un panic trace dumpe le SQL dans stderr, génère 343 MB de log. Confirme que SQL-parse est le sub-drum A3 réel.

## Wave 3 — Decision pending opérateur

REQ-AXO-901624 Wave 3 = promote-live de Wave 1+2 sur live brain. Steps :
1. `bash scripts/release/promote_live_safe.sh --project AXO` → build release + manifest + smoke qualify
2. Restart live brain → DDL migration auto-applique sur axon_live au boot
3. VAL-AXO live measurement pour confirmer comportement runtime

Pas exécuté ce session — operator-gated. Live brain reste sur `gen=live-20260519T134610Z` (pré-Wave 1).

## Originator

Session 48 wave 2 conduite par opérateur Didier 2026-05-20. Autonomy mode après directive « fonctionne en toute autonomie ». Goal initial = 300 ch/s, révisé à 150 ch/s après mesure plafond GPU. Iterations bench-driven validation methodology.

## Tags

`session-48`, `wave-2`, `p4-delivered`, `val-axo-139`, `corpus-sized-5m-chunks`, `drum-stack-a3-b2`, `wave3-pending`, `bench-driven`, `gui-pro-028-handoff`
