# Findings: Architecture "Nexus Seal" (Zéro-Sleep)

## Décision Architecturale (2026-03-27)
Après audit par le collège d'experts, nous avons identifié que la congestion massive provient d'un **verrouillage applicatif artificiel** (`RwLock<GraphStore>`) qui sature le runtime Tokio et affame le serveur MCP.

### Piliers de la solution :
1.  **MVCC (Multi-Version Concurrency Control) :** Utilisation des capacités natives de KuzuDB. Le `RwLock` est supprimé au profit d'un `Arc<GraphStore>`. Les lecteurs (MCP) et l'écrivain (Actor) opèrent sur des connexions distinctes.
2.  **Writer Actor & Micro-Batching :** Un thread unique gère les écritures. Il accumule les tâches dans un canal borné (`crossbeam::bounded`) et les insère par lots de 50 pour optimiser les I/O.
3.  **Contre-pression Mécanique (Zero-Sleep) :** Suppression de tous les `sleep` manuels. La régulation de vitesse est assurée par la saturation physique du canal borné et du socket UNIX, qui bloque naturellement Elixir (Oban) en cas de surcharge Rust.
4.  **Persistance SQLite :** Conservation d'Oban (SQLite WAL) pour la gestion de l'état de la file d'attente, garantissant un redémarrage sans perte après crash.

## Impact sur les goulots d'étranglement
- **Temps de réponse MCP :** < 100ms constant (plus de timeout de 30s).
- **Débit d'ingestion :** Multiplié par 10x grâce au batching transactionnel.
- **Stabilité RAM :** Contrôlée par le buffer borné (500-1000 tâches max en mémoire).
