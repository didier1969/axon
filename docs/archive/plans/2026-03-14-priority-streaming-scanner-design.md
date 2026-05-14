# Design : Axon Priority Streaming Scanner (APSS)

**Date :** 2026-03-14
**Statut :** Approuvé
**Vision :** Transformer le scan d'Axon d'un processus séquentiel "Batch" en un pipeline réactif "Stream & Prioritize" pour une intelligence structurelle instantanée.

## 1. Problématique
Actuellement, Axon (Pod A) attend la fin du scan complet du disque dur avant de commencer l'indexation. Sur les gros projets (ex: monolithes > 50 000 fichiers), cela crée une latence initiale frustrante. De plus, l'ordre d'indexation est alphabétique, ce qui retarde l'analyse des fichiers critiques (`mix.exs`, `Cargo.toml`, `README.md`).

## 2. Architecture Technique (Triple-Pod Reactive)

### A. Producteur Rust (Zone A)
Le NIF `axon_scanner` est refactorisé pour devenir asynchrone :
- Utilisation de la crate `ignore` pour un parcours haute performance respectant `.axonignore`.
- Lancement d'un thread détaché (`std::thread`) pour ne pas bloquer les ordonnanceurs Erlang.
- Communication via messages asynchrones : `rustler::Env::send` envoie `{:file_discovered, path}` au GenServer Elixir dès qu'un fichier est trouvé.

### B. Orchestrateur Elixir (Zone B)
Le GenServer `Axon.Watcher.Server` devient le cerveau de la priorité :
- **Scoring Sémantique** : Chaque chemin reçoit un Score de Valeur (SV).
    - **SV 100** : Fichiers critiques (`mix.exs`, `Cargo.toml`, `README.md`).
    - **SV 80** : Code source à la racine (`/*.ex`, `/*.rs`).
    - **SV 50** : Code source standard dans les sous-dossiers.
    - **SV 10** : Tests et documentation secondaire.
- **Dispatching Adaptatif** :
    - Si **SV >= 100** : Envoi immédiat vers une file d'attente Oban prioritaire (`indexing_critical`).
    - Sinon : Accumulation dans une `PriorityQueue` locale et envoi par lots (toutes les 200ms ou tous les 20 fichiers) vers la file standard (`indexing_default`).

### C. Consommateurs Oban (Zone C)
Les workers existants traitent les fichiers en respectant la priorité des queues :
1. `indexing_critical` (traitée en priorité absolue).
2. `indexing_hot` (modifications en direct).
3. `indexing_default` (scan initial massif).

## 3. Flux de Données (Data Flow)

1. `Watcher.Server` appelle `Axon.Scanner.start_streaming(path, self())`.
2. Le thread Rust parcourt le disque et émet un flux de messages.
3. Elixir calcule le SV et aiguille vers la bonne queue Oban.
4. L'indexation commence **quelques millisecondes** après le début du scan.
5. Le Graphe (Pod C) se construit de manière "squelettique" (les fichiers structurants d'abord) puis se densifie.

## 4. Sécurité et Résilience
- **Back-pressure** : Le GenServer Elixir peut envoyer un signal "PAUSE" au thread Rust si la file d'attente dépasse 50 000 fichiers (Memory Safety).
- **Isolation** : Le thread Rust est monitoré ; son arrêt anormal déclenche une alerte système dans le Dashboard.

## 5. Succès Criteria
- **Time to First Insight (TTFI)** : Réduit de 90% sur les gros dépôts.
- **Priorité Sémantique** : Les fichiers structurants du projet sont indexés dans les 2 premières secondes.
- **Consommation Mémoire** : Stable, ne dépendant plus de la taille de la liste des fichiers stockée en RAM.
