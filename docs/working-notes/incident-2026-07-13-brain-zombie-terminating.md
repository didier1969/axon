# Incident live 2026-07-13 — brain coincé `Terminating` → zombie → panne totale

**Evidence live pour REQ-AXO-902233 (graceful shutdown / cutover quasi-zéro-downtime).**

## Symptôme
`promote_status` → `-32000 Backend unavailable` sur `http://127.0.0.1:44129/mcp`.
Vérité-sol : `axon-brain` = **zombie** (`ps STAT=Zl`, defunct), PPID=346 (subreaper, PAS le superviseur),
process-compose (`:8080`) montrait `axon-brain status=Terminating restarts=1 exit=-1`.
`:44129` non-LISTEN. Clients à -32000.

## Chaîne causale (root cause)
1. Brain servait bien sur `v0.8.0-1382-g069382a4` jusqu'à ~20:27 (derniers logs = warnings
   `dashboard_state ... lock timeout [55P03]`, non-fatals — bug connu REQ-AXO-902216).
2. process-compose a déclenché un **restart** du brain (`restarts=1`). Déclencheur exact NON confirmé
   (probable : readiness/liveness-probe flappée sous la lenteur lock-timeout, seuil `failure_threshold`).
3. **Point de bascule = pas d'arrêt propre** : `main_services.rs` `axum::serve(listener, app)` **sans**
   `.with_graceful_shutdown` et **aucun handler SIGTERM**. Au SIGTERM du superviseur, le brain ne meurt
   pas proprement → reste **`Terminating`** → **zombie** reparenté au subreaper 346, jamais reap.
4. Vu de process-compose, l'ancienne instance ne finit jamais de mourir → **aucun brain neuf relancé**
   → `:44129` mort durablement (pas un trou de quelques secondes : panne jusqu'à intervention manuelle).
5. Clients → -32000.

**Root cause = absence de graceful shutdown / SIGTERM handler sur le brain.** Un restart de routine
s'est transformé en panne totale. MÊME root-cause que l'échec de promote (`/mcp` wedgé 300 s) : sans
arrêt propre, tout restart est fragile.

## Lien avec le RCA du promote (même jour, 19:49–20:04)
`promote-20260713T194933Z.log` : le cutover full-stop+start a lié `:44129` + passé `/readyz` en **7,3 s**
(`[timing] brain launch→readyz: 7295ms`) MAIS le post-check MCP `initialize` (Content-Type correct) a
**timeout 300 s** (`TimeoutError` dans `getresponse`/`_read_status`) → `/mcp` wedgé alors que `/readyz`
était vert. ⇒ **`/readyz` MENT** (trivial `SELECT 1` via `spawn_blocking`, ne prouve pas le service MCP).
Nuance : ce hang 300 s était en **indexer_full** (build TensorRT + warm complet en contention) ; le
self-heal `brain_only` de cet incident sert `/mcp` en 0,4 s (`initialize`) / 0,018 s (`status`). Le
« up-but-slow » est vraisemblablement lié à la charge de boot indexer_full, pas au warm brain fondamental.

## Recovery appliquée
1. `stop --hard` (reap superviseur wedgé + orphelins). Le zombie ne tenait AUCUNE ressource
   (flock SOLL + port libérés à la mort du process).
2. Le self-heal client (`ensure-axon-running.sh`, déclenché par un client tapant le port mort) avait
   déjà relancé le brain sur 1382 — mais en **`brain_only`** (indexer `Disabled`), d'où l'indexer manquant.
3. Restauration chirurgicale de l'indexer : `POST :8080/process/start/axon-indexer` (process séparé,
   flock IST distinct du brain → **sans toucher le brain qui sert**). `promote_status` → **`clean`**.

## Fix (dérive vers 902233)
- **n°1 no-regret : graceful shutdown** (handler SIGTERM + `axum::serve(...).with_graceful_shutdown(...)`
  qui draine les requêtes en vol). Aurait empêché la panne (arrêt propre → reap → relance normale).
- **health-gate sur un vrai appel MCP** (`status`/`initialize`), PAS `/readyz` (qui ment).
- Tunnel `axon-mcp-tunnel-static` : retry/backoff sur -32000 pour lisser le trou de bind (~7 s).
- Blue/green complet (2 brains, handoff du flock SOLL) = seulement SI un fix warm moins cher ne suffit pas.
