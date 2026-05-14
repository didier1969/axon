# Design : Axon Live Cockpit v2.0 (Clustered)

**Date :** 2026-03-15
**Statut :** Approuvé
**Vision :** Transformer le Dashboard d'une interface statique v1 en un Cockpit Temps Réel connecté aux entrailles du Watcher via le clustering Erlang.

## 1. Architecture Réseau (Clustering)
Pour que le Dashboard (UI) voit l'activité frénétique du Watcher (Pod A), nous utilisons la distribution native de la BEAM.
- **Noms de nœuds** :
  - Dashboard : `dashboard@127.0.0.1`
  - Watcher : `watcher@127.0.0.1`
- **Cookie** : Partagé via la variable d'environnement `RELEASE_COOKIE="axon_v2_cluster"` définie dans `devenv.nix`.
- **Topologie** : Au démarrage, le Dashboard tente un `Node.connect(:"watcher@127.0.0.1")`.

## 2. Flux de Données (PubSub Translucide)
- Le Watcher (Pod A) inclut la dépendance `:phoenix_pubsub` et lance son propre registre PubSub sous le nom `Axon.PubSub`.
- **Événements diffusés par le Watcher** :
  - `{:scan_started, total_files}` (Début de la découverte)
  - `{:file_indexed, path, status}` (À chaque fichier avalé par Oban)
  - `{:batch_enqueued, count, queue}` (Progression des files)
- Le Dashboard s'abonne à ces événements dès qu'il rejoint le cluster.

## 3. Interface Utilisateur (LiveView)
La page `StatusLive` est refondue pour refléter l'autonomie du système.

**Modifications de l'UI :**
1. **Statut du Cluster** : Un indicateur "Lien Télépathique (Erlang)" vert ou rouge indiquant si l'UI est connectée au cerveau du Watcher.
2. **Compteurs Rapides (Approche C)** : 
   - Nombre total de fichiers dans la file d'attente (Oban backlog).
   - Vitesse d'ingestion (Fichiers/sec calculée en temps réel).
3. **Terminal "Matrix" (Approche A)** :
   - Une zone noir/vert (`font-mono`) en bas de page affichant les 20 derniers fichiers indexés, qui défile au fur et à mesure de l'ingestion (`> Indexing src/main.rs [✅ OK]`).
4. **Simplification des commandes** : Suppression du bouton "Start Scan" (puisque le Watcher est autonome). Conservation du bouton "Purge/Reset".

## 4. Bénéfices Attendus
- **Feedback instantané** : L'utilisateur voit l'indexation se produire sous ses yeux sans avoir à regarder les logs shell.
- **Architecture idiomatique** : Exploitation à 100% de la puissance d'Erlang/OTP pour la communication inter-processus, évitant de surcharger le pont UDS ou la base de données SQLite pour des événements éphémères.
