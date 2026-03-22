# Plan de Stabilisation MCP Industrial-Grade (Phase Apollo)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**But :** Éliminer 100% des instabilités du serveur MCP en appliquant les décisions de l'audit Maestria (Isolation, Tunnel Rust, Priorité de Lecture, Requêtes Paramétrées).

**Architecture :**
1.  **Dual Sockets :** Séparer `/tmp/axon-telemetry.sock` (Elixir) de `/tmp/axon-mcp.sock` (IA).
2.  **MCP Tunnel (Rust) :** Créer un binaire Rust `axon-mcp-tunnel` pour remplacer le proxy Python.
3.  **Pause-on-Query :** Implémenter un verrou atomique en Rust pour mettre l'ingestion en pause lors des requêtes MCP.
4.  **Prepared Statements :** Refondre le pont Rust <-> Kuzu pour utiliser des requêtes paramétrées.

---

### Task 1: Implémentation des Dual Sockets (Rust & Elixir)

**Files:**
- Modify: `src/axon-core/src/main.rs`
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex`

**Step 1: Rust Multi-Listener**
Modifier `main.rs` pour écouter sur deux chemins UNIX. Le socket télémétrie accepte les commandes `PARSE_FILE`, le socket MCP n'accepte que du JSON-RPC.

**Step 2: Elixir Re-routing**
Mettre à jour `pool_facade.ex` pour qu'il se connecte à `/tmp/axon-telemetry.sock`.

---

### Task 2: Création du Binaire `axon-mcp-tunnel`

**Files:**
- Create: `src/axon-mcp-tunnel/src/main.rs`
- Create: `src/axon-mcp-tunnel/Cargo.toml`

**Step 1: Rust Tunnel Implementation**
Implémenter un tunnel asynchrone ultra-simple : `stdin -> socket` et `socket -> stdout`.

**Step 2: Start Script Update**
Mettre à jour `scripts/start-v2.sh` pour compiler et utiliser ce nouveau tunnel à la place de Python.

---

### Task 3: Priorité de Lecture (Ingestion Pause)

**Files:**
- Modify: `src/axon-core/src/main.rs`

**Step 1: Atomic Pause Signal**
Ajouter un `Arc<AtomicBool>` partagé. Dès qu'une donnée arrive sur le socket MCP, l'Atomic passe à `true`, forçant les threads d'ingestion à attendre (`yield_now` ou court sleep).

---

### Task 4: Requêtes Paramétrées (Kuzu Bridge)

**Files:**
- Modify: `src/axon-core/src/graph.rs`
- Modify: `src/axon-core/src/mcp.rs`

**Step 1: Bridge Update**
Modifier `GraphStore::query_json` pour accepter un `Map<String, Value>` de paramètres. Mettre à jour les fonctions MCP pour ne plus formater les chaînes mais passer les arguments via ce Map.
