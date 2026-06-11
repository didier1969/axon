# Session 74 — 2026-06-11 — Backup bloque le boot, fix + promote propre + nettoyage disque

Audit-only narrative (GUI-PRO-028 step 5). Canonique = `CPT-AXO-052` (session_pointer) + git + SOLL `GUI-AXO-1025`.

## Contexte
Reprise (`resume`). Live ET dev étaient DOWN → tunnel MCP Claude `axon` injoignable (il vise le socket/HTTP live). Démarrage live figé ~7 min.

## RCA — pourquoi le boot live se figeait
Le boot s'est bloqué sur un `psql` appliquant `db/ddl/02_axon_runtime.sql`. Investigation des verrous PG :
- pid 9140 (DDL boot) `ALTER TABLE axon_runtime.EmbedderLifecycleHeartbeat ADD COLUMN` attendait un `AccessExclusiveLock`.
- Bloqué par pid 8815 = `pg_dump` (application_name) détenant un `AccessShareLock` sur **toutes** les tables.
- Chaîne : `backup_soll_daily.sh` (pid 8777) → `pg_dump` (8813) → `gzip -9` (8814, état R). pg_dump bloqué en `pipe_write` car gzip -9 (CPU-bound) ne suit pas. socket : 4,1 Mo en send-Q PG, 2,9 Mo en recv-Q pg_dump. **Pas wedge — backpressure normale d'un dump 12 Go.**

### Cause racine
`backup_soll_daily.sh` est appelé par le **hook devenv enterShell** (`.devenv/shell-*.sh`). Il dumpait le DB axon_live **ENTIER** (12 Go : `ist.chunk` 7,5 Go + `ist.chunkembedding` 3,8 Go — reconstructibles) via `gzip -9`. Le dump n'aboutissait jamais dans la fenêtre entre redémarrages du brain → le `mv` final + l'écriture du marqueur `daily` (ligne 112) ne s'exécutaient jamais → **chaque `enterShell` re-déclenchait un dump complet**. Résultat : empilement de partiales multi-Go (~11 Go) + `AccessShareLock` ~10 min sérialisant la DDL de boot.

## Fix (GUI-AXO-1025, commit 53199681)
Scoper le dump à `--schema=soll --schema=axon_runtime --schema=axon` (intentions irremplaçables ~10 Mo) ; `ist` + `pgmq` reconstructibles, exclus. Mesuré : **0,56 s / 1,3 Mo**, marqueur posé, plus de re-trigger ni de verrou bloquant. Validation directe : live a ensuite booté proprement (`Axon ready`, brain `:44129/readyz`=ready).

Tailles par schéma (axon_live) : ist 12 Go · pgmq 334 Mo · soll 7 Mo · axon_runtime 2,6 Mo · axon 8 Ko.

## Promote propre (retest + promotion demandés)
`promote_live_safe.sh --project AXO` :
- 1er essai → ❌ step 3 preflight « tracked git state is dirty » (fix backup staged non commité).
- Commit via `axon_commit_work` en HTTP/curl (tunnel Claude down) → 53199681.
- 2e essai → step1 build · step2b dev-gate ✅ (`build_id v0.8.0-919-g53199681 == HEAD`) · step3 preflight ✅ · step4 manifest · step5 promote_copy_restart · step6 qualify-mcp ✅ · step7 finalize. **PROMOTE COMPLETE**, live HEALTHY, MCP 67 outils, dashboard 44127 HTTP 200.
- Les REQ-901930/931/945/946 (idle-reclamation, commit 6084f502) + 36 commits sont désormais EN LIVE → clôt les items 1-3 du pointer précédent côté code+promote.

## Nettoyage disque
- 11 Go de `*.partial` orphelines supprimées.
- 26 vieux dumps full-DB (32 Go) élagués (>50 Mo). Conservés : snapshots SOLL `soll.db.*` + dump scopé du jour. `~/backups/soll` 33G → 89M.

## Fin de session
Tout arrêté proprement (live + dev) sur demande opérateur. Brain rallumé brièvement pour ce handoff puis re-stoppé.

## Technique notable
Tunnel MCP Claude `axon` ne se reconnecte pas si live était down à l'init. Contournement : piloter le brain en HTTP curl sur `:44129/mcp` (JSON-RPC stateless, réponses SSE `data:`). `pre_flight_check`, `commit_work`, `document_intent`, `soll_manager`, `sql`, `soll_validate` tous exécutés ainsi. Voir memory `reference_mcp_via_http_curl_when_tunnel_down`.

## Ouvert
1. Vérifier en runtime l'idle-reclamation (preuve manquante).
2. Pousser la branche + décider merge→main (37 commits devant main).
3. SOLL : 22 violations pré-existantes (REQ 901893-901899 Watchman sans criteria, DEC-901629).
