# Session 117 — 2026-08-18 — REQ-902337 piste 1 + REQ-902331 + promote v0.8.0-1507

Audit-only, append-only. Canonique = `CPT-AXO-052`. Ne remplace pas le session_pointer.

## Contexte
Reprise « axon init et reprends ». Build installé au départ = `v0.8.0-1493` (activation des 6 commits s116 en attente). Fork identifié : le promote (activation) était operator-gated (coupe le MCP des tiers VPC).

## Décision de fork
Le promote resté operator-gated → avancé la piste autonome à plus forte valeur (W2) au lieu de l'auto-déclencher. Re-validation avant de coder (practice #1060) sur chaque REQ.

## Livré
1. **REQ-902337 piste 1** (`a12794eb`) — `soll_verify_requirements` nomme les preuves cassées. La méthode de balayage (`broken_file_evidence_by_requirement`, ex-`..._counts_by_requirement`) connaissait déjà chaque chemin cassé en Phase 3 mais ne gardait qu'un compte → fait remonter les offenders `{traceability_id, path}` (DRY). Exposés en JSON (`details[].broken_file_evidence_offenders`) + texte borné 15 lignes. Mesure live : 609 preuves cassées.
   - **Piste 2 mesurée → NON livrable** : une règle lexicale (corps « hors périmètre / superseded / obsolète ») sur le corpus SOLL réel donne 100% de faux positifs (en-têtes de section markdown `## Hors périmètre`, méta-mentions). Scopé `type=Requirement` : 0 vrai positif (cas SWT déjà nettoyés). Close par la mesure, pas par abandon. Le vrai signal body→statut se capte à l'écriture (GUI-PRO-110) ou structurellement (arête SUPERSEDES), pas lexicalement.
   - Piste 3 (résidu scoring 902295) non traitée.
2. **REQ-902331** (`165d3780`) → delivered — `scan_symbol_names` ajoute `AND s.tested IS NOT TRUE`. Prémisse du REQ (« l'IST n'a aucun marqueur de test ») corrigé : le flag `tested` EST le marqueur (vérifié : fn `#[test]` à 0 caller = tested=true ; prod couverte = tested=false → pas de sur-exclusion possible). Résidu pipeline 0 (3 fns de test exclues), `reset_baseline` honnête.

## Promote (autorisation opérateur explicite après contrôle)
Contrôle avant : host sain (0 build concurrent), git propre, soll_validate=0, promote_status=clean, pas de D-state (dxg OK). **Trouvaille clé** : `axonctl.rs:902` — le cutover respawn `scripts/axon start full` (start.sh) après `process-compose down`, en héritant de l'env du shell de promote ; `start.sh:131` défaut = `tensorrt` (manifeste ORT mort) si `AXON_EMBEDDING_PROVIDER` unset. → Exporté `AXON_EMBEDDING_PROVIDER=cuda` sur la commande de promote.
Résultat : v0.8.0-1493→1507, coupure MCP **9s**, pas d'auto-rollback, phase=clean. Post-check `embedding_status` : compute=GPU, effective_provider=cuda, provider_compute_mismatch=false. Table `role_exit_event` créée (apply_ddl_live), contrôle négatif OK (arrêt propre = 0 ligne).

## Incident géré (pas un échec)
Pendant le gate scopé, load a picé à 74. Diagnostic vmstat : `wa=53, r=4, si=82/so=0` → I/O-bound (link de `soll_and_guidelines.rs` recompilé), PAS CPU. Les 3 rôles live ont survécu. N'ai pas abORTé ; gate fini vert (288/0). Contraste avec DEC-AXO-901670 (load 105 CPU → PG killed). → practice #1094.

## Practices déposées
#1075 (re-valider corps REQ vs code live) · #1090 (cutover hérite env shell promote ; épingler+vérifier provider) · #1094 (diagnostiquer pic load I/O vs CPU avant d'abORTer).

## Reste
902340 P0 (arbitrage index opérateur) · 902330/902343 (grosses features) · reclasser s116 residuals delivered · contrôle positif role_exit_event (kill injecté) non testé (invasif).
