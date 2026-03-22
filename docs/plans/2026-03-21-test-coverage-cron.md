# Axon Test Coverage: Passage en Cron Asynchrone

## Objectif
Actuellement, la requête de calcul de couverture de test (`update_test_coverage_flags`) s'exécute à *chaque* insertion de données KuzuDB (toutes les 1.5s), forçant une simplification sémantique (profondeur de 1 seulement, chemin flou).
Le but est de sortir cette analyse lourde du pipeline d'ingestion critique, et de la faire tourner en arrière-plan (Cron) toutes les 2 minutes. Cela permettra d'allonger la profondeur du graphe Cypher à 3 niveaux d'appels (`*1..3`) pour capturer la vraie couverture via les Mocks/Fixtures sans bloquer le moteur d'indexation.

## Changements Requis
1. **Rust (`src/axon-core/src/graph.rs`) :**
   Mettre à jour la requête Cypher pour qu'elle soit exacte :
   ```cypher
   MATCH (test_file:File)-[:CONTAINS]->(test_func:Symbol)-[:CALLS*1..3]->(prod_func:Symbol)
   WHERE test_file.path CONTAINS '/test/' OR test_func.name STARTS WITH 'test_'
   SET prod_func.tested = true
   ```
2. **Rust (`src/axon-core/src/graph_writer.rs`) :**
   - Retirer l'appel à `locked.update_test_coverage_flags()` de la fonction critique `flush_buffer()`.
3. **Rust (`src/axon-core/src/main.rs`) :**
   - Déclarer un nouveau thread asynchrone (`tokio::spawn`) avec un `interval` de 120 secondes (2 minutes).
   - Ce thread prendra un verrou en écriture (`write().unwrap()`) toutes les 2 minutes pour exécuter la requête Cypher profonde.

Ceci garantira une intégrité à 100% de la métrique, sans compromis sur la stabilité système.