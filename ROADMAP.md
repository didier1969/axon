# Roadmap Axon

Ce document décrit la **suite rationnelle après livraison** à partir de l’état vérifié au `2026-04-01`.

Il ne remplace pas `STATE.md`, qui porte la photographie de vérité exécutable.
Le plan maître livré est [docs/plans/2026-04-01-axon-delivery-plan.md](/home/dstadel/projects/axon/docs/plans/2026-04-01-axon-delivery-plan.md).

## Maintenant

1. Étendre la retrieval sémantique au-delà du rang symbole, notamment vers les chunks et voisinages graphe quand cela apporte un vrai gain développeur
2. Exposer au cockpit un état explicite de disponibilité sémantique (`hybride disponible` vs `fallback structurel`) sans créer de boucle de feedback
3. Continuer à durcir les workflows développeur `impact`, `audit`, `quality` et `risk` sur des cas réels de projets mixtes

## Ensuite

1. Consolider encore la fraîcheur et l’invalidation des couches dérivées (`ChunkEmbedding`, `GraphEmbedding`, projections)
2. Renforcer l’analyse inter-projets seulement après preuve d’utilité sur le mono-projet
3. Raffiner le signal de pression hôte si l’usage réel montre encore des angles morts sur WSL ou grosses machines

## Plus tard

1. Réconciliateur sémantique inter-projets
2. Ergonomie cockpit et surfaces MCP avancées
3. Nettoyage supplémentaire des plans historiques si une archive plus fine devient utile

## Règles de lecture

- les docs sous `docs/archive/` sont historiques
- les anciens jalons `v1.0` et `v2` ne sont plus la roadmap canonique
- les références historiques à `KuzuDB` décrivent des étapes passées; le backend nominal courant est **Canard DB** (`DuckDB`)
