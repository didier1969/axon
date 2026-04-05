# Architecture Mission-Critical : Stratégie Dual-Track (Production vs Développement)

## 1. Vision SRE (Zero Blast Radius)
L'infrastructure Axon (Control Plane) doit garantir une disponibilité de 100% (Zero Downtime) pour les agents IA qui s'en servent comme source de vérité (SOLL), tout en permettant à ces mêmes agents de développer et tester de nouvelles fonctionnalités sur le moteur Axon lui-même.

Pour éviter toute collision (Ports, Filesystem, Locks SQLite, Erlang EPMD), l'architecture repose sur une isolation absolue via **Git Worktrees**.

## 2. Implémentation Physique

### A. La Production (La Forteresse)
- **Emplacement :** Racine du projet (`/home/dstadel/projects/axon/`).
- **Branche :** `main`.
- **Ports :** `44129` (MCP/SQL), `44127` (Dashboard Phoenix).
- **Stockage :** `.axon/graph_v2/ist.db`.
- **TMUX Session :** `axon`.

### B. Le Laboratoire (Le Bac à Sable de Développement)
- **Emplacement :** Dans un Git Worktree ignoré (ex: `.worktrees/dev/feat-x`).
- **Branche :** `feat/*`.
- **Mécanique de Démarrage :** Les scripts de lancement (`start.sh`, `stop.sh`) détectent qu'ils tournent hors de la racine de production et instancient automatiquement l'environnement de développement :
  - **Ports :** `44139` (MCP/SQL), `44137` (Dashboard Phoenix).
  - **Stockage :** Un dossier `.axon-dev/` est créé.
  - **TMUX Session :** `axon-dev` (avec nœud Elixir renommé en `axon_dev_nexus`).

## 3. Workflow Opérationnel de l'Agent IA

1. **Isolation Git :** L'agent IA DOIT créer un Git Worktree dans `.worktrees/dev/`. Il lui est interdit de développer dans le dossier racine.
2. **Synchronisation Médico-légale :** Avant de démarrer, l'agent IA exécute `scripts/sync_to_dev.sh`. Ce script copie la base de données DuckDB de Production (`.axon/graph_v2/ist.db`) vers le dossier de Dev (`.axon-dev/graph_v2/ist.db`) afin d'offrir des données réalistes sans impacter les I/O de la Prod.
3. **Expérimentation :** L'agent lance le serveur local (`scripts/start.sh`) qui tournera en parallèle de la Prod sans interférence.
4. **Validation (Commit) :** L'agent utilise le serveur MCP de Dev (`44139`) pour exécuter l'outil `axon_commit_work`. L'outil vérifie les règles et valide le commit sur la branche de Dev.
5. **Promotion (Zero Downtime) :** L'humain fusionne la branche dans `main`. Un script de déploiement en Prod tire le code (`git pull`), compile la release Rust en arrière-plan (`cargo build --release`), puis redémarre la session TMUX `axon` en quelques millisecondes (Hot Swap).

## 4. Configuration LLM (Dual-MCP)
Pour opérer dans cet environnement, le fichier `claude.json` (ou `mcp_servers.json`) de l'utilisateur héberge deux serveurs distincts :
- `axon-prod` (`http://127.0.0.1:44129/mcp`) : Le Juge Officiel Read-Only.
- `axon-dev` (`http://127.0.0.1:44139/mcp`) : Le Serveur Local pour l'expérimentation.