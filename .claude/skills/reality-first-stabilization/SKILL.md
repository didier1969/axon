---
name: reality-first-stabilization
description: |
  Utiliser ce skill pour reprendre, auditer, stabiliser ou réaligner un système complexe avant de l'optimiser. Il s'applique particulièrement aux dépôts polyglottes, aux architectures dérivées de leur vision initiale, aux environnements de développement non fiables, et aux systèmes où il faut distinguer clairement vision, architecture cible, code réel et runtime réel.

  Déclencher quand : audit de qualité, reprise de projet, dette technique structurelle, environnement Nix/Devenv ou CI instable, divergence docs/code, ingestion ou pipeline fragile, concurrence douteuse, faux signaux de santé, ou demande explicite de remettre un système "sous contrôle" avant de le pousser plus loin.
---

# Reality-First Stabilization

Méthodologie de reprise structurée pour systèmes réels, avec priorité absolue à la stabilité, à la fiabilité et à la vérité d'exécution.

## Principe central

Ne jamais partir des intentions seules.

Toujours séparer et comparer ces 4 couches :

1. **Vision** : ce que le projet dit vouloir être
2. **Architecture cible** : ce que les documents décrivent comme structure souhaitée
3. **Code réel** : ce qui est effectivement implémenté
4. **Runtime réel** : ce qui fonctionne, casse, dérive ou ment en conditions de test

Le travail ne commence vraiment qu'une fois ces quatre couches distinguées.

## Quand ce skill est le bon choix

Utiliser ce skill si au moins deux de ces symptômes sont présents :

- environnement de développement non fiable ou non reproductible
- documentation ambitieuse mais code partiellement aligné
- mélange de générations techniques ou de migrations incomplètes
- tests cassés pour des raisons d'environnement autant que de logique
- systèmes critiques avec ingestion, orchestration, concurrence ou observabilité fragiles
- besoin d'un audit priorisé, pas d'une liste exhaustive sans hiérarchie

## Résultat attendu

Ce skill ne produit pas un "beau rapport".

Il doit produire :

1. une cartographie nette de la réalité du système
2. une liste priorisée de défauts dominants
3. une séquence de remédiation fondée sur les dépendances réelles
4. des preuves de progression mesurables
5. un handoff durable pour reprise sans perte de qualité

## Workflow obligatoire

### Phase 1. Prise de terrain

Avant toute correction importante :

1. identifier la branche active et l'état Git
2. repérer les documents de vision, roadmap, audit, architecture
3. localiser les points d'entrée runtime et les composants réellement exécutés
4. identifier les artefacts générés, composants historiques et couches en transition

Ne pas faire de promesse de correction avant d'avoir compris où vit la vérité du système.

### Phase 2. Validation de l'environnement de vérité

Avant de conclure quoi que ce soit sur la qualité du code :

1. identifier l'environnement officiel du projet
2. vérifier si le shell courant correspond réellement à cet environnement
3. distinguer les outils "présents sur la machine" des outils "fournis par l'environnement de vérité"
4. corriger en priorité les dérives d'outillage qui invalident les diagnostics

Exemples :

- `Nix + Devenv`
- `Docker Compose`
- `mise`
- `asdf`
- `direnv`
- CI hermétique

Une erreur de shell ne doit jamais être interprétée trop vite comme une erreur métier.

### Phase 3. Réconciliation théorie / réalité

Comparer explicitement :

1. ce que disent les docs
2. ce que fait le code
3. ce que les tests prouvent
4. ce que le runtime confirme

Le livrable minimal est une table mentale ou écrite :

- aligné
- partiel
- non aligné
- obsolète

### Phase 4. Détection des défauts dominants

Ne pas viser l'exhaustivité d'abord.

Chercher les quelques défauts qui :

- cassent les tests en chaîne
- compromettent la confiance dans les résultats
- rendent l'ingestion ou la persistance non déterministes
- exposent des incohérences de protocole entre composants
- font mentir les métriques, audits, dashboards ou rapports

Le but est d'identifier le noyau de désordre qui génère le reste.

### Phase 5. Remédiation par ordre de dépendance

Ordre par défaut :

1. environnement
2. bootstrap stockage / init système
3. cohérence des transitions d'état
4. protocole et intégration inter-composants
5. fiabilité des tests
6. seulement ensuite performance, tuning et raffinement

Refuser les optimisations qui reposent sur un socle encore instable.

### Phase 6. Mesure de progression

Chaque étape importante doit être vérifiée par un signal concret :

- tests passés ou régressions isolées
- réduction chiffrée des échecs
- environnement reproductible
- protocole validé
- runtime plus déterministe

Toujours préférer une progression de type :

`21 échecs -> 9 -> 4 -> 1 -> 0`

à une conclusion vague du type :

"ça semble beaucoup mieux".

### Phase 7. Handoff anti-perte

Avant compaction, pause longue ou changement d'agent :

1. écrire un handoff dans le repo
2. y noter état Git, branche, fichiers modifiés, tests, décisions, prochains pas
3. décrire les risques restants et les validations non terminées

La continuité doit reposer sur des preuves écrites, pas sur la mémoire implicite.

## Discipline d'exécution

### Toujours faire

- commencer par le terrain réel
- expliciter les hypothèses
- séparer symptômes et causes
- prioriser les défauts structurants
- utiliser les skills spécialisés comme sous-outils
- conserver un fil de progression vérifiable

### Ne pas faire

- corriger localement sans comprendre l'environnement
- confondre architecture cible et implémentation actuelle
- appeler "stable" un système qui ne passe pas ses tests clés
- optimiser un pipeline dont les transitions d'état ne sont pas fiables
- produire un audit verbeux mais non actionnable

## Usage des skills complémentaires

Ce skill orchestre volontiers d'autres skills.

Utiliser selon besoin :

- `devenv-nix-best-practices` pour l'environnement et la reproductibilité
- `mission-critical-architect` pour concurrence, deadlocks, backpressure, topologie réelle
- `system-observability-tracer` pour checkpoints, propagation de trace et vérité runtime
- `hardware-aware-scaling` pour distinguer parallélisme réel, faux gains et contention
- un skill projet local pour les concepts métier ou le vocabulaire interne

Ce skill ne remplace pas ces skills. Il décide quand les invoquer et dans quel ordre.

## Sortie attendue

Une bonne sortie produite avec ce skill contient généralement :

1. un constat court sur la réalité du système
2. une liste priorisée de findings
3. les validations réellement passées
4. les validations encore ouvertes
5. la prochaine étape logique

Si le travail est long, produire aussi un handoff durable dans le repo.

## Formule de rappel

Stabilité avant sophistication.
Environnement avant diagnostic.
Réalité avant intention.
Preuve avant conclusion.
