# Spécification Technique Axon v3.0 : Nexus Pull

Cette documentation définit l'architecture souveraine **Nexus Pull**, conçue pour l'excellence opérationnelle et la transparence sémantique totale.

## 1. Vision Holistique
Le système Axon transforme une base de code brute en un graphe de connaissance vivant. L'architecture v3.0 abandonne le modèle de file d'attente aveugle (Push) pour un modèle de synchronisation déterministe (Pull) où chaque cœur CPU est orchestré individuellement.

---

## 2. Le Graphe comme Source de Vérité (SSOT)
KuzuDB devient le pivot central. Il n'est plus seulement le réceptacle des résultats, mais le gestionnaire d'état de l'ingestion.

### Ontologie des Nœuds `File`
Dès la phase de scan, chaque fichier est représenté par un nœud avec un cycle de vie d'état complet :
- **`pending`** : Découvert, en attente d'allocation.
- **`indexing`** : Verrouillé par un Agent Elixir spécifique.
- **`indexed`** : Parsing AST et embeddings AI terminés.
- **`error`** : Échec après $N$ tentatives.
- **`titan_ignored`** : Fichier trop volumineux (>1MB), traité en mode "structure seule".

**Propriétés Étendues :**
- `path`: STRING (Primary Key)
- `size`: INT64 (Taille physique en octets)
- `priority`: INT64 (Score 0-100 pour le Traffic Shaping)
- `mtime`: INT64 (Dernière modification)
- `worker_id`: INT64 (ID du cœur en charge)

---

## 3. Gestion Spécifique des Flux (Traffic Shaping)

### Priorisation Intelligente
Le `Kuzu.Guardian` utilise les propriétés du graphe pour arbitrer le travail :
1. **Fichiers Hot (Priorité > 80) :** Fichiers requis par une tâche LLM active.
2. **Backlog Normal :** Trié par `priority` décroissante.
3. **Requête Cypher de Pull :**
   ```cypher
   MATCH (f:File {status: 'pending'}) 
   RETURN f.path ORDER BY f.priority DESC LIMIT $slots
   ```

### Le Protocole "Titan" (Gros fichiers)
Les fichiers volumineux sont identifiés dès le scan initial par Rust.
- **Filtrage Kuzu :** Les fichiers > 1MB reçoivent une priorité basse ou le statut `titan_ignored` pour préserver les ressources CPU/RAM.
- **Visibilité LLM :** Même si un fichier est trop gros pour être parsé en profondeur, son existence reste enregistrée dans KuzuDB. Le LLM peut ainsi informer l'utilisateur : *"Je vois le fichier X, mais il est trop volumineux pour une analyse sémantique complète."*

---

## 4. Orchestration Control/Data Plane

### Niveau Elixir (Control Plane) : Le Guardian
Le `Kuzu.Guardian` est le chef d'orchestre. Il surveille la disponibilité des cœurs CPU et "tire" le travail de KuzuDB.
- **Trémie Active (Buffer ETS) :** Le Guardian maintient une trémie de **100 fichiers** (taille optimisée pour la réactivité) en RAM. Cette taille réduite garantit que les fichiers urgents (Hot Path) ne sont pas bloqués par un backlog trop important déjà chargé en mémoire vive.

### Niveau Rust (Data Plane) : L'Arène CPU
Le Data Plane est divisé en workers isolés. Chaque worker est un esclave direct d'un Agent Elixir.
- **Handshake de Session :** À chaque boot, Elixir envoie un `SESSION_ID`. Rust purge sa mémoire pour garantir une synchronisation parfaite.
- **Annulation Coopérative (SIG_ABORT) :** En cas de timeout (déterminé dynamiquement par Elixir, défaut 10s), Elixir envoie un signal d'arrêt. Le thread Rust vérifie ce signal entre le parsing et l'embedding.
- **Intégrité de l'AST :** Le parsing par Tree-sitter est toujours **atomique et intégral**. Aucune fragmentation n'est autorisée sur l'AST pour préserver la structure hiérarchique. Le découpage (chunking) n'intervient qu'au moment de l'inférence vectorielle si le fichier dépasse les limites du modèle.
- **Audit de Capacité (RAM/Time) :** Chaque worker mesure l'empreinte RAM réelle (via `sysinfo`) et la durée de traitement. Ces données sont renvoyées à Elixir.
    - *Auto-calibration :* Elixir utilise ces statistiques pour décider d'augmenter le seuil de rejet (ex: passer de 1MB à 5MB) si les ressources le permettent.

---

## 5. Régulation de Flux Adaptative (v3.2)
Le système ne subit plus la charge, il l'orchestre selon une logique de fenêtre glissante.

### Calcul de la "Trémie" (ETS Buffer)
La taille optimale du buffer en mémoire vive Elixir est calculée par le Guardian :
- **Formule :** `Taille_Buffer = (Débit_Actuel_Files/sec) * Latence_Cible_Urgence`.
- **Objectif :** Garantir qu'un fichier urgent entrant dans KuzuDB sera traité dans un délai déterministe (ex: 10 secondes).
- **Mécanisme Watermark :**
    - *High-Water Mark :* Cible calculée (ex: 200).
    - *Low-Water Mark :* 50% de la cible (ex: 100).
    - *Action :* Dès que l'ETS descend au seuil bas, le Guardian tire un nouveau lot de 100 fichiers de KuzuDB.

---

## 6. Observabilité Mission-Critical : Axon.Watcher.Tracer
Le système s'appuie sur la bibliothèque **`Axon.Watcher.Tracer`** déjà intégrée pour mesurer la performance à chaque étape clé (T0: Discovery -> T4: Commit).
- **Points de Mesure :** Micro-latences sur l'envoi socket, le parsing AST, et le commit KuzuDB.
- **Métriques P99 :** Calcul en temps réel des percentiles pour détecter les anomalies de performance par cœur CPU.
- **Circuit Breaker :** Si le Tracer détecte une saturation au niveau de l'Actor Writer (T4), il peut ordonner au Guardian de suspendre momentanément le dispatch.

---

## 6. Écosystème MCP (Model Context Protocol)
Le serveur MCP offre une fenêtre de lecture prioritaire sur le graphe, protégée de la charge d'ingestion par le mode **MVCC Snapshot Isolation**.

### Flux de Requête LLM
1. **Demande :** L'agent IA (ex: Claude/Gemini) envoie une requête MCP (ex: `axon_query`).
2. **Priorité :** Le proxy MCP achemine la demande via un canal dédié à latence ultra-faible.
3. **Vérité :** Rust interroge KuzuDB en lecture seule. Même si 14 cœurs écrivent, le LLM obtient une vue cohérente et instantanée.

---

## 7. Méthodologie MBSE & Traçabilité (Digital Thread)
Axon v3.0 implémente une approche **Model-Based Systems Engineering (MBSE)**. Le graphe KuzuDB n'est pas un simple index, c'est le **Modèle Central** du système.

### Le Fil Numérique (Digital Thread)
Nous assurons une **Traçabilité Bidirectionnelle** intégrale entre l'intention et la réalité physique :
1.  **Problem Space (Conceptuel) :** Nœuds `Requirement` et `Concept`. C'est la partie **SOLL** décrivant le *Ce qui doit être*.
    - *Généalogie du Concept :* Les concepts sont versionnés via la relation `SUPERSEDES`. Toute évolution de la vision conserve l'archive de la décision précédente pour analyse historique.
    - *Adressage Sémantique Projet-Aware :* Pour maintenir des séquences courtes et lisibles, les identifiants sont préfixés par le type et le slug du projet (Format : `[TYPE(3)]-[PROJ(3)]-[NUM(3)]`). Ex: `REQ-AXO-001` (Requirement Axon), `CPT-HYD-012` (Concept HydraDB).
2.  **Solution Space (Technique) :** Nœuds `TechnicalSpec` et `Implementation`.
3.  **Physical Space (Code) :** Nœuds `File` et `Symbol` (AST). C'est la partie **IST** décrivant le *Ce qui est*.
4.  **Verification Space (Qualité) :** Nœuds `Test` certifiant que l'implémentation (IST) satisfait le requirement initial (SOLL).

---

## 8. Écosystème Technique & Environnement
L'infrastructure Axon est bâtie sur un socle technologique de classe industrielle garantissant isolation et performance.

### Stack Logicielle & Topologie
- **Topologie (Dual-Track) :** L'architecture repose sur trois systèmes distincts :
  - **Système Démon (Omniscience) :** Le service global en arrière-plan qui traite de manière concurrente N projets. C'est l'autorité de runtime et d'ingestion.
  - **Système de Production (La Forteresse) :** L'environnement "Live" de référence (racine du projet, ports `44129`/`44127`). Pour l'Agent IA, ce système est strictement **Read-Only** (le "Juge Officiel").
  - **Système de Développement (Le Laboratoire) :** L'environnement d'isolation asymétrique pour TDD (Git Worktree, ports `44139`/`44137`). Clone la base de prod localement pour expérimenter sans Blast Radius. Validation locale avant promotion.
- **Control Plane (Orchestration) :** Elixir 1.18+ / OTP 27. Interface Read-Only (Dashboard) et télémétrie.
- **Data Plane (Calcul) :** Rust 1.80+ / Tokio. Parsing haute performance via Tree-sitter et inférence AI via FastEmbed (ONNX). Autorité canonique.
- **Stockage Unifié :** DuckDB (Canard DB). Isolation physique SOLL/IST via `ATTACH DATABASE`. Support du MVCC pour des lectures concurrentes.
- **Protocole d'Échange :** MCP (Model Context Protocol) via JSON-RPC sur HTTP/SSE.

### Environnement de Développement (Reproducibilité)
Le projet utilise **Nix** et **Devenv** pour garantir une isolation totale :
- **Herméticité :** Toutes les dépendances (compilateurs, librairies C++, runtimes AI) sont déclarées dans `devenv.nix`.
- **Portabilité :** Le système se déploie à l'identique sur n'importe quelle machine Linux/WSL2, éliminant l'effet "ça marche sur ma machine".
