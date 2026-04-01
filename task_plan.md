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

## Working Assumptions
- Les modifications Git actuellement visibles sont principalement des artefacts de runtime/devenv et non un signal suffisant de travail produit.
- Toute conclusion tirée hors `devenv shell` est non fiable pour ce dépôt.
- Les documents `progress.md` et `STATE.md` peuvent surestimer le niveau réel de fermeture.

## Current Priority
1. Réduire l'autorité résiduelle Elixir encore présente dans `src/dashboard/lib/axon_nexus/axon/watcher/*` et `src/dashboard/lib/axon_nexus/axon/backpressure_controller.ex`.
2. Conserver la frontière documentaire maintenant posée:
   - `docs/` = canonique
   - `docs/archive/` = historique
   - `docs/vision/` = exports live
   - `docs/archive/soll-exports/` = snapshots déplacés
3. Remplacer la logique de seuil fixe par un scheduler mémoire plus intelligent:
   - démarrage prudent par type de parser et bucket de taille tant que la confiance est faible
   - refus explicite des fichiers trop gros même seuls pour le budget courant
   - admission par lot optimisé sous budget au lieu d'un ordre FIFO naïf
4. Garder les documents de statut alignés sur la preuve runtime, pas sur des formulations aspiratoires.

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| Validation environnement échoue dans le shell courant | 1 | Rejoué via `devenv shell`, validation verte |
