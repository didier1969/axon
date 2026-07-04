# Session 99 (2026-07-04) — clôture ROI SHI (902205/902187/902186) + promote propre

Audit-only. Canonique = SOLL (`CPT-AXO-052`, `REQ-AXO-902205/902187/902186`) + `MEMORY.md`.

## Contexte d'entrée

`axon init` (continuation) → opérateur demande de trier les 8 tâches ouvertes du backlog SHI par ROI, puis dit `go`.

## Tri ROI produit

1. **902205** (clôture triviale — fix déjà commité, restait valider+promote)
2. **902162** (mesurer le lag, pas commencé)
3. **902185** reste (dimensions restantes, lourd)
4. **902184** reste (déjà quasi terminé)
5. **902186** reste (vrai ROI + resilience/acyclicity)
6. **902187** reste (Δ persistée + re-surfacing)
7. **902190** reste (~45 hubs, volume)
8. **902192 S3** (gated, hors ROI autonome)

Un appel `advisor` avant d'exécuter a recalé le tri initial ("902205 n'est pas gratuit — la valeur nécessite un promote") et demandé de mener avec les tiers de valeur avant le classement fin détaillé.

## Exécution (ordre réel : 902205 → 902187 → 902186)

### 902205 — indexeur live rapporte un build_id figé

RCA (déjà posée par une session antérieure) : `resource_release_identity()` (runtime_boot.rs:500) no-op sans `AXON_ACTIVE_IDENTITY_FILE` ; le bloc `axon-indexer` de `process-compose.live.yaml` ne l'avait pas (le bloc `axon-brain` si). Fix mirroré en dev (parité), validé via `axon-dev start full` (indexer HEALTHY, 0 erreur identity), puis **full restart** du live (pas un simple resume — un fix config-only ne prend effet qu'au restart complet du superviseur process-compose, qui a parsé le yaml UNE FOIS à son lancement). Vérifié : `embedding_status.indexer_build_id` == manifest post-restart.

### 902187 — boucle fermée SHI (Δ persistée + re-surfacing)

`structural_health_index` persiste chaque mesure (`snapshot_id=AXON_BUILD_ID`, aggregate, sub_scores) dans `.axon/structural-history/{project}-shi.jsonl` (fichier dédié, distinct de l'historique anomaly-summary existant). Chaque appel diffe contre la mesure précédente : `delta_vs_previous` + par axe `below_target[].delta_vs_previous/re_surfaced`. `re_surfaced=true` = l'axe est toujours sous cible ET n'a pas progressé — le verdict anti-Goodhart (un correctif ne compte que si la RE-MESURE le confirme).

Allocation 70/20/10 mentionnée dans le corps original **délibérément différée** (aucun consommateur identifié — dashboard/gate/reporting — l'implémenter serait spéculatif, GUI-PRO-015 YAGNI). Suivi : `REQ-AXO-902206` (P3, tags deferred+yagni).

**Advisor a détecté un test malhonnête** avant que je passe à autre chose : le test affirmait vérifier `weighted_coverage` mais les ids de fixture (`TST::target`, sans composant fichier) étaient exclus par `is_testable_symbol` — l'axe réellement en `below_target` était `main_sequence`, un artefact non intentionnel. Corrigé (ids avec `src/lib.rs` embarqué) + ajouté le cas manquant (amélioration → `re_surfaced=false`, jusque-là seul le cas stagnant était testé bout-en-bout).

### 902186 — worklist : vrai ROI + resilience/acyclicity

Refactor DRY : extrait `compute_shi_raw_metrics()`/`build_sub_scores()`, partagés maintenant entre `structural_health_index` et `structural_health_worklist` (avant : le worklist dupliquait sa propre passe Martin-D, un fork qui pouvait diverger silencieusement).

Worklist unifie 4 catégories (coverage/coupling/resilience/acyclicity — les deux dernières absentes jusqu'ici) dans UN classement par `roi = expected_delta_shi ÷ blast_radius`. `expected_delta_shi` = simulation "si SEUL ce candidat était corrigé" (swap de la valeur d'axe dans une copie des sous-scores de base + `geometric_aggregate` pure, zéro divergence avec l'index). `blast_radius` = proxy direct (callers / degré de couplage / taille SCC) — PAS une simulation d'impact multi-hop complète (limite assumée).

**Dogfood dev finding** (dev-testé sur AXO réel, 20818 nœuds, AVANT promote per "Dev FIRST no exception") : le top ROI "coupling" était dominé par du bruit — des ids sans composant fichier (titres markdown `AXO::Risque 3. Nettoyer…`, sélecteurs CSS `AXO::.stack-title`, cibles stdlib `AXO::shutil.which`) retombaient sur le fallback `rfind("::")` de `module_of()` en "module" à 1 nœud, scorant trivialement Martin-D=1.0. Fix : filtre `is_real_source_symbol` (pas `is_testable_symbol`, qui exclurait aussi trait/struct/enum nécessaires à l'abstractness). Mesuré : SHI AXO 0.523→0.627, modules couplés réels 4677(bruit)→292(réels).

## Promote

Après dev-test complet (config + logique + fix pollution, tous validés sur données réelles), `promote_live_safe.sh --project AXO` exécuté. `PROMOTE COMPLETE build_id=v0.8.0-1349-g824a7c5b` = HEAD exact, `promote_status` phase=clean. Vérifié post-promote via appel direct `structural_health_worklist` sur le live : plus aucun module de bruit dans le top-5.

## Practices enregistrées

- **196** (AXO) : fix config-only process-compose → besoin full restart du superviseur, pas resume process enfant.
- **197** (AXO) : toute nouvelle dimension SHI itérant TOUS les nœuds doit filtrer `is_real_source_symbol` avant `module_of()`.
- **198** (`*`, shareable) : assertion "liste non vide" doit filtrer par NOM avant d'asserter — sinon un axe non visé peut déclencher la condition à la place de celui réellement testé (piège découvert via `advisor`).

## Hygiène SOLL (Step 3 GUI-PRO-028)

- `MIL-AXO-042` flippé `delivered` (tous enfants terminaux, hard gate réconciliation milestone).
- Gate `delivered sans evidence` : 66 REQ legacy (044/082/…) — dette **pré-existante, non touchée** cette session (hors scope d'un tri ROI).
- Gate `REQ ouvert sans milestone parent` : 10 REQ (902158/162/174/183/184/185/190/192/204/206) — organisés sous `REQ-902183` (REFINES) mais pas sous un Milestone (TARGETS). Pas d'action (créer un Milestone ad-hoc serait cosmétique) — à re-planifier si une vague dédiée démarre.

## Non fait / backlog restant (ROI décroissant)

1. 902185 reste — duplication-taux (scan clones pgvector) + god-objects (complexité cyclomatique) + profondeur module.
2. 902190 reste — ~45 hubs non couverts (méthode établie ; angle mort parser découvert s96 sur les imports nommés, non bloquant).
3. 902192 S3 — gate anti-orphelin fail-closed, **EN ATTENTE validation opérateur explicite** (faux-positifs connus).

## Addendum — 902162 re-vérifié sur le bon proxy (revue `advisor`)

La première mesure (`ist.indexedfile.mtime_ms` vs `last_seen_ms`) prouvait la fraîcheur du *bookkeeping fichier* en PG, pas celle du symptôme réellement rapporté par OPV : « `inspect` montre l'ancien corps du symbole ». `advisor` a relevé l'écart de proxy — corrigé par un test direct sur le vrai chemin de lecture du corps.

**Test réel** : injection d'un marqueur unique (`PROBE-902162-STALENESS-CHECK-7f3a91`) dans le corps de `clamp01` (structural_health.rs), sondage de `retrieve_context` en boucle (24× / 5s, 141s max) — le marqueur n'apparaissait pas dans le *snippet* retourné pour une requête générique. Investigation :
- `inspect` (mode=verbose) ne montre AUCUN corps de symbole — seulement métadonnées structurelles (kind/tested/callers/callees). Le symptôme original ne peut donc pas viser `inspect` tel qu'il existe aujourd'hui (l'outil a changé depuis le message OPV originel, ou OPV visait `retrieve_context`).
- `retrieve_context` est le vrai porteur de corps (`packet.supporting_chunks[].snippet`, dérivé de `ist.chunk.content`).
- Requête PG directe sur `ist.chunk` : le contenu du chunk fusionné `fused_L33_76_0` (qui embarque `clamp01`) contenait déjà le marqueur au moment du contrôle — la table est bien à jour.
- Le snippet ne l'affichait pas car c'est un aperçu tronqué (début du chunk), pas une extraction centrée sur le terme recherché — **bruit de rendu/ranking, pas de staleness**. Preuve : une requête `retrieve_context` avec le marqueur EXACT comme question retrouve le chunk fusionné en position 2 (le contenu EST indexé et cherchable).

**Conclusion confirmée** (sur le bon proxy cette fois) : le corps est réindexé en quelques secondes dans le cas courant — cohérent avec la mesure PG initiale (148ms sur ce fichier précis pendant ce test). Le vrai risque de « corps périmé » reste la fenêtre pause dev/live GPU (déjà identifiée), pas un défaut du chemin d'écriture chunk. Marqueur de sonde retiré du code après vérification (aucune trace résiduelle).
