# Plan: Reprise Reality-First d'Axon

## Goal
Reprendre le projet sur la base de sa réalité actuelle, valider l'environnement de vérité, mesurer l'état exécutable réel, puis choisir la prochaine tranche de travail la plus défendable.

## Current Snapshot
- Date de reprise: 2026-04-01
- Branche active: `feat/rust-first-control-plane`
- Constat initial: les documents locaux parlent d'une stabilisation "final gate passed", mais le dépôt reste très sale et le plan précédent n'est plus un reflet fiable de la situation courante.

## Phases

### Phase 1: Prise de terrain réelle
- [x] Relire les documents de reprise existants (`README.md`, `STATE.md`, handoff, plans actifs).
- [x] Vérifier l'état Git et distinguer code métier vs artefacts/runtime.
- [x] Identifier l'environnement officiel du projet.

### Phase 2: Validation de l'environnement de vérité
- [x] Vérifier que le shell courant n'est pas un shell Devenv valide.
- [x] Vérifier que `devenv shell` fournit bien l'environnement officiel attendu.
- [x] Noter les écarts/outils encore externes mais tolérés (`uv`, `tmux`, `nc`, `curl`).

### Phase 3: Réconciliation théorie / réalité
- [x] Comparer le récit local (README / STATE / handoff / progress) avec l'état Git réel.
- [x] Vérifier la présence réelle des frontières Rust-first vs autorité résiduelle Elixir.
- [x] Confirmer si les objectifs `A/B/C` du handoff sont déjà matérialisés dans le code et/ou les docs.

### Phase 4: Validation exécutable
- [x] Obtenir un premier signal sur le core Rust (`cargo test` ciblé ou global).
- [x] Obtenir un premier signal sur le dashboard Elixir (`mix test` ciblé ou global).
- [x] Vérifier la surface runtime canonique (`start-v2.sh` + probes `/sql` et `/mcp`).

### Phase 5: Priorisation de reprise
- [x] Identifier les défauts dominants bloquants.
- [x] Choisir la prochaine tranche de remédiation par ordre de dépendance.
- [x] Écrire un handoff de reprise durable si la session se termine avant correction.

### Phase 6: Formalisation de la tranche "ingress guard"
- [x] Documenter la décision d'architecture pour un filtre amont dérivé de `File`.
- [x] Fixer les invariants non négociables avant toute implémentation.
- [x] Écrire un plan d'implémentation TDD minimal-risque.
- [x] Exécuter le plan.

### Phase 7: Investigation mémoire post-pic
- [x] Distinguer `RssAnon` / `RssFile` / `RssShmem` dans la télémétrie runtime.
- [x] Exposer les métriques DuckDB utiles (`duckdb_memory()`, `duckdb_temporary_files()`, taille DB/WAL).
- [x] Vérifier sur un run réel si le pic mémoire est majoritairement allocateur, cache fichier, ou working set DuckDB.
- [x] Définir ensuite une expérimentation prudente sur purge/trim/checkpoint/allocateur.

### Phase 8: Causalité `pending`
- [x] Ajouter une première vérité persistée sur la cause de retour en `pending`.
- [x] Couvrir les transitions critiques `pending/indexing/indexed/...` avec une causalité canonique exploitable, y compris les échecs de queue/commit.
- [x] Exposer ces causes dans les vues opératoires et MCP.
- [x] Couvrir explicitement les transitions de scheduling `pending -> indexing` et `pending différé`.

### Phase 9: Tampon mémoire d’ingress
- [x] Formaliser l’architecture cible `Watcher/Scanner -> IngressBuffer -> IngressPromoter -> DuckDB`.
- [x] Décider que le MVP d’ingress reste mémoire seulement, sans WAL disque dédié.
- [x] Introduire `IngressBuffer` isolé avec contrat TDD.
- [x] Introduire `IngressPromoter` et l’API canonique de promotion batchée vers `File`.
- [x] Convertir le watcher en producteur d’ingress.
- [x] Convertir le scanner en producteur d’ingress.
- [x] Réaligner la vérité MCP/opératoire pour distinguer ingress buffer vs backlog canonique.

### Phase 10: Séparation DB lecture/écriture exploitable
- [x] Router les lectures pures sur `reader_ctx`.
- [x] Préserver la fraîcheur immédiate après write avec une garde courte.
- [x] Faire passer les chemins SQL bruts par une gateway qui sépare lecture et mutation.
- [x] Rerouter les lectures techniques lourdes hors du writer quand cela est sûr.

### Phase 11: Refonte cockpit LiveView
- [x] Documenter la refonte cible du cockpit operateur.
- [x] Supprimer les dependances CDN des layouts dashboard.
- [x] Refondre `CockpitLive` autour de la valeur operatoire.
- [x] Exposer backlog, projets, runtime, ingress et memoire dans la page.
- [x] Verrouiller le rendu par tests LiveView.
- [x] Valider `mix test`, `mix compile` et `mix precommit` sans redemarrer le runtime courant.

### Phase 12: Revalidation et repeuplement SOLL
- [x] Migrer additivement les tables `soll.*` au boot.
- [x] Restaurer le snapshot `SOLL_EXPORT_2026-04-03_002835.md`.
- [x] Completer les stakeholders et verifier les liens minimaux.
- [x] Valider `axon_validate_soll`.
- [x] Corriger la lecture stale de `soll.db` sur `reader_ctx`.

### Phase 13: Phase 3 retrieval + staged vector pipeline
- [x] Formaliser l'architecture `retrieve_context` et la tranche pipeline vectoriel stagee.
- [x] Introduire le contrat d'embedding canonique (`embedding_contract.rs`).
- [x] Extraire les frontieres `prepare -> embed -> persist` du worker vectoriel sans changer la concurrence runtime.
- [x] Ajouter la surface MCP `retrieve_context` avec planner, degradation honnete et evidence packet.
- [x] Ajouter le harness de qualification `scripts/qualify_retrieval_context.py` et ses tests Python.
- [x] Revalider au moins les tests cibles Rust de la tranche (`test_prepared_vector_embed_batch_`, `test_retrieve_context_`).
- [ ] Decider si la reprise doit continuer sur:
  - la qualification runtime reelle de `retrieve_context`
  - ou la fermeture hygiene/warnings de la tranche avant commit

### Phase 14A: Operator Truth Unification
- [x] Unifier la surface opérateur publique autour de `status`, `why`, `path`, `anomalies`.
- [x] Réaligner la documentation et le skill canonique unique sur les vrais outils MCP exposés.
- [x] Remplacer le report imprimé par `project_status`, une surface MCP live de situation projet.
- [x] Verrouiller par tests la découvrabilité publique et les contrats JSON minimaux.

### Phase 14B: Structural Diagnostics Completion
- [x] Exposer les diagnostics structurels à plus fort ROI: `wrappers`, `orphan code`, `orphan intent`.
- [x] Ajouter `feature envy`, `detours` et `abstraction detours` à la surface `anomalies`.
- [x] Inclure sévérité, confiance, provenance et action recommandée dans la surface `anomalies`.
- [x] Ajouter un score minimal de validation/sûreté aux recommandations structurelles.
- [ ] Différer historique graphe complet, couche conception explicite et propagation avancée d'incertitude.

### Phase 15: Persistent Structural Memory
- [x] Persister des snapshots structurels dérivés hors `SOLL` sous un stockage non canonique.
- [x] Exposer `snapshot_history` et `snapshot_diff` comme surfaces MCP publiques de lecture.
- [x] Brancher `project_status` sur `snapshot_id`, `generated_at` et `delta_vs_previous` basés sur ce stockage dérivé.
- [x] Requalifier ces surfaces contre le runtime live une fois le serveur revenu.

### Phase 16: Confidence And Provenance
- [x] Standardiser `provenance`, `confidence`, `evidence_sources`, `safe_to_act` et `needs_human_confirmation` sur les sorties structurales publiques.
- [x] Marquer explicitement les findings heuristiques comme heuristiques/inférés.
- [ ] Raffiner la propagation de confiance au-delà des règles discrètes v1.

### Phase 17: Derived Conception Layer
- [x] Exposer une vue `conception_view` dérivée en lecture seule (`modules`, `interfaces`, `contracts`, `flows`, frontières suspectes).
- [x] Garder cette couche hors `SOLL` et non canonique.
- [ ] Décider plus tard si une persistance dédiée de conception apporte assez de valeur pour justifier une nouvelle tranche.

### Phase 18: Validation And Change Safety
- [x] Exposer `change_safety` comme synthèse dérivée de couverture, traceability et signaux de validation.
- [x] Intégrer des guardrails de mutation lisibles par agent.
- [ ] Enrichir ultérieurement la couverture validation par cible/fichier/module sans modifier `SOLL` tant que non demandé.

## Working Assumptions
- Les modifications Git actuellement visibles sont principalement des artefacts de runtime/devenv et non un signal suffisant de travail produit.
- Toute conclusion tirée hors `devenv shell` est non fiable pour ce dépôt.
- Les documents `progress.md` et `STATE.md` peuvent surestimer le niveau réel de fermeture.

## Current Priority
1. Consolider la performance des surfaces analytiques live (`anomalies`, `project_status`, `soll_work_plan`) maintenant que la qualification runtime passe.
2. Requalifier `retrieve_context` dans un mode/runtime qui l'expose effectivement, sans rouvrir la surface publique `graph_only`.
3. Garder `SOLL` comme seule vérité canonique d'intention:
   - `soll_export` = backup réimportable
   - `project_status` = surface de pilotage live non canonique
4. Conserver `DuckDB` comme vérité de `pending/indexing/indexed` avec ingress amorti en mémoire.
5. Préserver la causalité explicite pour toute future extension du scheduler, du writer et du planneur de retrieval.
6. Observer le reclaimer mémoire idle avant toute politique plus agressive.
7. Conserver la frontière documentaire maintenant posée:
   - `docs/` = canonique
   - `docs/archive/` = historique
   - `docs/vision/` = exports live
   - `docs/archive/soll-exports/` = snapshots déplacés
8. Remplacer la logique de seuil fixe par un scheduler mémoire plus intelligent:
   - démarrage prudent par type de parser et bucket de taille tant que la confiance est faible
   - refus explicite des fichiers trop gros même seuls pour le budget courant
   - admission par lot optimisé sous budget au lieu d'un ordre FIFO naïf
9. Garder les documents de statut alignés sur la preuve runtime, pas sur des formulations aspiratoires.

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| Validation environnement échoue dans le shell courant | 1 | Rejoué via `devenv shell`, validation verte |
