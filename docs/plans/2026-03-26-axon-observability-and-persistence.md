# Plan d'Architecture : Observabilité, Timeouts et Persistance (Anti-Deadlock)

## 1. Contexte & Problème

L'architecture actuelle souffre de blocages silencieux (Deadlocks). L'utilisation de verrous bloquants `RwLock::read()` et `RwLock::write()` sans limite de temps conduit à une paralysie de l'infrastructure si un thread (comme un worker traitant un gros fichier) maintient le verrou trop longtemps. L'absence de logs détaillés (observabilité) nous empêche de diagnostiquer ces blocages en production. De plus, la tentative de tout traiter en mémoire vive (canaux unbounded) menace la stabilité du système (limite de 16 Go de RAM).

## 2. Décisions Architecturales

En tant que Lead Architect, j'impose les trois piliers suivants :

### Phase 1 : Observabilité (Tracing)
- Remplacement des logs basiques (`log::info!`) par la crate `tracing`.
- Chaque opération critique (parsing, accès DB, appels MCP) sera tracée avec son ID de thread et son contexte, permettant d'identifier immédiatement où le système bloque.

### Phase 2 : Verrous à Délai d'Attente (Watchdog)
- Plus aucun verrouillage infini. Les appels `graph_store.read()` et `graph_store.write()` seront encadrés par des timeouts stricts (ex: 5 secondes).
- Si un thread n'obtient pas le verrou dans le temps imparti, il abandonne l'opération, logue une erreur détaillée via le système de tracing, et libère ses ressources, évitant ainsi un deadlock en cascade.

### Phase 3 : Persistance de la File d'Attente (Fin du "Tout en RAM")
- Remplacement des canaux asynchrones massifs (unbounded) par une base de données de progression persistante (SQLite `tasks.db`).
- Le `Scanner` insérera les fichiers découverts dans cette base (Statut `PENDING`).
- Les `Workers` piocheront les tâches une par une, garantissant une empreinte RAM minimale et constante, et assurant la reprise sur erreur (aucun fichier perdu).

## 3. Déroulement du Cycle (TDD Strict)

1. **Intégration de `tracing`** : Ajout des dépendances et refonte de l'initialisation du logger dans `main.rs`.
2. **Implémentation des Timeouts** : Ajout de méthodes `try_read_for` et `try_write_for` dans un wrapper autour du `GraphStore` (ou via une crate comme `parking_lot`). Refonte des accès dans `mcp.rs` et `worker.rs`.
3. **Mise en place de SQLite** : Création d'un module `queue.rs` pour la gestion des tâches persistantes et adaptation des workers.
4. **Validation** : Preuve que le système ne bloque plus lors de requêtes concurrentes massives.
