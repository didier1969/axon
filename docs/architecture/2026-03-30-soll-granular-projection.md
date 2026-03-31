# 2026-03-30 - SOLL Granular Disk Projection

## But

Ameliorer la revue Git et la lisibilite humaine de `SOLL` sans changer la source de verite runtime.

Le principe retenu est strict:

- `soll.db` reste canonique
- `SOLL_EXPORT_*.md` reste l'archive officielle horodatee
- toute projection disque granulaire reste derivee, regenerable, et non autoritaire

## Options Comparees

### 1. Snapshot horodate uniquement

Description:

- un export Markdown complet par snapshot

Avantages:

- tres robuste
- simple a restaurer
- excellent pour l'historique

Limites:

- diff Git grossiers
- review difficile sur un seul changement `Requirement` ou `Decision`
- conflits plus probables si plusieurs agents touchent `SOLL`

## 2. Snapshot + projection par item

Description:

- le snapshot horodate reste present
- une vue courante derivee projette certains objets dans des fichiers stables

Exemple de layout:

```text
docs/soll/current/
  requirements/
    REQ-AXO-002.md
  decisions/
    DEC-AXO-001.md
  validations/
    VAL-AXO-001.md
```

Avantages:

- diff Git fins
- revue locale par entite
- conflits reduits
- bon support pour audit humain et travail agentique

Limites:

- exige des identifiants stables
- ne doit pas devenir une nouvelle source de merge
- representation des liens a normaliser proprement

## 3. Snapshot + projection par document

Description:

- quelques fichiers agreges par type ou domaine

Exemple:

```text
docs/soll/current/
  requirements.md
  decisions.md
  validations.md
```

Avantages:

- plus simple que per-item
- lisible humainement

Limites:

- granularite encore trop large
- conflits Git encore frequents
- gain de review plus faible

## Critères D'Evaluation

La solution retenue doit:

- conserver `soll.db` comme source de verite
- ne pas fragiliser `axon_restore_soll`
- rester compatible avec `SOLL_EXPORT_*.md`
- rendre les diffs Git plus ciblés
- rester assez mince pour ne pas ouvrir un second systeme de persistance

## Decision Provisoire

Le prototype mince recommande est:

- `snapshot + projection par item`
- limitee aux types a identifiant stable:
  - `Requirement`
  - `Decision`
  - `Validation`

Ce choix donne la meilleure valeur de review sans toucher aux points encore fragiles:

- `Concept` reste exclu du prototype car sa cle actuelle est `name`
- `Pillar` et `Milestone` peuvent rester dans le snapshot tant que le besoin review n'est pas prouve

## Prototype Mince Recommande

La projection prototype est un `current view` derive.

Layout cible:

```text
docs/soll/current/
  requirements/
    REQ-AXO-002.md
  decisions/
    DEC-AXO-001.md
  validations/
    VAL-AXO-001.md
```

Exemple de fichier:

```md
# REQ-AXO-002

title: Reliable Restore
priority: P1
status: restored
metadata: {"risk":"high"}

links:
- BELONGS_TO: PIL-AXO-001
- SOLVED_BY: DEC-AXO-001
- VERIFIED_BY: VAL-AXO-001

description:
Restore from official export without destructive reset
```

## Utilite Pour La Review

Avant:

- un changement `Requirement` apparait dans un gros snapshot horodate
- le reviewer doit relire un document complet

Avec la projection prototype:

- un PR montre directement `docs/soll/current/requirements/REQ-AXO-002.md`
- les liens modifies sont visibles dans le meme diff
- la discussion Git devient locale a l'objet concerne

## Hors Perimetre

Ce prototype ne doit pas etre compris comme:

- une nouvelle source de verite
- un mecanisme de restore canonique
- une synchronisation bidirectionnelle garantie
- une couche transactionnelle entre fichiers

## Formulation Canonique

La projection granulaire proposee est une vue de travail derivee, orientee revue Git et inspection locale. Elle n'augmente pas, a elle seule, les garanties runtime ou la durabilite canonique de `SOLL`.
