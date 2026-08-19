# Session 119 (2026-08-20) — debt_digest stubs + « réindex proprement » + récolte promote

Audit-only. Canonique = `CPT-AXO-052` (session_pointer) + SOLL. Ne remplace pas la SOLL.

## Contexte d'ouverture
Reprise post-compaction sur le résidu de REQ-AXO-902361 (debt_digest v3) : l'opérateur avait signalé « Code n'est pas dry, on a déjà du code pour identifier les stubs ».

## Fil 1 — debt_digest stubs : détection au parser (REQ-902361, commit f85205db, live v1517)
Audit DRY des 4 sections. Découverte : la section `stubs` (`content LIKE '%todo!(%'`) était **cassée** — 5 puis 8 faux positifs sur AXO, dont le code du détecteur lui-même (ses chaînes de pattern) et son propre test. Un scan de contenu matche des occurrences de chaînes, pas des nœuds AST.
Fix : `parser/rust.rs::extract_macro_invocation` émet `kind='STUB'` sur les macros `todo!()/unimplemented!()` (nœud AST, zéro faux positif), nom sanitisé `::`→`_` (pas de collision d'id méthode). Le digest lit la colonne indexée `ist.Symbol.kind='STUB'`. Preuve avant/après : 0 (propre) vs 8. Les autres sections : dry+unlinked_code DRY-OK ; unlinked_soll garde sa query PG (le RAM SOLL n'expose pas le statut → fusion = hybride fragile, écartée).
→ practice #1149 (scan de contenu pour construction de CODE = cassé par construction ; détection au parser).

## Fil 2 — récolte des REQ activés par le promote v1517
Le promote (fait pour debt_digest) a rendu live un backlog de commits « activation en attente ». 5 REQ clos depuis PREUVE LIVE :
- **902339** : step 5b `apply_ddl_live` = no-op **4s** contre indexeur live (vs timeout 52s) — lu dans le log du promote même. Gardes catalogue no-lock validées sur le vrai live.
- **902337** : `soll_verify_requirements` nomme désormais les **609 preuves cassées** (`REQ→path(trc)`) ; Piste 2 fermée par mesure (règle lexicale = bruit).
- **902357+902355** : `axon_init_project` inline Vision+Pillar corps, re-joué sur AXO ET INK.
- **902345** : canal d'erreur `provider_compute_mismatch` first-class dans embedding_status (GPU embed sain).
→ practice #1163 (un promote flushe ce backlog → récolter depuis preuve live).

## Fil 3 — 902340 P0 « réindex proprement » (commit 84447c49)
Opérateur a tranché l'arbitrage : réindexer proprement. Déroulé :
1. Mesure précise (`max token_count > 512`) : **485 fichiers, 14,73% de l'index**, 24 projets — 3× le cadrage « 161 » du REQ.
2. Triage : KKI `optaplanner-examples/data/` = données numériques → `.axonignore` (côté kie, confirmé actif) ; le CODE Java KIE (drools/kogito) N'est PAS junk → réindexé.
3. Outil : `purge_amplified_chunks.sh` gagne un critère `oversized` (DRY, une CTE paramétrée), le critère que le REQ prescrivait.
4. Purge live (guard fix-fenêtre 3f93a514) + rebuild : pire morceau **37610→1414 jetons**.

### ERREUR MÉTHODO (à ne pas refaire) — mesure en vol
J'ai déclaré à l'opérateur « 44 morceaux >512 / 0,008% » sur un instantané pris **pendant** que FSF re-chunkait. Le stable réel = **3627 morceaux (49 gros PDF/HTML)** — 82× plus. Reproduction exacte du défaut de la famille session 113 : « le chiffre est juste, sa lecture est fausse ». Corrigé à l'opérateur.
→ practice #1162 (ne jamais mesurer un réindex tant que pending non drainé + désignation stable).

### Résidu tracé — REQ-902364
49 gros fichiers (wegleitung 335K→1309 jetons alors que stv_nw 113K→492 ; lt580.html 1,3M) que v1517 **reconstruit oversized** (pas un oubli de purge : `pending`). Gap chunker réel non root-causé (le code coarse lu cape à ~1100 chars mais produit 3927 ; `target_chunk_tokens=384` confirmé par diag). Nécessite instrumentation runtime. Non bloquant (96% mieux qu'avant, rien n'est pire).

## État de clôture
HEAD 84447c49 = origin/main (poussé), live v0.8.0-1517, soll_validate=0, handoff_check WARN (dette historique + advisory, aucun de mes REQ). Réindex draine en tâche de fond. Follow-ups : 902362/363/364.
