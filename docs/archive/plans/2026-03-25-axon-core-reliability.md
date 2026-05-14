# Plan d'Ingénierie : Fiabilité Absolue (Éradication des Panics & Deadlocks)

## 1. Contexte & Problème

Suite à l'audit de performance et de fiabilité du projet `axon`, de graves failles architecturales ont été identifiées dans le Data Plane (Rust). L'utilisation massive et non-sécurisée de `.unwrap()`, `.expect()` et de `.lock().unwrap()` dans les threads asynchrones expose le démon central à des panics irrécupérables.
Ces panics détruisent la confiance (Agent DX) et forcent un redémarrage complet du moteur, ce qui est inacceptable pour un standard industriel.
De plus, le module `mcp_http.rs` contient une dette technique (`TODO`) concernant la gestion des SSE (Server-Sent Events) qui peut perturber les agents MCP qui attendent une vraie connexion persistante.

## 2. Décision Architecturale : Résilience par Pattern Matching

En tant qu'Architecte Lead, j'ordonne la refonte suivante pour atteindre une fiabilité de type "Zero-Downtime" :

### Phase 1 : Sécurisation du `GraphStore` et de l'Accès MCP (Red -> Green -> Refactor)
- **Cible :** `src/axon-core/src/mcp.rs`
- Remplacer tous les appels `self.graph_store.read().unwrap()` par des `match` explicites pour gérer le `PoisonError`. Si le lock est empoisonné, le serveur MCP doit retourner une réponse HTTP/JSON-RPC propre indiquant que le système de graphe doit être réinitialisé, plutôt que de crasher le serveur Axum tout entier.

### Phase 2 : Isolation et Protection du Moteur de Parsing (Workers & Acteurs)
- **Cible :** `src/axon-core/src/worker.rs`
- Éradiquer les `unwrap()` lors de la lecture des messages (`rx.recv()`) et de la sérialisation JSON (`serde_json::to_string(&...).unwrap()`).
- Protéger le pipeline d'ingestion contre les erreurs de lecture de fichiers ou de parsing, en utilisant des fallback silencieux (log dans `/tmp/axon_forensic.log`) plutôt que d'interrompre l'immortalité du worker.

### Phase 3 : Sécurisation de l'IA Embarquée (FastEmbed ONNX)
- **Cible :** `src/axon-core/src/embedder.rs`
- Protéger le verrou global statique (`EMBEDDER.lock().unwrap()`). La perte de ce verrou (si ONNX crashe) paralyse la génération vectorielle. Il doit être géré avec un "poison-recovery" pour recréer le cache dynamiquement.

### Phase 4 : Conformité MCP SSE
- **Cible :** `src/axon-core/src/mcp_http.rs`
- Résoudre le `TODO` de la ligne 46. Implémenter une vraie boucle de maintien de vie (keep-alive) ou un mécanisme de stream persistant correct, au lieu d'une simple émission statique d'un `Infallible` channel, afin que la connexion HTTP/SSE avec l'agent soit formellement validée.

## 3. Déroulement du Cycle (TDD Strict)
Pour chaque phase, la compilation et les tests doivent rester verts. Aucune régression n'est tolérée.
