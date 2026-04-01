# Architecture Axon v1.0 : Le Modèle Triple-Pod Découplé

## 1. Vision : "Separation of Concerns for Scalability"
La v1.0 fragmente les responsabilités en trois unités autonomes (Pods) pour garantir une performance maximale et une résilience totale.

## 2. La Triade des Pods

### Pod A : Axon Watcher (The Orchestrator - Elixir/OTP)
*   **Responsabilité :** Surveillance active du système de fichiers (FS Events).
*   **Rôle :** Maître de l'esclave Python (Pod B). Il décide quand parser.
*   **Flux :** Détecte un changement -> Demande un parsing au Pod B -> Pousse le résultat au Pod C.
*   **Avantage :** Découple la détection de la persistence. Gère la file d'attente d'ingestion.

### Pod B : Axon Parser (The Slave - Python/Tree-sitter)
*   **Responsabilité :** Transformation de texte en structure (AST).
*   **Rôle :** Esclave du Pod A via ErlPort.
*   **Caractéristique :** Stateless, purement calculatoire (CPU-bound).
*   **Avantage :** Peut être répliqué si plusieurs cœurs sont disponibles pour le parsing.

### Pod C : HydraDB (The Source of Truth - Elixir/RocksDB/Dolt)
*   **Responsabilité :** Persistence, Versionnage et Graph Query.
*   **Rôle :** Reçoit les batchs de symboles du Pod A.
*   **Caractéristique :** I/O-bound, garantit l'atomicité des commits (Dolt).
*   **Avantage :** Centralise la connaissance pour tous les outils (MCP, CLI, WebUI).

## 3. Flux de Communication (The Event Pipe)
`OS Event` -> **[Pod A]** --(ErlPort)--> **[Pod B]** --(Symbols)--> **[Pod A]** --(Batch)--> **[Pod C]**

## 4. Bénéfices pour le Développeur
*   **Zéro Latence :** Le Pod A peut travailler en arrière-plan sans impacter les performances de lecture du Pod C.
*   **Robustesse :** Le crash d'un Pod n'entraîne pas la chute des autres.
*   **Flexibilité :** Chaque brique est remplaçable indépendamment.
