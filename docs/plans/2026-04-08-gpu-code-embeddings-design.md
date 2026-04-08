# Design Axon: Embedder Code GPU 8 Go

## Statut

Valide pour implementation apres plan TDD. Aucun changement de code n'est encore autorise par ce document.

## Contexte

Axon vectorise aujourd'hui avec `BAAI/bge-small-en-v1.5` en `384d`, via `fastembed`, sur un unique worker semantique. Le stockage IST est fige en `FLOAT[384]` dans DuckDB pour `Symbol`, `ChunkEmbedding` et `GraphEmbedding`.

Cette architecture ne soutient pas l'objectif vise:
- meilleure qualite de retrieval sur code
- execution locale sur machine avec `8 Go VRAM`
- acceleration GPU reelle
- debit cible autour de `300 000 embeddings/heure` si le corpus et le batching le permettent

## Contrainte materielle

Machine cible:
- GPU local avec `8 Go VRAM`
- contrainte forte d'integration pragmatique dans le runtime Rust existant
- fallback CPU obligatoire si le provider GPU n'est pas disponible

## Sources de verite externes

- `fastembed` supporte `BAAI/bge-base-en-v1.5` et `jinaai/jina-embeddings-v2-base-code`
- `jinaai/jina-embeddings-v2-base-code` est explicitement oriente code retrieval, couvre l'anglais + 30 langages de programmation, supporte des sequences jusqu'a `8192`, et reste compact (`161M` params)
- `Qodo-Embed-1-1.5B` est plus ambitieux en qualite pure mais trop lourd et trop risqué pour une premiere integration locale `8 Go VRAM`
- ONNX Runtime supporte bien CUDA, mais l'usage GPU doit etre active explicitement et verifie a l'execution

## Decision

### Modele cible primaire

`jinaai/jina-embeddings-v2-base-code`

Raisons:
- meilleur fit semantique avec le probleme Axon: recherche et voisinage de code
- meilleur compromis qualite/performance/integration sous `8 Go VRAM`
- supporte documents de code plus longs que la pile BGE actuelle
- deja present dans l'ecosysteme `fastembed`

### Modele fallback

`BAAI/bge-base-en-v1.5`

Raisons:
- `768d`
- beaucoup plus simple conceptuellement si `jina` echoue sur la stack runtime GPU
- trajectoire de migration plus prudente que `Qodo`

### Modele explicitement non retenu pour la premiere tranche

`Qodo/Qodo-Embed-1-1.5B`

Raisons:
- `1.5B` params
- `1536d`
- stack plus lourde
- risque eleve de pression VRAM et de friction d'integration
- trop de variables nouvelles a la fois pour Axon dans son etat actuel

## Objectif produit

Construire une filiere embeddings code-aware acceleree GPU, configurable, observable et reindexable, capable de remplacer le pipeline `384d` actuel sans casser:
- la stabilite runtime
- la lisibilite du schema IST
- la coherence des queues et de la revectorisation

## Portee fonctionnelle

La filiere cible doit couvrir explicitement:
- methodes/procedures/fonctions via `Symbol`
- fichiers via la file `FileVectorizationQueue` et les `ChunkEmbedding`
- projections graphe via `GraphEmbedding`

Decision importante:
- on conserve la distinction `symbol/chunk/graph`
- on ne fusionne pas tout en une seule table d'embeddings
- on rend la configuration du modele et de la dimension explicite par type d'embedding, meme si la premiere tranche garde un modele unique

## Verite actuelle du code

Blocages structurants:
- dimensions fixees a `384`
- IDs de modeles hardcodes
- colonnes DuckDB `FLOAT[384]`
- un seul worker semantique OS thread
- pas d'activation explicite du provider GPU dans `InitOptions`
- batch sizes faibles et fixes

Conclusion:
- le passage a un embedder GPU code-aware n'est pas une simple substitution de nom de modele
- c'est une migration de contrat runtime + stockage + orchestration

## Approches evaluees

### Approche A: simple swap vers `bge-base-en-v1.5`

Avantages:
- faible risque d'integration
- `768d`
- garde la famille BGE deja connue

Limites:
- moins specialisee code
- gain qualite incertain sur recherche de methodes/procedures longues
- ne traite pas le vrai besoin de retrieval code-aware

### Approche B: migration vers `jinaai/jina-embeddings-v2-base-code`

Avantages:
- modele explicitement entraine pour le code
- support sequences longues
- taille compatible avec `8 Go VRAM`
- meilleur compromis pour Axon

Limites:
- migration plus profonde
- exige validation fine du backend ONNX/GPU
- necessite deconfigurer les dimensions et le schema

### Approche C: saut direct vers `Qodo-Embed-1-1.5B`

Avantages:
- ambition qualite maximale

Limites:
- trop de risque pour la VRAM, le packaging et le debit
- plus adapte a une phase benchmark avancee qu'a la stabilisation d'Axon

Recommendation:
- retenir `B`
- garder `A` comme fallback securise
- exclure `C` de la premiere tranche

## Architecture cible

### 1. Contrat embedding configurable

Introduire un contrat runtime explicite:
- `embedding_model_id`
- `embedding_model_name`
- `embedding_dimension`
- `embedding_backend`
- `embedding_execution_provider`
- `embedding_kind`

Ce contrat doit etre utilise pour:
- `Symbol`
- `ChunkEmbedding`
- `GraphEmbedding`

### 2. Backend d'inference explicite

Le runtime doit choisir et journaliser:
- `cuda`
- ou fallback `cpu`

Le simple fait de detecter un GPU n'est pas suffisant. Il faut prouver quel provider est reellement actif.

### 3. Schema compatible migration

Le schema ne doit plus supposer `384`.

Deux options existent:
- `FLOAT[]` si le plugin DuckDB et les usages Axon le supportent proprement
- colonnes migrables versionnees avec dimension derivee du modele

Decision de design:
- commencer par une migration explicite versionnee vers un schema dimension-configurable
- ne pas bricoler une cohabitation silencieuse de plusieurs dimensions sans gouvernance

### 4. Pipeline semantique calibrable

La filiere doit pouvoir:
- calibrer batch size selon VRAM
- mesurer tokens/chunk
- separer symbol backlog, file vectorization backlog, graph projection backlog
- prioriser les embeddings utiles

### 5. Benchmarks de qualification

Avant toute bascule par defaut:
- benchmark qualite sur corpus Axon reel
- benchmark debit sur votre machine
- benchmark VRAM

## Objectifs de performance

Objectif nominal:
- viser `>= 80 embeddings/s` sur un corpus representatif de production

Important:
- `300 000/h` n'est pas une garantie universelle
- cet objectif depend fortement:
  - de la longueur moyenne des chunks
  - du batching
  - du provider GPU reel
  - de la pression I/O et DuckDB

Donc le vrai contrat sera:
- debit mesure
- qualite mesuree
- consommation memoire mesuree

## Risques

- faux positif GPU: le runtime croit utiliser le GPU mais retombe sur CPU
- migration DuckDB incomplete ou couteuse
- derive memoire si on augmente dimensions et batching sans gouverneur
- qualite meilleure sur retrieval pur mais cout runtime trop eleve
- regression des files de vectorisation si la reindexation est mal versionnee

## Garde-fous

- TDD strict
- benchmark gate avant activation par defaut
- fallback CPU explicite
- invalidation/revectorisation versionnee
- documentation du modele reellement actif

## Definition of Done

La tranche sera consideree comme reussie seulement si:
- Axon n'est plus fige en `384d`
- le modele d'embedding est configurable
- le provider GPU reel est visible dans les logs et dans la telemetrie
- le schema et la migration sont propres
- la revectorisation est commandable et testee
- un benchmark Axon reel justifie le choix `jina` ou prouve qu'il faut retomber sur `bge-base`
