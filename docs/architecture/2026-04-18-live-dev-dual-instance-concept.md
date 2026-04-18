# Concept: Axon Dual Instance (Live vs Dev)

## Contexte

Axon doit satisfaire deux besoins en tension :

1. servir de source de vérité stable pour des agents/LLM qui utilisent sa surface MCP au quotidien ;
2. rester développable et testable sans mettre en risque la base de données, les ports, ou le runtime utilisé comme vérité active.

Aujourd’hui, ces deux besoins se recouvrent trop facilement :
- même machine ;
- même dépôt ;
- même famille de scripts ;
- même surface MCP ;
- et parfois confusion entre serveur utilisé pour travailler et serveur utilisé comme vérité.

Le besoin n’est pas de créer deux API différentes.
Le besoin est de créer **deux instances distinctes de la même surface MCP**.

## Vision

Axon doit exister localement sous deux formes explicites :

- **Axon Live**
  - instance stable, persistante, vérité opérationnelle ;
  - connectée à la base active ;
  - utilisée par les agents/LLM comme source de vérité principale ;
  - mutations limitées ou strictement contrôlées.

- **Axon Dev**
  - instance isolée, mutable, expérimentale ;
  - connectée à des bases séparées ;
  - utilisée pour coder, migrer, qualifier, casser, reconstruire ;
  - sans impact direct sur la vérité live.

Le principe central est :

- **même protocole MCP**
- **mêmes commandes**
- **même contrat d’outil**
- **instances différentes**
- **identité runtime différente**

## Règle d’identité runtime

Un LLM ne doit jamais déduire implicitement quelle instance il parle.

Chaque instance doit exposer explicitement dans `status` et dans ses métadonnées runtime :

- `instance_kind = live | dev`
- `runtime_identity`
- `data_root`
- `project_root`
- `mutation_policy`
- `build_id`
- `mcp_url`

La séparation ne doit pas être seulement mentale ou documentaire.
Elle doit être visible dans le protocole.

## Architecture cible

### 1. Deux endpoints MCP distincts

Exemple :

- `axon-live` → `http://127.0.0.1:44129/mcp`
- `axon-dev` → `http://127.0.0.1:45129/mcp`

Les commandes MCP restent identiques :

- `status`
- `query`
- `inspect`
- `retrieve_context`
- `soll_query_context`
- `soll_work_plan`
- etc.

Ce qui change est l’instance ciblée, pas l’API.

### 2. Deux racines d’état distinctes

Exemple :

- Live :
  - `~/.local/share/axon/live/ist.db`
  - `~/.local/share/axon/live/soll.db`
- Dev :
  - `~/.local/share/axon/dev/ist.db`
  - `~/.local/share/axon/dev/soll.db`

La règle est dure :

- jamais de partage direct de `ist.db`
- jamais de partage direct de `soll.db`
- jamais de WAL partagé

### 3. Deux identités de process distinctes

Exemple :

- service ou session `axon-live`
- service ou session `axon-dev`

Avec :

- PID files séparés
- sockets séparés
- ports séparés
- logs séparés

### 4. Deux politiques de mutation distinctes

- **Live**
  - lecture privilégiée ;
  - mutation très bornée ;
  - write-paths expertement contrôlés.

- **Dev**
  - mutations autorisées selon les workflows de développement ;
  - migrations et rechargements permis ;
  - qualification complète possible.

## Source de vérité

Le principe n’est pas :
- “Live = API différente”

Le principe est :
- “Live = même vérité protocolaire, autre identité runtime, autre stockage, autre politique”

Un LLM ou un opérateur ne doit pas apprendre deux Axon différents.
Il doit apprendre :

- un seul contrat MCP Axon ;
- deux cibles runtime explicites.

## Prévention de la confusion côté LLM

La confusion entre `live` et `dev` doit être empêchée par plusieurs couches :

### Couche 1. Nommage

- `axon-live`
- `axon-dev`

Jamais un seul alias générique ambigu si les deux existent en parallèle.

### Couche 2. Ports et URLs

Les deux instances doivent avoir des adresses différentes et stables.

### Couche 3. Handshake obligatoire

Le premier appel d’un agent doit être :

- `status`

Et la réponse doit permettre de confirmer :

- l’instance ;
- la politique de mutation ;
- les racines de données.

### Couche 4. Wrappers distincts

Côté scripts opérateur, il devient légitime d’avoir par exemple :

- `axon-live ...`
- `axon-dev ...`

ou des wrappers explicites de sélection d’instance.

### Couche 5. Configuration cliente distincte

Dans la configuration MCP du client LLM :

- deux serveurs distincts
- deux noms distincts
- jamais une seule entrée interchangeable.

## Politique d’usage recommandée

### Axon Live

Usage recommandé :

- lecture de vérité ;
- navigation et compréhension ;
- contexte pour développement ;
- requêtes MCP quotidiennes ;
- lecture SOLL/IST ;
- très peu de mutations, ou aucune par défaut pour les workflows de dev.

### Axon Dev

Usage recommandé :

- implémentation ;
- migrations ;
- requalifications ;
- expériences ;
- réparations de schéma ;
- tests mutateurs ;
- reconstruction de bases de dev si nécessaire.

## Promotion

Le cycle sain devient :

1. observer et comprendre via `live` ;
2. implémenter et qualifier via `dev` ;
3. valider les migrations et les invariants sur `dev` ;
4. promouvoir ensuite vers `live` de manière contrôlée.

## Non-objectifs

Ce concept ne propose pas encore :

- la forme exacte des scripts `start-live` / `start-dev` ;
- le packaging précis (`systemd`, user service, tmux, autre) ;
- la stratégie exacte de copie/synchronisation de bases ;
- la stratégie de migration live ;
- le plan d’implémentation détaillé.

Il fixe seulement la direction architecturale.

## Décisions conceptuelles

1. Axon doit exister en **deux instances** locales explicites : `live` et `dev`.
2. Les deux instances partagent la **même surface MCP**.
3. Les deux instances doivent avoir des **endpoints distincts**.
4. Les deux instances doivent avoir des **bases distinctes**.
5. `status` doit exposer une **identité runtime explicite**.
6. Un LLM ne doit jamais avoir à deviner s’il parle à `live` ou à `dev`.
7. Le développement, les migrations et les requalifications doivent se faire sur `dev` avant toute promotion vers `live`.

## Lien avec les documents existants

Ce concept prolonge l’idée de dual-track déjà présente dans :

- [2026-04-05-dual-track-environment.md](/home/dstadel/projects/axon/docs/architecture/2026-04-05-dual-track-environment.md)

Mais avec trois clarifications :

1. la séparation principale doit être pensée en **instances runtime** plus qu’en simple worktree ;
2. la surface MCP doit rester identique entre `live` et `dev` ;
3. l’identité runtime doit être rendue explicite dans le protocole et non laissée à la seule convention opérateur.
