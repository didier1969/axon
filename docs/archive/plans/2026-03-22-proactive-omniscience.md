# Plan d'Exécution Maestria : L'Omniscience Proactive

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**But :** Transformer le serveur MCP en un Oracle capable de fournir des rapports de décision structurés et de naviguer dans un graphe global cross-projets.

**Architecture :** 
1.  **Synthèse Sémantique :** Refonte des handlers de réponse dans `mcp.rs` pour formater le JSON KuzuDB en Markdown structuré.
2.  **Fédération Cross-Projets :** Modification des requêtes Cypher pour utiliser des recherches de chemins (`MATCH path = ...`) sans contrainte de projet exclusif.
3.  **Notifications Proactives :** Système de Heartbeat enrichi sur le canal de télémétrie du socket UNIX.

**Pile Technologique :** Rust, KuzuDB (Cypher), MCP Protocol, Tokio.

---

### Task 1: Refonte de `axon_inspect` (Synthèse Sémantique)

**Files:**
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Write the failing test**
Créer un test unitaire qui simule un appel à `axon_inspect` et vérifie que la réponse contient des sections Markdown (ex: "### Détails du Symbole") plutôt qu'une liste JSON brute `[["name", "kind", ...]]`.

**Step 2: Implement Semantic Formatting**
Modifier `fn axon_inspect` pour itérer sur les résultats JSON et construire une chaîne Markdown propre.

---

### Task 2: Globalisation du Graphe (Fédération Cross-Projets)

**Files:**
- Modify: `src/axon-core/src/graph.rs`
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Suppression des filtres restrictifs**
Modifier les fonctions de recherche (`axon_query`, `axon_audit`) pour que l'argument `project` devienne optionnel. Si omis, la requête Cypher doit s'exécuter sur l'intégralité du nœud `File`.

**Step 2: Test de jointure cross-projets**
Vérifier qu'une requête Cypher peut trouver une relation entre un fichier de `/projects/axon` et un fichier de `/projects/MetaGPT`.

---

### Task 3: Système de Notifications Proactives

**Files:**
- Modify: `src/axon-core/src/main.rs`
- Modify: `scripts/mcp-stdio-proxy.py`

**Step 1: Canal de notifications asynchrones**
Implémenter un mécanisme permettant au thread principal Rust d'envoyer des messages JSON-RPC de type `notifications` (sans ID) sur le socket UNIX de manière spontanée (ex: lors de la détection d'une dérive architecturale).

**Step 2: Proxy STDIO enrichi**
Mettre à jour le proxy Python pour qu'il route ces notifications vers `stderr` sans corrompre le flux `stdout` utilisé par l'agent IA.

---

### Task 4: Finalisation de l'Analyse d'Impact (`axon_impact`)

**Files:**
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Calcul de profondeur variable**
Optimiser `axon_impact` pour utiliser les capacités de Kuzu sur les longueurs de chemins variables (`*1..5`) et renvoyer un "Rayon d'Impact" (nombre de composants affectés).
