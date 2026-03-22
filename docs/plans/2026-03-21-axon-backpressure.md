# Axon Data Plane: Backpressure & Thread Starvation Mitigation

## Problème
Lors de l'ingestion massive (redémarrage), l'orchestrateur Elixir envoie des milliers de requêtes `PARSE_FILE` via UDS en quelques secondes. 
Pour chaque fichier, le serveur Rust utilise `tokio::task::spawn_blocking` pour exécuter WebAssembly (Tree-sitter) et FastEmbed. 
Cela sature la totalité du pool de threads bloquants de Tokio. Par conséquent, les requêtes MCP critiques pour les LLM (`axon_health`, `axon_query`) qui nécessitent aussi `spawn_blocking` (pour interroger KuzuDB) sont reléguées au fond de la file d'attente et finissent en Timeout (Time to First Byte > 10s).

## Solution : Sémaphore de Concurrence
Implémenter un `tokio::sync::Semaphore` global dans le point d'entrée Rust (`main.rs`) pour limiter la concurrence CPU-bound.
Le serveur acceptera toujours les connexions UDS instantanément, mais l'étape de parsing WASM ne pourra s'exécuter que si elle obtient un permis du sémaphore. 

- **Capacité du Sémaphore :** Fixée à 8 ou 16 (équivalent au nombre de cœurs CPU).
- **Effet :** Le pool de threads Tokio restera majoritairement libre. Les requêtes MCP s'exécuteront immédiatement, car elles ne demanderont pas de permis du sémaphore d'ingestion. La charge CPU sera lissée (Backpressure).

## Étapes
1. Ajouter `use tokio::sync::Semaphore;` dans `main.rs`.
2. Initialiser `let parse_semaphore = Arc::new(Semaphore::new(16));`.
3. Dans la branche `PARSE_FILE`, ajouter `let _permit = parse_semaphore_clone.acquire().await.unwrap();` avant de lancer le `spawn_blocking`.
4. Le permis sera relâché automatiquement à la fin de la tâche.