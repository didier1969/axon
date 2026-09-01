# Session 134 — 2026-09-01 · la fenêtre de compilation rouverte, et une arête qui a survécu à son fichier

État vivant : `CPT-AXO-052`, quatre dernières sections. Cette note ne duplique pas le pointeur ; elle garde ce qui n'a pas sa place dans un nœud SOLL.

## Ce qui a été livré

| REQ | commit | preuve |
|---|---|---|
| `REQ-AXO-902576` pt 3 | `c1f134c0` | le préflight n'accuse plus TensorRT quand son absence est nominale · test 3 cas |
| `REQ-AXO-902580` | `a1105ba7` | `sections`/`section` atteignent `data.description` · vu ROUGE puis vert |
| `REQ-AXO-902584` | `5cec186e` | `impact` refuse un symbole qui n'existe que comme extrémité d'arête · vu ROUGE puis vert |
| — | `59c05259` | relecture du précédent : branche rendue inatteignable + coût PG non déclaré |

Poussé (`6db80e49..59c05259`, 7 commits), promu (`v0.8.0-1687-g59c05259`), et **les trois correctifs vérifiés sur le runtime**, pas déduits d'un signal de santé.

Ouverts : `REQ-AXO-902585` (un gate qui ne peut pas échouer) · `REQ-AXO-902586` (les arêtes survivent à leur fichier).

## L'enquête, dans l'ordre où elle s'est faite

LLL signalait qu'`impact` affirmait `confidence: high` sur `stock_reserve`, que `query` et `inspect` déclaraient absent. L'énoncé du REQ disait « il AFFIRME FAUX ». La mesure a corrigé cet énoncé.

1. **Reproduit** à l'identique. Puis contraste : sur un symbole vraiment inventé, `impact` refuse proprement — donc `stock_reserve` existait quelque part.
2. **Première hypothèse** : l'extracteur Python indexe les identifiants situés dans des littéraux de chaîne. `brick_macro.py` définit `stock_reserve` uniquement dans des triple-quotes. **Réfutée** : `gross_to_net`, dans le même cas, est absent des DEUX surfaces.
3. **Blocage** : l'outil MCP `sql` rend une enveloppe vide, y compris sur `SELECT 1`. Contourné par le jumeau HTTP `POST 127.0.0.1:44129/sql` (le champ s'appelle `query`). **Toute l'attribution est passée par là.**
4. **Trouvé** : `ist.symbol` n'a aucune ligne `stock_reserve`. `ist.edge` en a une, et une seule :
   `LLL::llmlang::examples::tmph5laa9_f.lll::fulfill --CALLS--> …::stock_reserve` (2026-07-27).
   Le fichier — temporaire — est absent du disque.
5. **Mécanisme** : la traversée d'`impact` est INVERSE, donc son index RAM porte les nœuds ayant une arête ENTRANTE. La cible est résolue, la source non, l'id complet non plus. Ce qui semblait aléatoire est entièrement prédit.

`query` et `inspect` avaient raison. `impact` n'inventait rien : il acceptait une extrémité d'arête comme preuve d'existence.

## Deux erreurs de ma part, corrigées avant qu'elles coûtent

**Le garde mal placé.** Posé avant le calcul d'`effective_project`, il rendait inatteignable la branche « pass an explicit project » — sa condition ÉTAIT celle du garde — et rendait `symbol_project_code` inconditionnel, ajoutant une requête PG sur le chemin chaud d'un outil RAM-first. Trouvé en relisant le diff à froid, pas en attendant un symptôme. Déplacé après le réchauffage : les deux défauts tombent.

**Les pratiques sans preuve.** Mes cinq premiers `practice_put` ont été écrits avec `evidence` vide — le champ avait été avalé par `dense`. Rien ne le signalait dans la réponse ; mesuré en base (`len_evidence = 0`, là où les dépôts d'autres agents portent les deux champs). Rebattus, `1950` retirée avec `superseded_by`.

Dans les deux cas, ce n'est pas le test ni l'outil qui a averti : c'est une relecture volontaire de ce qui venait d'être écrit.

## Tentative mesurée et réfutée — à ne pas rejouer

Rendre le garde gratuit en le lisant dans le snapshot RAM (`IstGraphView::node_kind_db`) : le test du cas fantôme est repassé ROUGE. **Le snapshot fabrique un `kind` par défaut pour un nœud qui n'est qu'une extrémité d'arête.** Il ne sait donc pas distinguer un symbole d'un résidu. Versé à `REQ-AXO-902586`, où cela ouvre une seconde voie : que le RAM laisse `None` rendrait le résidu visible au lieu de le déguiser.

## Ce que le promote a appris au passage

`step 6: qualify_mcp ✅ done in 19s` — la qualification a tourné. `promote_status` appelé juste après rend toujours `qualification_passed: pass=true` avec `detail: "no qualification recorded"` et `qualification_ok: null`.

Le gate rend le même verdict **avant et après** la mesure : il ne peut structurellement jamais échouer. Symétrie exacte de `REQ-AXO-902566`, où la porte de build ne peut jamais passer. Un écart entre le contrôle déclaré et le contrôle réel, dans les deux sens.

## Outillage — ce qui a coûté du temps

- **`nexus-job run` ne rend pas la sortie du processus.** Le premier job a rendu `state=succeeded, exit_code=0` avec zéro ligne de cargo. Le script lancé doit rediriger lui-même (`exec > "$LOG" 2>&1`) et écrire une ligne de verdict. Sans cela, `REQ-AXO-902580` était commité rouge.
- **Le courtier reclasse en `huge`/12 G** un job demandé `--class medium --memory 6G`, sans le dire.
- La porte `GUI-AXO-1034` complète coûte ≈ 8 min ici (`test --lib` 456-465 s). Trois passages dans la séance ; les runs ciblés coûtent 3 s.

## Verdict de clôture

`axon_handoff_check` : **warn**. Sept `pass` — arbre propre, 0 commit non poussé, 0 violation SOLL, 0 REQ orphelin, runtime joignable. Deux `warn` : 50 REQ `delivered` sans preuve (dette héritée, aucun de cette séance) et `debt_digest` à 3 010 (advisory, était 3 018 à l'ouverture).

Pratiques déposées, scope `*` : `1944` `1945` `1946` `1951` `1956`.
