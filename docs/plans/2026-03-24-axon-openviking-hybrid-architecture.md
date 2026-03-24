# Plan d'Architecture : Hybridation Axon x OpenViking

## 1. Contexte & Problème

Axon, propulsé par Rust et KuzuDB, offre une puissance d'analyse de code brute exceptionnelle, mais souffre actuellement d'une grande instabilité en tant que serveur MCP (Model Context Protocol). 
Les problèmes identifiés sont :
- **Timeouts MCP :** Le tunnel UDS via stdio et un proxy Python gère mal le multiplexage et s'effondre sous le poids de gros JSONs. De plus, les workers d'ingestion (8 threads) écrivent massivement dans KuzuDB, créant une contention de verrous (RwLock) qui bloque les requêtes de lecture de l'IA.
- **OOM (Out Of Memory) :** Axon a une limite stricte de 16 Go de RAM. Les allocations massives sous `RwLock` et le chargement brutal du graphe font exploser la mémoire, forçant le système d'exploitation à tuer le processus.
- **Surcharge Cognitive de l'IA :** Les requêtes `axon_cypher` génèrent des réponses massives non structurées (L2 brut), polluant la fenêtre de contexte du LLM.

L'analyse du projet **OpenViking** a mis en lumière des solutions architecturales supérieures pour l'intégration de l'Agent, sans sacrifier les performances de notre moteur Rust.

## 2. Décision Architecturale : La Stratégie de la "Double Voie" (Dual-Path)

En tant qu'Architecte Lead, j'impose la refonte suivante pour surpasser OpenViking tout en respectant l'enveloppe de 16 Go de RAM. L'architecture va s'orienter vers :

### Phase 1 : Stabilisation Système (Actor Model & SSE)
- **Remplacement de stdio par HTTP/SSE :** Abandon du tunnel UDS et du proxy Python (`mcp-stdio-proxy.py`). Le binaire Rust `axon-core` implémentera un serveur SSE natif (via `axum` ou `tokio-mcp`) assurant une gestion robuste des flux asynchrones et du framing JSON-RPC.
- **Démantèlement du `RwLock` Global :** Utilisation de l'architecture MVCC interne de KuzuDB. Le serveur MCP obtiendra une `Connection` dédiée en lecture seule, le séparant des écritures.
- **Modèle Acteur pour l'Ingestion (Micro-batching) :** Les 8 workers ne touchent plus directement KuzuDB. Ils deviennent de purs générateurs d'AST et envoient leurs graphes dans un Ring Buffer borné (channel MPSC). Un `Writer Actor` unique lira ce channel et appliquera les écritures par lots, évitant le verrouillage et respectant le plafond RAM.

### Phase 2 : Hybridation RAG (L0/L1/L2)
- Implémentation du **"File System Paradigm"** inspiré d'OpenViking. Axon exposera deux jeux d'outils complémentaires :
    - **La Voie "Power User" :** Maintien de `axon_cypher`, `axon_impact`, etc. pour les analyses transversales de sécurité et d'architecture.
    - **La Voie "Agent DX" (Directory eXplorer) :** Création d'outils simulant un système de fichiers virtuel pour une navigation parcimonieuse en tokens :
        - `axon_fs_ls(path)` : L0 (Résumé abstrait du module/dossier).
        - `axon_fs_abstract(symbol)` : L1 (Vue d'ensemble avec dépendances et doc).
        - `axon_fs_read(symbol)` : L2 (Code source brut chargé uniquement à la demande).

## 3. Déroulement du Cycle (TDD Strict)

1. **Mise en place de l'infrastructure SSE :** Refonte de `src/axon-core/src/mcp.rs` pour intégrer un transport réseau non bloquant. (Tests d'intégration réseau d'abord).
2. **Implémentation de l'Actor Model (Ring Buffer) :** Refonte de `src/axon-core/src/worker.rs` et `src/axon-core/src/main.rs`. (Tests unitaires de concurrence).
3. **Implémentation du File System Paradigm :** Ajout des nouveaux outils MCP L0/L1/L2. (Tests unitaires de génération d'abstracts).
4. **Validation et Benchmarking :** Lancer l'ingestion massive tout en bombardant le serveur MCP de requêtes, pour prouver qu'il n'y a plus aucun timeout et que la RAM reste sous 16 Go.
