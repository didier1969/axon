# Axon Data Plane: Industrial Batching Design (Solution 2)

## Problème
L'ingestion synchrone de graphes avec KuzuDB (via Elixir Watcher) provoque un goulet d'étranglement fatal. Elixir envoie des requêtes d'indexation concurrentes très rapidement via le socket UDS. Pour chaque fichier, Axon Core (Rust) ouvre une transaction SQLite/KuzuDB, et exécute séquentiellement des centaines de commandes `MERGE` (Symboles, CONTAINS, CALLS).
Avec l'activation des appels standards (`extract_generic_call`), le volume d'arêtes explose, le Write Lock de KuzuDB sature, et le serveur UDS part en timeout (freeze complet).

## Architecture Cible : Queue d'Insertion Asynchrone
Plutôt que d'insérer directement dans KuzuDB, le thread principal Rust qui écoute le socket UDS va simplement déléguer les résultats de parsing (`ExtractionResult`) dans une file d'attente mémoire `mpsc::channel`. Un worker asynchrone (le *Graph Writer*) dépilera cette queue par lots (batchs) pour ne faire qu'une seule énorme transaction KuzuDB par seconde.

### Composants
1. **La Queue (tokio::sync::mpsc::unbounded_channel) :** 
   - `Sender` : clôné par chaque worker de parsing.
   - `Receiver` : consommé par le thread d'écriture en arrière-plan.

2. **Le Graph Writer (Background Task) :** 
   Un thread `tokio::spawn` dédié uniquement aux écritures KuzuDB.
   - Accumule les messages `FileInsertion`.
   - Exécute un "Flush" toutes les 1000ms ou dès 10 fichiers accumulés.
   - Fait un seul `BEGIN TRANSACTION` et boucle sur les exécutions.

## Étapes de l'implémentation
1.  **Refactorisation de `main.rs` :**
    - Créer le channel au démarrage.
    - Créer la tâche asynchrone `graph_writer_loop`.
    - Modifier `PARSE_FILE` pour envoyer un message au lieu d'écrire en DB.
2.  **Refactorisation de `graph.rs` :**
    - Créer la méthode `insert_batch_data` qui prend un `Vec<(String, ExtractionResult)>`.
    - L'entourer d'une unique transaction.

Cette architecture résoudra définitivement les crashs et propulsera Axon à une échelle d'ingestion industrielle (plusieurs milliers de nœuds par seconde).
