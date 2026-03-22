# Axon Data Plane: mtime Amnesia Fix

## Problème
Au démarrage, l'orchestrateur (Elixir) tente de lire la date de dernière modification (`mtime`) de chaque fichier via une connexion TCP vers HydraDB (port 44128) pour savoir s'il doit le réindexer. La connexion échoue silencieusement (`econnrefused`), renvoyant `0` par défaut. Résultat : Axon conclut que les 36 000 fichiers sont "nouveaux" et les envoie tous à Rust, causant des surcharges CPU, des timeouts MCP, et des deadlocks KuzuDB.

## Solution : Mtime Local (SQLite)
La table SQLite `indexed_files` contient déjà une colonne `file_hash`. Nous allons la renommer (ou l'utiliser) pour stocker le `mtime` (date de modification hachée). Le `Watcher` vérifiera d'abord SQLite (local, instantané) avant de déclencher l'ingestion, ignorant instantanément les dizaines de milliers de fichiers qui n'ont pas bougé.

## Étapes
1. **Elixir (`Tracking.ex`) :**
   - Créer une méthode `get_file_mtime(path)` qui interroge la table `indexed_files` (champ `file_hash` qui sert de mtime).
2. **Elixir (`Progress.ex`) :**
   - Rediriger `get_file_mtime` et `save_file_mtime` vers la nouvelle méthode locale (`Tracking.ex`) au lieu d'utiliser le réseau `sync_send_to_hydradb`.
3. **Elixir (`Server.ex`) :**
   - S'assurer que le scan initial met à jour le `file_hash` correctement avec le nouveau `mtime`.
4. **Purge :**
   - Vider Oban une dernière fois. Le prochain redémarrage sera foudroyant de rapidité.