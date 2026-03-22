# Axon Data Plane: Read Priority & Ingestion Pause

## Problème
L'agent LLM (Gemini) fait face à des timeouts lors de l'appel d'outils MCP, car la base de données (KuzuDB) est monopolisée par l'ingestion massive de fichiers via Elixir. La solution de "Lock Mitigation" (sleep) ne suffit pas car KuzuDB bloque les lectures en attente du write lock de manière agressive, ou parce que le thread de l'agent tombe quand même pendant un cycle d'écriture lourd.

## Solution Cible : Interruption Collaborative (Priority Reads)
L'agent LLM (utilisateur) doit avoir la **priorité absolue** sur l'ingestion (background).
Lorsqu'une requête MCP (lecture) arrive :
1. Le gestionnaire de commandes bascule un "Drapeau de Pause" (AtomicBool).
2. Le `GraphWriter` (qui ingère les batchs) vérifie ce drapeau avant chaque écriture. S'il est activé, le writer se met en pause et attend.
3. La requête MCP s'exécute avec la garantie d'obtenir le verrou KuzuDB instantanément.
4. Une fois la requête MCP terminée, le drapeau de pause est désactivé, et le `GraphWriter` reprend son ingestion.

## Étapes d'Implémentation
1.  **Shared State :** Créer un `Arc<AtomicBool>` nommé `pause_ingestion` dans `main.rs`.
2.  **Thread Writer :** Passer ce drapeau au `spawn_graph_writer`. Dans la boucle `loop`, si `pause_ingestion` est `true`, le writer fait un `tokio::time::sleep` et `continue` (skip le flush).
3.  **Thread MCP :** Dans `bridge_handler.rs`, lorsque la commande commence par `{` (requête JSON-RPC du LLM), définir `pause_ingestion` sur `true`.
4.  Attendre que le `RwLock` soit libéré (l'écriture en cours se terminera, mais la prochaine ne commencera pas).
5.  Exécuter la requête `handle_request()`.
6.  Remettre `pause_ingestion` sur `false`.