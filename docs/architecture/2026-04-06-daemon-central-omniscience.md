# Architecture: Démon Central (Omniscience)

## Contexte
Actuellement, Axon fonctionne selon une architecture "Sidecar" : une instance du démon (et de sa base de données `soll.db` / `ist.db`) est lancée localement dans le répertoire racine de chaque projet. Cette approche garantit une forte isolation mais empêche l'agrégation transversale des connaissances (cross-project analysis) et la "réflexion" du système sur lui-même lorsqu'il intervient sur un autre projet.

## Vision (The Living Lattice)
La souveraineté sémantique totale exige un **Treillis de Connaissance Vivant** centralisé. Le système doit basculer vers un modèle de Démon Central (Omniscience) capable de traiter, stocker, et analyser simultanément les graphes de N projets enregistrés sur la machine hôte.

## Décisions Architecturales

### 1. Démon Système Global (Control Plane)
* Le démon Axon ne s'exécute plus de manière isolée dans le dossier de travail du développeur.
* Il devient un service système unique (ex: service utilisateur `systemd`) écoutant sur un port global dédié (ex: 44129) pour exposer le protocole MCP et l'API HTTP.

### 2. Base de Données Unifiée (Single Source of Truth)
* L'ensemble des bases physiques (`soll.db`, `ist.db`, données vectorielles) sont consolidées dans un emplacement de stockage global par utilisateur (ex: `~/.local/share/axon/db/`).
* **Partitionnement Sémantique :** Toutes les tables (Symbol, File, Node, Edge, CALLS) utilisent le `project_slug` (ex: AXO, FSC, HYD) comme clé de partitionnement et critère de filtrage strict.

### 3. Fédération et Enregistrement Dynamique
* L'intégration d'un nouveau projet au Treillis s'effectue via une commande explicite d'enregistrement (ex: `axon register ~/projects/Fiscaly --slug FSC`).
* Le Control Plane maintient le catalogue des projets actifs (ProjectCodeRegistry) et orchestre l'ingestion et les watchers de fichiers de manière concurrente pour chacun d'eux.

### 4. Routage du Contexte MCP
* Les outils MCP (ex: `soll_query_context`, `axon_query`) exigent le passage en paramètre du `project_slug`, ou le déduisent automatiquement à partir du chemin (`uri`) fourni par le client MCP.
* Le Control Plane garantit l'isolation des requêtes : une requête sur le scope `FSC` ne filtre que les données de Fiscaly, à moins qu'une requête transversale explicite (scope `GLOBAL`) ne soit invoquée pour des analyses inter-projets.
