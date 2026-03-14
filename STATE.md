# État du Projet : Axon v1.0 (Triple-Pod Ready)

## Référence Projet
**Vision :** Copilote Architectural distribué basé sur le modèle Triple-Pod.
**Statut :** 🔵 TERMINÉ (Migration HydraDB validée).

## Architecture v1.0
- **Pod A (Watcher) :** Orchestration Elixir/OTP.
- **Pod B (Parser) :** Analyse Python Stateless via MsgPack (Extraits : Symboles + Relations).
- **Pod C (HydraDB) :** persistence, persistence Atomique (Dolt) et Graph Intelligence.

## MCP v1.1
- **Upgrade SDK :** Utilisation de `mcp[server]>=1.2.1`.
- **Nouveau Tool :** `axon_audit` pour l'audit de sécurité OWASP délégué.
- **Factory Pattern :** Serveur refactorisé pour une meilleure testabilité.

## MCP v1.2
- **Consolidation API :** Réduction de 17 à 8 outils haute performance.
- **Outils "Vue 360" :** Implémentation de `axon_inspect` (Fusion Code/Graphe/Stats).
- **Optimisation Contexte :** Gain de ~40%% d'\''espace pour les prompts IA.
- **Santé Globale :** Nouveau tool `axon_health` unifiant le diagnostic.

## MCP v1.3
- **Async-Native :** Passage au mode asynchrone complet pour le serveur MCP.
- **Support Notifications :** Activation des notifications de changement d'\''outils et de ressources.
- **Robustesse :** Amélioration de la gestion des erreurs et de l'\''initialisation du stockage.

## Correctifs Critiques
- **Crash Terminal :** RÉSOLU par la délégation de l'Audit au Pod C (Suppression du BFS local).
- **Connectivité :** `AstralBackend` implémenté comme client TCP/MsgPack réel.

## Axon v2.0 (Immune & Autonomous)
- **Priority Streaming Scanner (APSS)** : Scan disque asynchrone (NIF Rust multithreadé) avec priorité sémantique (README/Config d'abord).
- **Auto-Trigger Indexing** : Le Watcher déclenche l'indexation dès le démarrage sans intervention humaine.
- **Resilient Bridge** : Restauration du pont UDS Pod A -> Pod B pour l'indexation granulaire en temps réel.
- **Nix DevEnv Isolation** : Environnement de développement reproductible et isolé (BEAM cookies, CARGO cache).
- **WSL Auto-Resurrection** : Lancement automatique et garde-fou via `.bashrc` et script de monitoring (gestion robuste par sessions **TMUX**).
- **Industrial Persistence** : Utilisation de `setsid` et TMUX pour garantir que les services survivent à la fermeture du terminal utilisateur.

## Loop Position
```
AUTO-SCAN ──▶ PRIORITY-INDEX ──▶ LIVE-COCKPIT
    ●              ●                ●     [v2.0 Autonome]
```
