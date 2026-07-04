# Session 98 (2026-07-04) — La saga fragmentation-graphe + robustesse drain

Audit-only, append-only. Ne remplace PAS SOLL/`CPT-AXO-052`. Détail canonique en SOLL.

## Fil narratif

Session autonome partie sur le backlog SHI (`soll_work_plan` wave-1). Déroulé :

1. **902193** (déjà commité s96, non-clos) → vérifié + fermé. Puis **902185 dim-5** `intent_alignment` (anti-Goodhart : versant orphan_code DÉFÉRÉ car traçabilité symbol-grain sparse-by-design ~115 → un orphan_code naïf = artefact Goodhart).
2. **902189+902188** (promote : qualify gaté liveness + manifest∥dev_gate) → **prouvés en prod** au 1er promote (le log montre `indexer_down → SKIP qualify DEFER to 6c → recovery → clean` : exactement l'incident 1316 corrigé).
3. **902198 slice1** (cœur bisection poison-pill pur + flush_chunks résilient).
4. **902201** (worklist/pagerank : ids rankés dans le TEXTE) — friction dogfoodée en attaquant 902190 (l'outil disait « attack the top » sans que le top soit lisible).
5. **902202** (worklist pollué : gate `.rs` exclut les non-Rust).

## Le tournant : le RCA inline (reproché puis fait)

L'opérateur m'a repris deux fois sur mon réflexe de deferral (« RCA dédiée », « effort focalisé ») → **fait le RCA inline, tout de suite**. Il a mordu :

- En attaquant 902190, `try_snapshot` (29 callers) flaggé covered=false alors qu'il est testé. Vérité-sol SQL : l'arête vers try_snapshot a une source **`AXO::IstGraphView::forward_at_radius` FANTÔME** (pas un Symbol), la définition étant `…view.rs::forward_at_radius`.
- **Sévérité mesurée : 17 527 / 47 101 (37%) des CALLS AXO ont une source fantôme.** Le parser qualifie le caller en `Class::method` (désambiguïsation, rust.rs:229) ; `symbol_id` lit le `::` comme un chemin global et DROP le path → fantôme. Les arêtes ENTRANTES atterrissent sur la déf, les SORTANTES partent du fantôme → **graphe coupé à travers chaque méthode impl** (covered/impact/bidi_trace/path aveugles).
- **902203** : `resolve_call_source_id` réconcilie la source au nom court file-local (5324 défs Rust, toutes courtes, 0 qualifiée → fix sûr).

## Validation : promote ≠ re-index

Piège : après promote, `phantom_source` INCHANGÉ. Leçon (practice 191) : **un promote ne re-parse pas les fichiers completed** ; il faut `rescan full=true`. + l'indexeur live tournait encore 1338 après le 2e promote (pas de 6c full-restart) → `process-compose restart axon-indexer` pour charger 1340. Puis rescan full AXO → **validé** : real_def→try_snapshot 0→1, bidi_trace reconnecté, SHI 0.62→0.67.

## Les fantômes : nettoyage + non-récidive

- Les fantômes sont **inertes au RAM** (source non-nœud → droppée du CSR) — le fix fonctionnel = la reconnexion 902203, pas la suppression.
- **902204-a** : le re-index ne purgeait pas les CALLS sourcées des SYMBOLES du fichier (seulement `source_id=path`) → arête stale survivait. Fix + fault-injection dev.
- **902204-b** : cleanup one-shot live des **101 929 fantômes** (`source_id NOT IN ist.symbol`), validé dev-first (transactionnel avant/après : valides intactes), puis live (autorisé exceptionnellement). Ciblé, PAS wipe total (corruption confinée à une colonne d'un type d'arête).

## 902198 slice2 : drain robuste (défense en profondeur)

Sur question opérateur (« le stall vient-il de l'absence de COPY robuste ? ») : non (pas de poison dans les logs), mais risque latent réel. Livré : **pré-filtre `strip_nul` étendu à TOUTES les tables** (symbols/edges/indexed_files, pas juste chunks) + **backstop bisection** sur `flush_batch_async` (fallback résilient : cœur structurel atomique + chunks bisectés). **E2E fault-injection PG dev sans Docker** (FK 23503) prouve le maillon `pg_sqlstate` sur vraie erreur.

## Sagesse opérateur : ne pas toucher un système qui marche

Le discovery-stall (2106 `discovered`) → diagnostiqué **artefact dev-churn** (mes restarts multiples interrompent des walks ; drainé par full-restart/rescan ; sweep RIPPÉ à dessein). **Décision : PAS de réconciliation périodique** (CPT-AXO-90057). Filtre appliqué à tout le reste → aucun bug urgent, backlog = nice-to-have.

## État final
LIVE 1340 clean ; HEAD b5a33f5a (2 commits non-promus : 902198-slice2 + 902204-a). Reprise = `CPT-AXO-052`.
