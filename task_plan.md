# Axon - Industrialization & Daemon Robustness

## Goal
Sécuriser le démarrage des processus (Daemon Guard) via des PID locks, découpler le LiveView du processus d'indexation (BridgeClient devient le Master autonome), et implémenter des contrôles complets (START, STOP, RESET DB) synchronisés en temps réel.

## Phases

### Phase 1: Daemon Startup Guard (Script de lock) (COMPLETED)
- [x] Créer le dossier `.axon/run/` si inexistant.
- [x] Modifier `scripts/start-v2.sh` pour utiliser des fichiers PID (`.axon/run/rust.pid`, `.axon/run/elixir.pid`).
- [x] Empêcher le lancement si les processus tournent déjà.
- [x] Nettoyer automatiquement les sockets orphelins (ex: `/tmp/axon-v2.sock`) en cas de crash précédent.

### Phase 2: BridgeClient GenServer Refactor (Master) (COMPLETED)
- [x] Déplacer la responsabilité du déclenchement du scan initial depuis le LiveView vers le `BridgeClient`.
- [x] Ajouter une gestion d'état dans `BridgeClient` (`:idle`, `:indexing`).
- [x] Implémenter les handlers pour `START`, `STOP`, et `RESET` et relayer ces commandes au moteur Rust.
- [x] Notifier le LiveView des changements d'état (PubSub).

### Phase 3: Rust Backend (STOP & RESET DB) (COMPLETED)
- [x] Modifier `src/axon-core/src/main.rs` pour intercepter `STOP` et `RESET`.
- [x] Implémenter la logique `STOP` (interrompre la boucle de scan `for project in project_dirs`).
- [x] Implémenter la logique `RESET` (fermer proprement KuzuDB, supprimer `.axon/graph_v2/lbug.db`, et recréer `GraphStore`).

### Phase 4: LiveView Controls & UI (COMPLETED)
- [x] Retirer le `trigger_initial_scan` du `mount` dans `StatusLive`.
- [x] Ajouter les boutons `[ START / RESYNC ]`, `[ STOP ]`, `[ RESET DB ]` au panneau de contrôle supérieur.
- [x] Désactiver/griser les boutons dynamiquement selon l'état du daemon (`status: :idle` vs `:indexing`).
- [x] Connecter ces boutons aux nouvelles commandes de `BridgeClient`.
