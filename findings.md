# Findings: Architecture "Nexus Seal" (Zéro-Sleep)

## Décision Architecturale (2026-03-27)
Après audit par le collège d'experts, nous avons identifié que la congestion massive provient d'un **verrouillage applicatif artificiel** (`RwLock<GraphStore>`) qui sature le runtime Tokio et affame le serveur MCP.

### Piliers de la solution :
1.  **MVCC (Multi-Version Concurrency Control) :** Utilisation des capacités natives de KuzuDB. Le `RwLock` est supprimé au profit d'un `Arc<GraphStore>`. Les lecteurs (MCP) et l'écrivain (Actor) opèrent sur des connexions distinctes.
2.  **Writer Actor & Micro-Batching :** Un thread unique gère les écritures. Il accumule les tâches dans un canal borné (`crossbeam::bounded`) et les insère par lots de 50 pour optimiser les I/O.
3.  **Contre-pression Mécanique (Zero-Sleep) :** Suppression de tous les `sleep` manuels. La régulation de vitesse est assurée par la saturation physique du canal borné et du socket UNIX, qui bloque naturellement Elixir (Oban) en cas de surcharge Rust.
4.  **Persistance SQLite :** Conservation d'Oban (SQLite WAL) pour la gestion de l'état de la file d'attente, garantissant un redémarrage sans perte après crash.

## Résolution du Deadlock I/O (2026-03-27)
Un blocage critique a été identifié lors de l'ingestion massive : le GenServer Elixir `PoolFacade` se bloquait sur l'envoi vers le socket UNIX saturé, l'empêchant de lire les acquittements de Rust.

### Mesures Correctives :
1.  **I/O Decoupling (Elixir) :** Refonte de `PoolFacade` pour utiliser des `Task.start` lors des envois. Le GenServer reste ainsi disponible pour traiter les messages TCP entrants en priorité haute.
2.  **Buffer Scaling (Rust) :** Augmentation de la `QueueStore` de 500 à **50 000 slots**. Cela permet d'absorber les rafales d'Oban sans saturer immédiatement le buffer du noyau.
3.  **Résultat :** L'ingestion est désormais fluide et continue (vérifiée par l'incrémentation du compteur `indexed_files`). Le système est immunisé contre les deadlocks de backpressure.
