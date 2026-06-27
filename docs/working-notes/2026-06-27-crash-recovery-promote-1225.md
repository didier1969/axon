# Récupération post-crash — promote 76e82e9c (1225) interrompu

**Date :** 2026-06-27 (session 91, crash PC mi-promote).

## État figé au crash
- **HEAD = `76e82e9c`** (working tree propre). Embarque :
  - REQ-AXO-902110 — fix P0 over-filtering HNSW (`hnsw.iterative_scan` dans `run_ann_query_json` partagé) + instrumentation `data.scope` de contradiction_check.
  - REQ-AXO-902108 (#8) — `[profile.release] incremental=true` (build 189s→~37s).
  - REQ-AXO-902109 (#9) — `promote_live.sh` in-place restart indexer inconditionnel + fail-fast.
  - REQ-AXO-902107 (d60249bb) — budget NLI + verdict inconclusive.
- **Live = DOWN** (aucun process axon-brain/indexer au crash).
- `current.json` = **ancien `d60249bb` (1222)** → promote NON finalisé.
- `bin/axon-brain` md5 `0b491d89` = **nouveau binaire 1225** déjà swappé. `bin/axon-indexer` md5 `2b74c08a`.
- Manifest candidat : `.axon/releases/candidates/0.8.0-v0.8.0-1225-g76e82e9c.json`.
- `pending.json` présent (build_id vide → probablement malformé par le crash → à effacer avant re-promote propre).

## Récupération (après PC + Axon MCP full)
1. Effacer le pending corrompu si malformé : `mv .axon/live-release/pending.json .axon/live-release/pending.aborted-crash-1225.json`.
2. Re-promote propre (HEAD propre, build incrémental ~37s) : `bash scripts/release/promote_live_safe.sh --project AXO`.
3. **Vérifier le fix P0** (le cœur) : `contradiction_check` candidat far-probe (ex FR « Axon stocke ses données dans MongoDB ») scope=AXO → doit donner **`contradicts`, `passages_judged>0`** (avant : `0 passage`). Vérifier `current.json` = `1225-g76e82e9c`.
4. Vérifier latence `retrieve_context` non régressée (iterative_scan borné max_scan_tuples=20000).

## Reste après promote vert
- Signaler Nexus (feedbacks #28-31 résolus par REQ-902110) : `mcp_feedback_report mark_resolved` ids 28/30/31, REQ delivered 902110.
- Follow-up : veto `n_flagged/n_judged` dans le texte de `retrieve_context_layered` (`apply_entailment_veto` l.799 → retourner compteurs ; résumé l.1037). Édit déjà spécifié.
- REQ-902111 (#11) reconciler control-plane T1 : design-twice tranché (DEC-AXO-901662), à implémenter.
