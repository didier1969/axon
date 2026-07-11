# Session 100 suite — 2026-07-11 — SHI not_applicable (902214) + 902215 durci + promote + cleanup axon_live

Audit-only, append-only. Canonique = session_pointer `CPT-AXO-052` + SOLL (REQ delivered) + git log. Ne remplace pas SOLL.

## Arc de session
Reprise post-compaction sur la lignée 902215 (isolation tests, commité a670c969 la session précédente), puis 902214 (plan approuvé `stateful-drifting-crane`), puis promote groupé sur go opérateur, puis handoff.

## 902215 — durcissement (finalisation)
Le commit a670c969 était fait ; l'advisor a signalé un trou : la branche `#[cfg(not(test))]` de `resolve_database_url` n'avait été compilée par AUCUNE commande (`cargo test`/`--lib` compilent avec `cfg(test)` ON). `cargo build` nu → RC=0, arm production OK. Scope cfg(test) vérifié intact (les 4 tests d'intégration `tests/*.rs` sont tous `#[ignore]` ou surchargent `AXON_LIVE_DATABASE_URL` vers un testcontainer ; crate `axon-mcp-tunnel` = aucun store). Réf. patch éphémère de 902216 reformulée « re-dérivable de la RCA » (le scratchpad ne survit pas). → practice 297.

## 902214 — SHI weighted_coverage not_applicable (Fork B)
**Design (advisor)** : Fork B = vraie neutralisation, pas 1.0. Découverte clé en navigant le code : `weighted_coverage_score(0,0)` retourne déjà 1.0 (via le gate `.rs` de 902202, delivered), MAIS 1.0 dans une moyenne géométrique pondérée GONFLE le SHI vs exclure l'axe. Le mécanisme d'exclusion EXISTAIT déjà : `geometric_aggregate` filtre `weight>0` (num) et somme tous les poids (dénom) → un axe poids-0 est exclu des deux. Donc Fork B = changer l'ACTION au trigger existant.

**Implémentation** (3 fichiers) :
- `parser/mod.rs` : `language_has_coverage_model(module_id)` adjacent à `get_parser_for_file` (registre parseur), Rust-only, capacité = bit DISTINCT de l'existence du parseur (Python a un parseur, pas de modèle couverture).
- `structural_health.rs` : champ `SubScore.not_applicable` + constructeur `SubScore::not_applicable` (poids 0, value 1.0 display-only) + `below_target()` filtre les na.
- `mcp/tools_ist_algorithms.rs` : `is_testable_symbol` appelle le registre (au lieu de `.ends_with(".rs")` inline) ; `build_sub_scores` conditionnel sur `total_testable_symbols==0` ; sérialisation `not_applicable` + compte na dans le résumé ; **fix airtight** : ne PAS persister le 1.0 display-only (sinon transition na→mesuré = fausse régression).

**Discriminant décisif (advisor)** : trigger sur `total_testable_symbols==0` (CAPACITÉ), JAMAIS `covered_pr==0` — c'était le landmine du revert s100 (un vrai projet Rust 0% serait sur-neutralisé). Test dédié le prouve.

**Dogfood** : BKS (Odoo/Python) montrait `weighted_coverage=1.0` mislabel avant ; après promote, `structural_health_index BKS` → « 1 not_applicable (neutralized) ». Le SHI reste 0.0 (un AUTRE axe réel domine) — le fix concerne l'HONNÊTETÉ de l'axe couverture, pas l'agrégat de BKS.

**Tests** : 7 unitaires neufs + 2 SHI d'intégration préservés ; suite --lib 1604/1604 + --bins. Commits 48196b73 (cœur) + cac5c74f (airtight persistance).

## Cleanup axon_live (autorisé opérateur)
Re-vérification vérité-sol AVANT DELETE a attrapé une erreur du résumé pré-compaction : « BKS = code projet de test » était FAUX (BKS = BookingSystem réel, 602 fichiers Odoo, root /home/dstadel/projects/BookingSystem) → EXCLU. Supprimé : 103 lignes `ist.indexedfile` synthétiques AXO (0 chunk/symbol descendant) + 4 stubs `soll.projectcoderegistry` /tmp (OTH/PJA/PJB/PRJ). Backups CSV en scratchpad. After-counts 0|0, BKS 602 fichiers intact. → practice 300.

## 902117 (MBX-5) — analyse threat-model, resté deferred
Posture mailbox actuelle (lue) : identité sender/recipient AUTO-ASSERTÉE (spoofable) → fuite cross-tenant latente ; ACL default-open+deny ; signature existe (`mailbox_verify`) ; body en clair. **Finding** : tous les projets = même opérateur de confiance mono-host aujourd'hui → menace inexistante → crypto/identité PRÉMATURÉES. Reste `deferred` avec trigger (multi-opérateur) + décompo S1-S5 loggés dans le REQ. → practice 298.

## Promote
`promote_live_safe.sh --project AXO` : build release 2m32s → dev_restart + **dev_gate step 2b validé le binaire candidat sur DEV avant live** (satisfait « dev avant live ») → step 5 live swap (fallback full stop+copy+start, normal) → step 6c phase=clean → finalize HEALTHY. Live = v0.8.0-1372-gcac5c74f, HEAD==live exact, md5 158c5a7b. Promote decision opérateur : « pas encore » puis « go » après assessment §C.

## Assessment §C (backlog différé)
Aucun item actionnable fin-de-tour : 902117 threat-model-gated (deferred correct), MIL-043 XL sessions dédiées, 902121/902206 P3. Hygiène §B : aucune umbrella clôturable (902183/902185 travail ouvert ; 902192 slice viz à confirmer).

## État SOLL noté
0 milestone AXO ouvert (tracking umbrella-REQs) ; ~70 delivered historiques sans evidence = dette pré-existante (non backfillée — fabriquerait de la preuve). Gate 2 : MIL-018/027/050 rejected (laissés), MIL-043 deferred (enfant REQ-065 deferred, laissé).
