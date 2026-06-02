# Session 67 Hand Off — 2026-06-02

Audit & remédiation SOTA (directive : rendre Axon livrable, supprimer la dette sans archiver, RCA-discipline). Mode autonome end-to-end. **4 guidelines méthodologie créés : GUI-PRO-106 (RCA) + GUI-PRO-107 (grille 10 lentilles) + GUI-PRO-108 (pas de version dans noms internes).**

## ⭐ MISE À JOUR post-promotion (fin session 67)

**LIVE PROMU à jour + DASHBOARD CORRIGÉ end-to-end.** Le brain live était 38 commits périmé (`v0.8.0-757`, pré-`dashboard_state_v1`) → dashboard 0/unknown. RCA prouvée (socket : 0 dashboard_state_v1 en live, présent en dev), validée dev-first, puis `promote_live_safe` → live = **`v0.8.0-795-gf1cdab19`** (HEAD, inclut correctifs s67 AGE+fuse). Vérifié **navigateur réel** :44127 : toutes tuiles peuplées (14304 files, 78326 symbols, 210115 edges, 24 projets), console clean. REQ-AXO-901848 delivered.

- Promote post-check (150s MCP poll) a crié FAILED sur un succès fonctionnel → réconcilié via `--finalize-only` (sans restart). Trouvaille contract-honest #7 (REQ-901847 metadata, à rejouer : MCP flaky in-session bloquait l'écriture).
- **NOUVELLE directive opérateur (task #11)** : le dashboard a des valeurs REDONDANTES (tuile INDEXED FILES 14304 vs funnel INDEXED 3332 contradictoire ; CHUNKS/EMBEDDED en double tuile+funnel) + ERRONÉES (coverage_pct >100% possible) + split dev/live à reconfirmer. Re-vérifier TOUT.
- **NOUVELLE directive (task #10)** : audit cohérence/correction du pipeline complet.
- REQ-AXO-901849 (planned) : rename labels version résiduels cat-A (pipeline_v2→pipeline) — **session fraîche** (gros diff core, ne pas précipiter).
- Observé : live indexer redémarré (pid 7865) mais brain `brain_only`/`no_indexer_paired`/`stale` (warmup post-restart OU gap pairing brain↔indexer — à surveiller ; freshness des tools query/inspect/impact en dépend).

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
git log --oneline -10
psql -h127.0.0.1 -p44144 -U axon -d axon_live -tAc "SELECT id,status FROM soll.node WHERE id IN ('GUI-PRO-106','GUI-PRO-107','GUI-PRO-108','REQ-AXO-901846','REQ-AXO-901847','REQ-AXO-901848','REQ-AXO-901849','REQ-AXO-901850','REQ-AXO-901851')"
```

## ⭐⭐ SUITE post-promotion (fin de session 67)

**LIVE PROMU** `v0.8.0-795` (commit `e65d52ab` inclus à promouvoir aussi). Dashboard alimenté vérifié navigateur. Commits ajoutés : `7480f137` (fuse), `f1cdab19` (handoff), `e65d52ab` (dashboard dedupe+canonical+hot-reload).

**Audit correction dashboard (#11) — fait :**
- **Split dev↔live VÉRIFIÉ SÛR** : contamination impossible par défaut — socket per-instance + SqlGateway `allow_cross_instance_fallback:false` fail-loud (REQ-901800/901802) + **aucun Ecto Repo** (retiré REQ-901801 / PIL-AXO-001, dashboard owns no canonical state). Seule fuite = label sidebar codé en dur « live·MCP44129 » → corrigé instance-aware + **valeurs canoniques** (instance_kind + brain_port via env AXON_BRAIN_PORT). `priv/repo/migrations` = résidus morts Ecto à nettoyer.
- **Redondances supprimées** : tuiles Indexed Files / Total Chunks / Embedded (dupliquaient funnel) → tuiles = hors-funnel (Symbols/Edges/Pending). coverage_pct déjà clampé ≤100% (pas un bug). defp coverage_tone mort supprimé.
- **Ecto** : NE PAS réintroduire (PIL-001 + ajouterait un risque split). Réponse à « mieux avec ecto ? » = non.
- **Hot-reload dev** : `code_reloader:true` activé (édits réapparaissent au reload). live_reload auto-refresh = follow-up (ajouter dep `phoenix_live_reload only::dev`). Sur LIVE : à NE PAS activer (anti-pattern prod) ; vraie correction = séparer MIX_ENV (live→prod, dev→dev), REQ-901851.

**ROOT CAUSE valeurs « non fiables » (mode brain_only / chunks_sec 0 / provider cpu)** : le dashboard source l'état LOCAL du brain (brain_only, n'embed pas) au lieu de l'INDEXER. Le heartbeat indexer live dit `effective:tensorrt` (GPU OK post-promote) mais le dashboard montre le provider brain-local `cpu`. **FIX (prochain P1, EN COURS, non commité)** : `compose_dashboard_state_v1` (dashboard_state.rs) + main_telemetry.rs doivent sourcer provider/runtime_mode/rates depuis `projected_indexer_runtime_from_heartbeat()` (déjà lu par le brain) quand brain_only + indexer pairé. C'est le slice concret de DEC-AXO-901626 (observable runtime truth) qui corrige aussi freshness-stale.

**EXIGENCE OPÉRATEUR (3 valeurs compute distinctes, pas un seul 'GPU/CPU' ambigu)** : le dashboard doit indiquer SÉPARÉMENT le compute de : (1) **Brain** = CPU (n'embed pas) ; (2) **Pipeline A** (A1/A2/A3 graphe/chunks/FTS) = CPU ; (3) **Pipeline B** (B1/B2/B3 embedding) = GPU/tensorrt (ou cpu si fallback, depuis heartbeat indexer embedder_provider.effective). Le header actuel « GPU cpu » conflate les trois et montre le cpu brain-local → trompeur. Source : heartbeat indexer (B) + rôle local (brain) + nature des stages (A=CPU par design).

**SOLL ajoutés session 67** : GUI-PRO-106 (RCA), GUI-PRO-107 (10 lentilles), GUI-PRO-108 (no version interne), REQ-901841/843/845/846/847/848/849/850/851. MIL-027 réévalué (metadata).

**Prochaines P1 (ordre)** : (1) slice DEC-901626 dashboard-from-indexer-heartbeat (provider/mode/rates + freshness) — brain-side, rebuild+promote ; (2) re-promouvoir live avec e65d52ab (dashboard dedupe visible) ; (3) GPU : confirmer indexer utilise vraiment le GPU (nvidia-smi pid actif pendant embedding) ; (4) REQ-901849 rename pipeline_v2→pipeline (session fraîche) ; (5) #2 DuckDB comments + priv/repo/migrations morts ; (6) #3/#10 audit structurel IST (gated freshness) ; (7) #4 dead_code.

**Runtime @ handoff** : live `v0.8.0-795` UP (brain 44129 + indexer 44130 effective:tensorrt + dashboard 44127). Dev STOPPÉ (libéré). PG 44144 sain. Arbre propre.
