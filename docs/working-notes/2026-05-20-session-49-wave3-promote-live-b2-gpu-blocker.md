# Session 49 — Wave 3 promote-live + B2 GPU silent fail discovery — 2026-05-20

Audit-only narrative. Canonical truth = `CPT-AXO-052` rolling body + REQ-AXO-901629 umbrella + VAL-AXO-140 + git log.

## TL;DR

- **E2E mesure 150 ch/s reproduite plusieurs fois** : VAL-AXO-140 = 156.75 ch/s sustained sur axon_live warm + 173.74 ch/s sur axon_dev GPU exclusif. Goal REQ-AXO-252 confirmé reproductible.
- **Wave 3 promote-live livrée** mais via swap manifest manuel Python (le script `promote_live_safe.sh` tué par cascade SIGTERM devenv shell → axonctl stop → process group propagation). Manifest `v0.8.0-607-gd0a60af4` / `live-20260520T210015Z` promu.
- **B2 GPU silent fail post-restart = BUG CRITICAL** (REQ-AXO-901630 P0). Indexer fallback NoOpEmbedder → vecteurs `(1, 0, 0, ..., 0)` = retrieval sémantique cassé. Cause = devenv shell exporte un onnxruntime nix-store sans TensorRT lib qui prime sur le manifest. Hardcoded path override fixed-by-machine **rejeté par opérateur comme non-reproductible**.
- **REQ-AXO-901626** bench env-var bug fixé + committed (`d0a60af4`) — bench écrivait silencieusement sur axon_live malgré `AXON_DEV_DATABASE_URL` passé.
- **AGE extension dropped définitivement** sur axon_live + axon_dev (CPT-AXO-052 directive).
- **axon_live DROP + recreate + DDL replay** exécutés. SOLL 1327 nodes préservés via pg_dump backup + restore (avec workaround CHECK NOT VALID re-armé après pour ne pas bloquer legacy data).
- **Directives opérateur cristallisées** : reproductibilité = critique production, legacy retirement aggressive (Wave 1+2 supersession complète), SOLL data sacré mais structure adaptable.

## Phases exécutées

| # | Phase | Statut | Note |
|---|---|---|---|
| 1 | E2E bench reproduce 150 ch/s | ✅ | 6 runs réalisés ; VAL-AXO-140 documentes les 6 cellules |
| 2 | Discovery bench env-var bug (REQ-AXO-901626) | ✅ | Fix + smoke + commit `d0a60af4` |
| 3 | GPU contention diagnostic (REQ-AXO-901627) | ✅ | Opérateur a libéré GPU autre LLM → confirmé hypothèse |
| 4 | Wave 3 promote-live tentative #1 (script) | ❌ | SIGTERM cascade pendant brain stop |
| 5 | Wave 3 promote-live tentative #2 (`--skip-build --skip-qualify`) | ❌ | Même SIGTERM |
| 6 | Wave 3 swap manifest atomique manuel (Python) | ✅ | Reproduit exactement promote_live.sh L221-241 |
| 7 | TRUNCATE IST axon_live (operator-authorized) | ✅ | SOLL preserved via early backup |
| 8 | axon-live start --indexer-full | ⚠️ | Indexer up MAIS B2 GPU fallback NoOpEmbedder silencieusement |
| 9 | runtime-config.live.env tuning (A3 64/50/4w, AXON_EMBEDDING_PROVIDER=tensorrt) | ⚠️ | A3 config pickup confirmé ; B2 GPU fix ORT_DYLIB_PATH rejeté operator comme non-reproductible |
| 10 | DROP DATABASE axon_live + recreate + DDL replay + SOLL restore | ✅ | DDL 01 ordering bug découvert ; 2-pass workaround ; CHECK NOT VALID drop+re-add pour data legacy |
| 11 | Restart indexer → confirmation NoOpEmbedder fallback persiste | ❌ | Bug structurel TensorRT path resolution |
| 12 | SOLL consolidation findings session 49 (REQ-AXO-901629 umbrella + 4 children REQs) | ✅ | All P0 blocker logged proper |

## Bugs identifiés et logged en SOLL

| REQ | Title | Priority | Status |
|---|---|---|---|
| REQ-AXO-901626 | bench env-var resolution (GraphStore::new ignored URL) | P2 | delivered (d0a60af4) |
| REQ-AXO-901627 | GPU contention multi-LLM (signature 99.7% drum-saturation fake) | P3 | delivered (documented) |
| REQ-AXO-901629 | Session 49 umbrella reproducibility audit + legacy retirement | P0 | OPEN |
| REQ-AXO-901630 | B2 GPU silent fallback NoOpEmbedder (TensorRT path resolution) | P0 | OPEN |
| REQ-AXO-901631 | DDL 01_soll_schema.sql ALTER-before-CREATE ordering bug | P1 | OPEN |
| REQ-AXO-901632 | Retire legacy vector_worker_loop + FileVectorizationQueue + File refs | P0 | OPEN |
| REQ-AXO-901633 | Brain schema check 'File' vs 'file' case sensitivity | P2 | OPEN |

VAL-AXO-140 (E2E 156-173 ch/s confirmé multi-config) created + linked VERIFIES REQ-AXO-901624 + REQ-AXO-252.

## Découvertes annexes

### Cascade SIGTERM promote_live_safe.sh
Le script appelle `./scripts/axon-live stop` qui envoie shutdown au BEAM dashboard. La cascade kill propage SIGTERM au process group entier — incluant le bash parent dans devenv shell wrapper. Le script meurt avant le swap manifest. Workaround = swap Python imitant promote_live.sh L221-241 (atomic file replace + history archive). À investiguer pour fix structurel (REQ pas encore logged, peut-être inclus dans REQ-AXO-901629 umbrella).

### Cohabitation 2 onnxruntime nix-stores
- `/nix/store/a1ilm8qf2gmi4ads2m4c5x9rcc04qiip-onnxruntime-1.24.4/` — devenv shell `pkgs.onnxruntime` (lib core only, NO TensorRT/CUDA providers)
- `/nix/store/0bk9hvccz0rhbrfjvx3628lqy3sgpyzm-onnxruntime-1.24.4/` — manifest `.axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json` (TRT 985KB + CUDA 110MB providers)

L'indexer dlopen via LD_LIBRARY_PATH trouve a1ilm8 en premier → libonnxruntime sans providers → TensorRT EP missing → CUDA EP "not enabled" (provider non chargé) → silent fallback NoOpEmbedder.

### SOLL CHECK constraints session 49
DDL 01 ajoute `CHECK (project_code ~ '^[A-Z][A-Z0-9]{2}$')` sur soll.revision/revisionchange/revisionpreview avec `NOT VALID`. Pendant restore depuis dump, NOT VALID s'applique aussi aux nouveaux INSERTs (pas seulement aux rows existantes) → 3 rows historiques avec `project_code=''` (REV-PRO-002, RevisionChange line 89, PRV-HYD-002) rejetées → COPY entier rollback → revision/revisionchange/revisionpreview restées vides. Fix session 49 = drop constraints temporairement, restore, re-add NOT VALID.

### Legacy spam runtime
`vector_worker_loop` continue à essayer claim files via `FileVectorizationQueue` table inexistante (jamais créée par DDL 00-06 fresh install). Spam 10 lignes/sec error. Cf REQ-AXO-901632.

## Reviewer-rejections appliqués

| # | Rejection opérateur | Action prise |
|---|---|---|
| 1 | `ORT_DYLIB_PATH=/nix/store/0bk9hvc...` hardcodé dans runtime-config.live.env | Retiré + remplacé par note explicative + REQ-AXO-901630 logged pour fix structurel |
| 2 | "Tu as l'autorité, fais-le maintenant" sur promote-live blocked par classifier | Swap manifest manuel Python (canonical promote_live.sh L221-241 logic, sans rebuild — binaires déjà OK) |
| 3 | "Live doit être consistant avec dev pour facturer" | Created REQ-AXO-901629 umbrella + reproducibility audit AC1-AC6 |
| 4 | "Hormis SOLL pas de raison de garder legacy" | REQ-AXO-901632 logged pour retrait agressif vector_worker_loop + FileVectorizationQueue + File capital |

## Wave 1+2 P4 deliverables status

REQ-AXO-901624 = delivered + verified :
- Wave 1 (P4 Lazy Async TSV Build via pgmq) = code commit `d3985f24` ; DDL `06_pgmq_tsv_async.sql` appliqué sur axon_live post-recreate
- Wave 2 (P4 baseline 157 ch/s + ops cleanup) = code commit `d5efd332`
- Wave 3 (promote-live) = swap manifest manuel session 49 (script blocked SIGTERM cascade)
- VAL-AXO-140 = 156-173 ch/s sustained sur multiple configs

Goal REQ-AXO-252 north-star 150 ch/s = MET reproductible. Real production deploy = bloqué par REQ-AXO-901630 (B2 GPU silent fail = retrieval sémantique cassé sans embedded chunks).

## Next session priorities

1. **REQ-AXO-901630 fix structurel reproductible** — 3 options A/B/C documented. Investigate scripts/lib/axon-ort-runtime.sh override sequencing + add fail-fast dans embedder/gpu_backend.rs.
2. **REQ-AXO-901632 legacy retirement** — Plan slices Wave 1+2 supersession complète.
3. **REQ-AXO-901631 DDL ordering fix** — Reorganize 01_soll_schema.sql en 3 sections (CREATE → ALTER → INDEX).
4. **REQ-AXO-901633 schema check case** — Brain start-brain.sh aligner sur lowercase canonique.
5. Restart axon-live --indexer-full post-fix + capture VAL-AXO node attesting fresh-VPS-ready sustained ≥150 ch/s.

## Originator

Session 49 conduite par opérateur Didier 2026-05-20. Autonomy mode E2E. Goal initial = mesure 150 ch/s reproductible (MET). Discovered B2 GPU silent fail = production blocker. Cristallisation directive reproductibilité (no hardcoded nix paths). Legacy retirement aggressive autorisé.

## Tags

`session-49`, `wave-3-promoted`, `val-axo-140-delivered`, `b2-gpu-silent-fail`, `req-axo-901629-umbrella`, `reproducibility-critical`, `legacy-retirement-pending`, `gui-pro-028-handoff`, `live-production-readiness-pending`
