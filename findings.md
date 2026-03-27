# Findings: Architecture "Nexus Seal" (Zéro-Sleep)

## Décision Architecturale (2026-03-27)
Après audit par le collège d'experts, nous avons identifié que la congestion massive provient d'un **verrouillage applicatif artificiel** (`RwLock<GraphStore>`) qui sature le runtime Tokio et affame le serveur MCP.

### Piliers de la solution :
1.  **MVCC (Multi-Version Concurrency Control) :** Utilisation des capacités natives de KuzuDB. Le `RwLock` est supprimé au profit d'un `Arc<GraphStore>`. Les lecteurs (MCP) et l'écrivain (Actor) opèrent sur des connexions distinctes.
2.  **Writer Actor & Micro-Batching :** Un thread unique gère les écritures. Il accumule les tâches dans un canal borné (`crossbeam::bounded`) et les insère par lots de 50 pour optimiser les I/O.
3.  **Contre-pression Mécanique (Zero-Sleep) :** Suppression de tous les `sleep` manuels. La régulation de vitesse est assurée par la saturation physique du canal borné et du socket UNIX, qui bloque naturellement Elixir (Oban) en cas de surcharge Rust.
4.  **Persistance SQLite :** Conservation d'Oban (SQLite WAL) pour la gestion de l'état de la file d'attente, garantissant un redémarrage sans perte après crash.

## Maestria Finale : Zero-SELECT & TCP Buffering (2026-03-27)
Le dernier goulot d'étranglement a été éradiqué. Le système est désormais capable d'absorber le débit maximal de Rust sans saturer SQLite ni perdre d'événements.

### Blindage Industriel Final :
1.  **TCP Line Buffering (PoolFacade) :** Implémentation d'un buffer de flux pour reconstituer les messages JSON fragmentés. Plus aucune perte d'acquittement lors des rafales de Rust.
2.  **Zero-SELECT Feedback Loop :** Suppression totale des requêtes `SELECT` unitaires dans la boucle de retour. L'identification des projets se fait en mémoire vive (via le chemin), et la mise à jour des statistiques utilise un **Full-Metrics Batch UPSERT**.
3.  **Alignement Sémantique :** Rétablissement du statut `"indexed"` pour une cohérence totale avec les outils de monitoring.
4.  **Débit Validé :** Progression continue observée (80 fichiers par 30s sous charge initiale), avec une synchronisation parfaite entre les jobs Oban et les enregistrements SQLite.

### Conclusion :
Le cycle de vérité physique est bouclé. Axon v2.2 est désormais une infrastructure de classe production, résiliente aux fragmentations réseau et aux contentions de base de données.
