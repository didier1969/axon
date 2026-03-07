# Spécification Macro & Micro - Axon v1.0

## 1. Vision Macro : Le Cycle "Pulse-to-Graph"

L'objectif macro est la **synchronisation instantanée et fidèle** de la connaissance.

### Scénario Nominal (E2E Flow)
1.  **Pulse (Trigger) :** L'utilisateur modifie `auth_service.py`.
2.  **Detection (Pod A - Watcher) :** Le watcher Elixir capte l'événement `modified`. Il calcule le hash du fichier. Si le hash a changé, il verrouille le fichier pour traitement.
3.  **Transformation (Pod B - Parser) :** Pod A envoie le contenu au Pod B (Python). Pod B renvoie la liste des symboles et des relations.
4.  **Ingestion (Pod C - HydraDB) :** Pod A pousse les symboles vers le Pod C. HydraDB effectue un commit atomique via Dolt.
5.  **Audit (Lazy-Deep) :** HydraDB notifie les abonnés (MCP Server) que la structure est à jour. En tâche de fond, il lance l'analyse de flux profonde.

---

## 2. Vision Micro : Les Contrats d'Interface

### A. Flux Direct : Pod A (Watcher) ↔ Pod C (HydraDB)
*   **Responsabilité :** Gestion de la structure physique (FileSystem).
*   **Messages :**
    - `{:file_detected, path, hash, size}` -> Création/Update du nœud `File`.
    - `{:file_deleted, path}` -> Suppression immédiate (Pruning) de l'arbre associé au chemin.
    - `{:sync_status, :start | :complete}` -> Gestion de l'état global.
*   **Optimisation :** Si le hash envoyé par A correspond au hash stocké dans C, le cycle s'arrête (Short-circuit).

### B. Flux Transformé : Pod A (Watcher) → Pod B (Parser) → Pod C (HydraDB)
*   **Responsabilité :** Gestion de la structure logique (Intelligence).
*   **Flux :**
    1. Pod A lit le contenu (si hash divergent).
    2. Pod A envoie `{:parse, path, content}` au Pod B.
    3. Pod B renvoie `symbols_list`.
    4. Pod A pousse `symbols_list` au Pod C via `add_nodes_batch`.

## 3. Optimisation & Distribution (The Industrial Layer)

### A. Stratégie de Batching (Pod A ↔ Pod B)
Pour éviter la surcharge de processus, le Pod A ne traite pas les fichiers un par un lors d'un scan massif.
*   **Batch Size :** Les requêtes de parsing sont regroupées par paquets de **50 fichiers**.
*   **Message :** `{:parse_batch, [{path, content}, ...]}`.
*   **Timeout :** Un batch a un timeout de 30s. En cas d'échec, le Pod A divise le batch en deux pour isoler le fichier problématique (Binary Search Pruning).

### B. Gestion de la Charge (Back-pressure)
Le Pod A surveille la charge du Pod B.
*   **Worker Pool :** Le Pod A peut lancer jusqu'à `N` instances du Pod B (où `N = nombre de cœurs CPU`).
*   **Queue :** Si tous les workers sont occupés, les événements de modification sont empilés dans une file d'attente prioritaire (FIFO).

### C. Consistance Graphe (The Dolt Strategy)
Pour garantir que l'utilisateur ne voit jamais un état intermédiaire :
1.  **Branching :** Chaque session d'ingestion massive s'effectue sur une branche Dolt isolée : `ingest/[UUID]`.
2.  **Validation :** Une fois le parsing terminé, le Pod A demande au Pod C de valider l'intégrité du graphe sur cette branche.
3.  **Atomic Merge :** Si la validation réussit, la branche est mergée dans `main` de manière atomique.

---

## 4. Herméticité Nix (The Native Bridge)
Le projet Axon v1.0 n'est parfait que s'il est reproductible.
*   **Input Flake :** HydraDB est importé comme `github:didier1969/hydradb`.
*   **Unified Shell :** Le `devShell` de Nix expose à la fois `python` avec Tree-sitter et `elixir` avec le runtime Astral.
*   **Native Link :** Le Pod A (Elixir) invoque le Pod B via l'exécutable Python garanti par le `store` Nix, éliminant tout conflit de version.

---

## 5. Critères d'Acceptation (Definition of Done)
1.  **Latence :** Un changement de fichier doit être répercuté dans le graphe en moins de **100ms** (hors embeddings).
2.  **Batching :** Le système doit être capable d'ingérer 1000 fichiers sans faire exploser la consommation mémoire du worker Python.
3.  **Consistance :** Une requête MCP lancée pendant une ingestion doit renvoyer les données du dernier commit stable (Isolation).
