# Roadmap Axon

Ce document décrit la **suite rationnelle** du projet à partir de l’état vérifié au `2026-04-01`.

Il ne remplace pas `STATE.md`, qui porte la photographie de vérité exécutable.
Le plan maître d’exécution jusqu’à livraison est désormais [docs/plans/2026-04-01-axon-delivery-plan.md](/home/dstadel/projects/axon/docs/plans/2026-04-01-axon-delivery-plan.md).

## Maintenant

1. Finir la désautorisation Elixir sur le chemin d’ingestion
2. Exposer au dashboard les métriques Rust de budget, réservations, `oversized` et modes de throttling
3. Réduire le dashboard à un rôle de visualisation, télémétrie et projection opérateur
4. Ajouter la fairness restante pour éviter l’affamement des gros fichiers différés

## Ensuite

1. Renforcer la couche de retrieval orientée développeur
2. Renforcer les garde-fous avant changement:
   - impact
   - qualité
   - régression
   - sécurité
3. Consolider la mémoire projet et la continuité `SOLL`

## Plus tard

1. Réconciliateur sémantique inter-projets
2. Raffinement des couches dérivées (`GraphProjection`, embeddings, clones sémantiques)
3. Nettoyage supplémentaire des plans historiques si une archive plus fine devient utile

## Règles de lecture

- les docs sous `docs/archive/` sont historiques
- les anciens jalons `v1.0` et `v2` ne sont plus la roadmap canonique
- les références historiques à `KuzuDB` décrivent des étapes passées; le backend nominal courant est **Canard DB** (`DuckDB`)
