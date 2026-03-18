# Axon v2.2 - Resource-Aware Scaling (Dynamic Backpressure)

## Goal
Intelligence d'infrastructure et respect absolu de l'environnement développeur. Mettre en place un backpressure dynamique basé sur la charge de l'OS.

## Tasks

### Task 1: OS Telemetry Monitor (COMPLETED)
- Intégration de `:os_mon` (Erlang) pour lire la charge CPU et RAM en temps réel.
- Créer un `Axon.ResourceMonitor` (GenServer) qui poll régulièrement `:cpu_sup` et `:memsup`.

### Task 2: Dynamic Worker Scaling & Hard Limit 70% (COMPLETED)
- Adaptation à la volée des limites d'Oban (`indexing_default` / `indexing_hot`) via le moniteur.
- Implémentation d'un plafond strict (Circuit Breaker) garantissant qu'Axon ne consomme jamais plus de 70% des ressources globales de la machine, se mettant en "pause" automatique si le système utilisateur exige la pleine puissance.

### Task 3: Dynamic Batching (COMPLETED)
- Réduction de la taille des lots (chunk size) envoyés au Data Plane Rust si la pression mémoire augmente. (Cela nécessitera de modifier `Axon.Watcher.Server` pour ajuster la taille du batch avant de l'insérer dans Oban).
