# Audit d'Installation - Axon Industrial v1.0

## Journal des Difficultés Rencontrées (Session 2026-03-09)

### 1. Corruption du Stockage HydraDB (Pod C)
- **Symptôme :** Crash immédiat au démarrage (`Failed to start HydraDB DB`).
- **Erreur :** `Failure while replaying WAL file "...row_store.duckdb.wal": Could not find node in column segment tree!`.
- **Cause probable :** Environnement Mix/Nix modifié ou arrêt brutal lors de la migration.
- **Remède factuel :** Suppression manuelle du contenu de `priv/storage/row_store/`.
- **Action corrective :** Implémenter une routine de self-healing dans le wrapper de démarrage.

### 2. Dépendances OS manquantes (inotify-tools)
- **Symptôme :** Avertissement `inotify-tools is needed to run file_system`.
- **Impact :** La surveillance temps réel (Pod A) ne fonctionne pas sans cet outil.
- **Remède factuel :** Vérifier que `inotify-tools` est présent dans `buildInputs` du `flake.nix` et que la session tourne sous `nix develop`.

### 3. Avertissements de Compilation (Rustler)
- **Symptôme :** Warning `use of deprecated constant rustler_init::explicit_nif_functions`.
- **Remède :** Retirer la liste explicite des fonctions `[scan]` dans `rustler::init!`.

### 4. Environnement Python (Pod B)
- **Symptôme :** `ModuleNotFoundError: No module named 'msgpack'`.
- **Cause :** Exécution directe sans `uv run` ou hors du `devShell` Nix.
- **Règle Rigide :** Toujours préfixer les commandes Python par `uv run` pour garantir l'utilisation de l'environnement isolé.

---
*Document maintenu par le Nexus Lead Architect.*
