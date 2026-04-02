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
- [ ] Définir ensuite une expérimentation prudente sur purge/trim/checkpoint/allocateur.

### Phase 8: Causalité `pending`
- [x] Ajouter une première vérité persistée sur la cause de retour en `pending`.
- [ ] Couvrir toutes les transitions `pending/indexing/indexed/...` avec une causalité canonique complète.
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

## Working Assumptions
- Les modifications Git actuellement visibles sont principalement des artefacts de runtime/devenv et non un signal suffisant de travail produit.
- Toute conclusion tirée hors `devenv shell` est non fiable pour ce dépôt.
- Les documents `progress.md` et `STATE.md` peuvent surestimer le niveau réel de fermeture.

## Current Priority
1. Introduire un tampon mémoire d’ingress entre `Watcher/Scanner` et `DuckDB` pour séparer découverte brute et décision canonique.
2. Garder `DuckDB` comme vérité de `pending/indexing/indexed` et pousser uniquement des batchs canoniques depuis l’ingress buffer.
3. Finir ensuite la causalité complète de la machine d’état une fois l’ingress stabilisé.
4. Ouvrir ensuite une expérimentation mémoire prudente sur la base des mesures `RssAnon`, pas avant.
5. Conserver la frontière documentaire maintenant posée:
   - `docs/` = canonique
   - `docs/archive/` = historique
   - `docs/vision/` = exports live
   - `docs/archive/soll-exports/` = snapshots déplacés
6. Remplacer la logique de seuil fixe par un scheduler mémoire plus intelligent:
   - démarrage prudent par type de parser et bucket de taille tant que la confiance est faible
   - refus explicite des fichiers trop gros même seuls pour le budget courant
   - admission par lot optimisé sous budget au lieu d'un ordre FIFO naïf
7. Garder les documents de statut alignés sur la preuve runtime, pas sur des formulations aspiratoires.

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| Validation environnement échoue dans le shell courant | 1 | Rejoué via `devenv shell`, validation verte |
