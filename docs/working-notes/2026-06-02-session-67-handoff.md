# Session 67 Hand Off — 2026-06-02

Audit & remédiation SOTA (directive : rendre Axon livrable, supprimer la dette sans archiver, RCA-discipline). Mode autonome end-to-end. **2 guidelines méthodologie créés cette session : GUI-PRO-106 (RCA) + GUI-PRO-107 (grille 10 lentilles).**

## Commits (vérifiés, branch feature/pipeline-sq-reorder-point)

| Commit | Contenu |
|---|---|
| `a17b3f5a` | fix fuite bases test : `static TEST_DBS` jamais drainé → sweep idempotent OnceLock + test |
| `de9ad85e` | suppression codepath AGE vestigial (−144 LOC, 9 tests verts) |
| `7480f137` | **fix collision chunk_id** : `fuse_small_chunks` id `fused_L{s}_{e}` non unique (spans dupliqués) → compteur `_{seq}` ; TDD RED→GREEN |

Hors-commit : 726 bases `axon_test_*` / 96 GB → 8,5 MB. 2 edges SOLL invalides `DEC-901626→PIL` retirés.

## SOLL créé/maj cette session

- **GUI-PRO-106** (BELONGS_TO PIL-PRO-003) — RCA obligatoire (3 niveaux amont+aval+optimalité ; fix masquant symptôme = rejeté).
- **GUI-PRO-107** (BELONGS_TO PIL-PRO-003) — Grille 10 lentilles d'audit : Redondance, Idempotence, Unicité/Invariants, Back-pressure, Parallélisme, Couplage/Cohésion, Code mort, Observabilité, Fail-loud, Latence/Débit.
- **REQ-AXO-901846** (delivered) — collision chunk_id, RCA : racine = fuse id non unique (PAS symbol_id, car arêtes résolues par nom graph_ingestion:1129-1130). BUG B latent : homonymes intra-fichier sautés (seen_symbols:1064), non bloquant.
- **REQ-AXO-901847** (planned) — surface commandes contract-honest : axonctl entrée unique start+stop, `start` réconciliant (jamais 'stop first'), teardown dé-dédoublé (stop.sh 548 LOC enveloppe axonctl 1686 LOC). Éval techno incluse.
- **REQ-AXO-901848** (planned) — **dashboard 0/unknown ROOT-CAUSÉ** : chaîne transport SAINE (PG composite dashboard_state_full(3) = 129K edges/14K files/21 projets ; runtime.exs configure le bon socket /tmp/axon-live-brain-telemetry.sock ; bridge connecté). Racine = `missing_runtime_truth_heartbeat` : brain ne reçoit pas le heartbeat indexer→brain (bridge::RuntimeTruthFeed) → freshness bloquée `stale` → tuiles 0/unknown. MÊME racine que freshness-jamais-fresh. FIX = subsumé par DEC-AXO-901626 (observable-derivation depuis PG).
- **REQ-AXO-901841/843/845** — fuite test / cluster test isolé différé / AGE.
- **MIL-AXO-027** metadata.session_67_reeval — voir ci-dessous.

## Réévaluation MIL-AXO-027 (umbrella v4 SOLL contract, 9 slices)

Recommandation (sur expérience) : (1) PAS en bloc maintenant — c'est ergonomie/efficience (tokens, 67→15 outils, MVCC) ; les blocants livrabilité = correctness (pipeline, heartbeat, dashboard, commandes). (2) PROMOUVOIR slice 8 self-introspection (REQ-901793) — ROI max, validé : effort bash énorme faute de surface MCP runtime cross-instance. (3) FUSIONNER slice 8 avec DEC-901626 (observable runtime truth = même objectif). (4) DIFFÉRER slices 0-7. (5) ÉVOLUER : re-scoper l'umbrella autour de « observable runtime truth » (MIL-027 = s60, DEC-901626 = s66 a changé la donne).

## DÉCOUVERTE TRANSVERSALE (fil rouge)

`missing_runtime_truth_heartbeat` relie : freshness IST jamais `fresh`, dashboard 0/unknown, et le pivot DEC-901626. L'indexer écrit PG (46K symbols/59K chunks/129K edges, 8 projets, vivant) mais le brain reste `stale`/`brain_only` car le feed de vérité runtime n'arrive pas. C'est LE nœud à dénouer pour la livrabilité (dashboard + freshness + status fiables).

## Runtime @ handoff

Live `indexer_full` UP : brain :44129 (pid 17876, telemetry sock /tmp/axon-live-brain-telemetry.sock), indexer :44130 (pid 17875, vivant, écrit PG), dashboard :44127 (beam 17999). PG :44144 sain. Branch arbre **propre**. IST `stale` (heartbeat manquant, pas faute d'indexation — données fraîches en PG).

## Waves restantes (plan, après les P1 correctness)

1. **REQ-901848 + DEC-901626** : observable runtime truth (dénoue heartbeat→freshness→dashboard). **P1, fil rouge.**
2. **REQ-901846 aval** : re-index complet (anciens fused_L{s}_{e} orphelins) + valider via impact dès IST fresh.
3. **REQ-901847** : implémenter surface commandes (axonctl entrée unique, réconciliation) — risqué à chaud, faire hors-prod.
4. **Audit structurel IST #3/#10** (anomalies/clones/drift/god-objects : tools_context.rs 4294, embedder.rs 3684, optimizer.rs 2749 ; cohérence pipeline_v2/drain/fusion/writer) — **gated IST fresh** (donc gated sur #1).
5. **#2 DuckDB comments** (corriger à l'état PG réel, télégraphique ; FFI déjà pg_*) + image docker test axon-test/age-pgvector (renommer).
6. **#4 dead_code** : 145 `#[allow(dead_code)]` (soll.rs 12, guidance.rs 10, mcp.rs 8) — vérifier non-API avant suppression.

## Règles adoptées (à respecter désormais)

GUI-PRO-106 (RCA) + GUI-PRO-107 (10 lentilles) + [[feedback_delete_debt_no_archive_prelaunch]] (supprimer, jamais archiver, zéro rétrocompat ; SOLL jamais supprimée).

## Frontières produit/dev (opérateur)

Dashboard + Memgraph = produit. Benches = conserver (garde-fous perf). Seam FFI duckdb_* = vestige (déjà pg_*, reste comments).

## Probes vérif prochaine session

```
git log --oneline -7
psql -h127.0.0.1 -p44144 -U axon -d axon_live -tAc "SELECT id,status FROM soll.node WHERE id IN ('GUI-PRO-106','GUI-PRO-107','REQ-AXO-901846','REQ-AXO-901847','REQ-AXO-901848')"
# heartbeat racine : pourquoi missing_runtime_truth_heartbeat ? grep bridge::RuntimeTruthFeed publisher (indexer) vs consumer (brain)
timeout 4 python3 -c "import socket;s=socket.socket(socket.AF_UNIX);s.connect('/tmp/axon-live-brain-telemetry.sock');s.settimeout(3);print(s.recv(4000))"
```
