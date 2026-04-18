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
- `release_version`
- `data_root`
- `project_root`
- `mutation_policy`
- `build_id`
- `install_generation`
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
4. préparer une version installable avec `release_version` incrémentée ;
5. promouvoir ensuite vers `live` de manière contrôlée ;
6. vérifier après promotion que `live` annonce bien la nouvelle identité de version.

## Qualification vs Promotion

Un `git push` n’est pas une promotion production.

Il faut distinguer trois états :

- `pushed`
  - le code est sauvegardé et partagé sur GitHub ;
  - aucune valeur de production n’est impliquée.

- `qualified`
  - un commit ou un tag immuable a passé les gates définis ;
  - les preuves de qualification existent ;
  - la version est candidate à la promotion.

- `promoted`
  - la version qualifiée a été effectivement installée sur `live` ;
  - `live status` annonce cette identité exacte ;
  - la promotion est vérifiée après installation.

Le principe dur est :

- `push` ≠ `qualified`
- `qualified` ≠ `promoted`

## Cycle de mise en production cible

Le cycle cible devient :

1. développement et qualification sur `dev` ;
2. merge sur `main` ;
3. exécution des gates de qualification ;
4. création d’une version immuable :
   - tag `vX.Y.Z`
   - artefact installable immuable
   - manifest de release
   - preuves de qualification
5. promotion explicite vers `live` ;
6. post-check sur `live` :
   - `release_version`
   - `build_id`
   - `install_generation`
   - santé runtime
7. rollback vers la version précédente si le post-check échoue.

## Promotion maîtrisée

La promotion de `live` doit être un cycle séparé et contrôlé.

Elle doit reposer sur :

- une version qualifiée immuable ;
- un artefact installable identifié exactement ;
- un manifest de release canonique ;
- un script ou workflow de promotion explicite ;
- un post-check obligatoire ;
- un chemin de rollback simple.

La décision “cette version est production” doit donc venir :

- d’un état de qualification prouvé ;
- puis d’une promotion explicite ;
- jamais d’un simple push GitHub.

## Identité d’artefact

Le manifest de release ne doit pas seulement identifier le code source.
Il doit identifier **l’artefact exact** qui a été qualifié puis promu.

Le manifest doit donc porter au minimum :

- le tag ou commit source ;
- `release_version` ;
- `build_id` ;
- l’identifiant d’artefact installable ;
- un checksum ou digest de l’artefact ;
- les preuves de qualification attachées à cet artefact.

La règle dure est :

- la promotion doit installer l’artefact qualifié ;
- elle ne doit pas reconstruire implicitement “quelque chose d’équivalent” à partir du même commit.

## Contrat de rollback

Le rollback ne peut pas être défini seulement comme “revenir à la version précédente”.
Il faut aussi définir ce qui arrive aux données et au schéma live.

Une release promouvable doit donc satisfaire au moins une des deux conditions :

- soit elle ne réalise que des changements backward-compatible sur les données et le schéma live ;
- soit elle fournit un plan de rollback explicite pour l’état live :
  - snapshot / backup requis ;
  - stratégie de retour ;
  - limites connues.

Donc la promotion `live` doit lier :

- le code / artefact ;
- la version annoncée ;
- et la compatibilité ou la stratégie de retour sur l’état de données live.

## Contrat de version de production

La promotion vers `live` ne doit pas seulement remplacer du code.
Elle doit remplacer une **version identifiée**.

Chaque runtime doit pouvoir annoncer :

- `release_version`
  - la version opératoire humaine qui est censée être installée ;
- `package_version`
  - la version de package / build interne ;
- `build_id`
  - l’identité exacte du build (commit, tag, dirty/clean) ;
- `install_generation`
  - l’identifiant de l’installation ou de la promotion active.

Cela permet de dire explicitement :

- “voici ce qui tourne en `live` maintenant” ;
- “voici la version candidate qui remplacera `live`” ;
- “la promotion a bien été effectuée”.

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
