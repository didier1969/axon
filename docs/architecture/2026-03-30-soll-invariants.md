# 2026-03-30 - SOLL Executable Invariants

## Intention

Axon ajoute une premiere couche d'invariants `SOLL` executables pour detecter les etats orphelins evidents sans casser le contrat historique:

- `SOLL` reste protege et distinct de `IST`
- la validation est `read-only`
- `export -> restore` reste supporte en mode merge
- aucun auto-repair silencieux n'est autorise a ce stade

## Premier slice retenu

Le premier slice couvre uniquement des garde-fous minimaux et compatibles avec l'etat actuel du graphe `SOLL`:

1. un `Requirement` ne doit pas etre totalement orphelin
2. une `Validation` doit verifier quelque chose
3. une `Decision` doit etre rattachee a un besoin ou a un impact explicite

Ces checks sont exposes via `axon_validate_soll`.

## Pourquoi ce slice

Ce slice est volontairement limite:

- il ne suppose pas une restauration complete de toutes les relations `SOLL`
- il ne bloque pas les anciens exports `SOLL_EXPORT_*`
- il n'introduit pas de contrainte DB native qui casserait le mode merge actuel
- il rend deja executables les garde-fous de coherence les plus utiles

## Portee exacte

`axon_validate_soll` signale:

- les `Requirement` sans lien dans `BELONGS_TO`, `EXPLAINS`, `SOLVES`, `TARGETS`, `VERIFIES`, `ORIGINATES`, `SUBSTANTIATES`, `IMPACTS`
- les `Validation` sans lien `VERIFIES`
- les `Decision` sans lien `SOLVES` ni `IMPACTS`

Le resultat est un rapport de coherence minimale.

## Hors perimetre volontaire

Le validateur ne pretend pas:

- certifier la qualite metier complete de `SOLL`
- prouver l'exhaustivite strategique du graphe conceptuel
- reconstruire toutes les liaisons absentes des exports historiques
- auto-corriger les violations detectees

## Suite prevue

Les prochains durcissements devront rester compatibles avec ces principes:

- validation d'abord, mutation ensuite seulement si elle est explicite
- invariants bases sur les liens reellement modelises et restaurantables
- pas de promesse de coherence totale tant que le restore reste partiel
