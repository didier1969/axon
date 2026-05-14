# Axon Codebase Cleanup ID to Delivery

## Intent

Objectif: nettoyer le code Axon réel, pas seulement la doctrine, pour réduire la dette structurelle accumulée par les itérations récentes.

Ce plan vise:
- moins de code mort et de résidus legacy
- moins de surfaces critiques surchargées
- moins de cycles et de détours inutiles
- une traçabilité plus honnête entre `SOLL`, code, tests et runtime
- un socle plus simple avant toute optimisation supplémentaire

Ce plan est volontairement dur:
- on privilégie suppression, consolidation et réduction de surface
- on ne protège pas les abstractions faibles “par habitude”
- on ne lance pas de nouvelles features tant que les hot paths ne sont pas assainis

## Sources de vérité utilisées

### MCP live

- `status`
- `project_status`
- `anomalies`
- `conception_view`
- `change_safety`
- `why`
- `soll_verify_requirements`
- `soll_validate`

### Repo réel

- volumétrie fichiers
- traces `legacy/fallback/compatibility`
- état Git courant
- hotspots runtime et scripts

## Diagnostic synthétique

### 1. Le code réel est plus complexe que la base intentionnelle

La base `SOLL` est désormais propre pour `AXO`, mais le code projeté reste nettement plus sale:
- `anomalies` remonte `8 wrappers`, `4 detours`, `20 orphan code`, `20 heuristic intent gaps`, `3 cycles`, `2 god objects`
- `change_safety` considère [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:1) et [start.sh](/home/dstadel/projects/axon/scripts/start.sh:1) comme `unsafe`
- beaucoup de surfaces critiques n’ont pas de traçabilité ou de validation directe au niveau fichier

Conclusion:
- la doctrine est en avance sur l’implémentation
- la prochaine dette n’est pas conceptuelle, elle est structurelle

### 2. Les plus gros centres de complexité sont identifiés

Hotspots par taille:
- [mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs:1) `13421` lignes
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:1) `12082`
- [tools_soll.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs:1) `8607`
- [graph_ingestion.rs](/home/dstadel/projects/axon/src/axon-core/src/graph_ingestion.rs:1) `5919`
- [tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs:1) `5141`
- [main_background.rs](/home/dstadel/projects/axon/src/axon-core/src/main_background.rs:1) `4621`
- [tools_context.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs:1) `3073`
- [optimizer.rs](/home/dstadel/projects/axon/src/axon-core/src/optimizer.rs:1) `2542`
- [service_guard.rs](/home/dstadel/projects/axon/src/axon-core/src/service_guard.rs:1) `2312`
- [start.sh](/home/dstadel/projects/axon/scripts/start.sh:1) `1057`
- [cockpit_live.ex](/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex:1) `1556`
- [qualify_runtime.py](/home/dstadel/projects/axon/scripts/qualify_runtime.py:1) `2184`
- [qualify_ingestion_run.py](/home/dstadel/projects/axon/scripts/qualify_ingestion_run.py:1) `1914`

Conclusion:
- il ne faut pas “nettoyer partout”
- il faut attaquer une poignée de fichiers-monde et leurs dépendances

### 3. Les reliquats legacy et fallback sont encore trop présents

Le code et les scripts contiennent encore beaucoup de logique de compatibilité:
- `legacy_monolith`
- `legacy_compatibility_shim`
- `fallback_*`
- compatibilité de vieilles surfaces runtime
- chemins de rollback encore entremêlés dans les commandes opérateur

Ce n’est pas seulement cosmétique:
- ces branches gonflent le raisonnement
- brouillent les diagnostics
- et rendent les scripts plus difficiles à certifier

### 4. La dette la plus visible côté produit est dans la couche watcher/dashboard

Les anomalies les plus concrètes remontent surtout dans:
- `Axon.Watcher.Progress`
- `Axon.Watcher.CockpitLive`

Patterns vus:
- wrappers mono-appel
- détours à faible valeur
- symboles sans traçabilité directe

Conclusion:
- la dette UI/observer est probablement plus élevée que la dette intentionnelle
- cette zone est un bon candidat de simplification rapide

### 5. Le runtime critique reste surchargé

Les responsabilités sont encore trop entassées dans:
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:1)
- [main_background.rs](/home/dstadel/projects/axon/src/axon-core/src/main_background.rs:1)
- [graph_ingestion.rs](/home/dstadel/projects/axon/src/axon-core/src/graph_ingestion.rs:1)
- [tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs:1)
- [tools_soll.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs:1)

Symptômes:
- fonctions nombreuses et hétérogènes
- règles d’ordonnancement, instrumentation, IO, provider selection, recycle, batching et persist mélangés
- blast radius trop large

Conclusion:
- avant même de parler perf, le coût cognitif est déjà trop élevé

## Principes de nettoyage

1. Supprimer avant de déplacer.
2. Réduire les branches de compatibilité avant de raffiner les heuristiques.
3. Une responsabilité dominante par fichier critique.
4. Aucun gros fichier ne doit continuer à grossir pendant le nettoyage.
5. Toute suppression structurante doit être couverte par un signal:
   - test
   - outil MCP
   - qualification
   - ou comparaison avant/après

## Priorités dominantes

### P0. Arrêter d’augmenter la dette

Effet immédiat:
- gel des nouvelles features hors TensorRT hard cut déjà engagé
- aucun nouveau comportement ne doit être ajouté dans les fichiers déjà au-dessus de `2k` lignes sans extraction ou suppression simultanée

### P1. Réduire la surface critique runtime

Cible:
- `embedder.rs`
- `main_background.rs`
- `graph_ingestion.rs`
- `tools_framework.rs`
- `tools_soll.rs`

### P2. Nettoyer la couche opérateur et observateur

Cible:
- `scripts/start.sh`
- `scripts/qualify_runtime.py`
- `scripts/qualify_ingestion_run.py`
- `cockpit_live.ex`
- `telemetry.ex`
- `Axon.Watcher.Progress`

### P3. Éliminer le reliquat legacy explicite

Cible:
- `legacy_monolith`
- shims de compatibilité non indispensables
- fallbacks purement historiques

### P4. Reconnecter code critique et traçabilité

Cible:
- fichiers runtime à haut risque
- preuves directes
- validations plus ciblées

## Workstreams

## WS1. Runtime Critical Decomposition

### Scope

- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:1)
- [graph_ingestion.rs](/home/dstadel/projects/axon/src/axon-core/src/graph_ingestion.rs:1)
- [main_background.rs](/home/dstadel/projects/axon/src/axon-core/src/main_background.rs:1)
- [service_guard.rs](/home/dstadel/projects/axon/src/axon-core/src/service_guard.rs:1)
- [vector_control.rs](/home/dstadel/projects/axon/src/axon-core/src/vector_control.rs:1)
- [vector_pipeline.rs](/home/dstadel/projects/axon/src/axon-core/src/vector_pipeline.rs:1)

### Constats

- `embedder.rs` concentre provider selection, GPU telemetry, batching, ORT/TensorRT dispatch, subprocess service, finalize path et metrics
- `main_background.rs` mélange gouverneur, admission, reclaimer, scan, watcher, telemetry snapshot, quiescent logic
- `graph_ingestion.rs` est à la fois modèle, SQL factory, lifecycle, queue semantics et benchmark bridge

### Objectif

Transformer ces fichiers-monde en modules à responsabilité plus étroite.

### Sous-travaux

1. Extraire dans `embedder.rs`:
   - provider/runtime selection
   - GPU telemetry
   - micro-batch building
   - GPU service client/subprocess boundary
   - persist/finalize path

2. Extraire dans `main_background.rs`:
   - quiescent/governor policy
   - admission planning
   - watcher handling
   - memory pressure/reclaimer
   - runtime telemetry snapshot assembly

3. Extraire dans `graph_ingestion.rs`:
   - file/vector queue SQL
   - outbox logic
   - vector batch run data contracts
   - compatibility/bootstrap helper fragments that truly belong elsewhere

### Done criteria

- aucun de ces fichiers ne continue à porter plus d’une famille majeure de responsabilité
- diminution nette des tailles
- APIs internes plus étroites

## WS2. MCP and SOLL Surface Consolidation

### Scope

- [tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs:1)
- [tools_soll.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs:1)
- [tools_context.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs:1)
- [tools_system.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_system.rs:1)
- [mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs:1)

### Constats

- `tools_soll.rs` est à la fois policy engine, validator, docs generator, revision system et evidence tool
- `tools_framework.rs` agrège beaucoup de vérité runtime de façon très large
- les surfaces MCP sont riches, mais le code est trop monolithique

### Objectif

Réduire le coût de maintenance des surfaces MCP sans perdre la valeur produit.

### Sous-travaux

1. Séparer `tools_soll.rs` en sous-modules:
   - relation policy
   - requirement completeness
   - revision/apply/rollback
   - docs generation
   - evidence + traceability

2. Séparer `tools_framework.rs` en sous-vues:
   - runtime topology/status
   - vector/runtime diagnostics
   - change/anomaly/project diagnostics

3. Clarifier dans `tools_context.rs`:
   - retrieval nominal
   - semantic fallback
   - repo literal fallback
   - pressure-aware degradation

### Done criteria

- code MCP plus modulaire
- même surface externe
- logique de fallback localisée au lieu d’être diffuse

## WS3. Script and CLI Rationalization

### Scope

- [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh:1)
- [scripts/axon](/home/dstadel/projects/axon/scripts/axon:1)
- [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh:1)
- [scripts/stop.sh](/home/dstadel/projects/axon/scripts/stop.sh:1)
- [scripts/qualify_runtime.py](/home/dstadel/projects/axon/scripts/qualify_runtime.py:1)
- [scripts/qualify_ingestion_run.py](/home/dstadel/projects/axon/scripts/qualify_ingestion_run.py:1)
- nouveaux scripts benchmark/TensorRT

### Constats

- beaucoup de logique opérateur est devenue architecture métier
- `start.sh` est maintenant un bootstrapper, un router de runtime, un constructeur ORT/TensorRT et un gardien de compatibilité
- les scripts de qualification portent encore de la compatibilité historique et des chemins d’interprétation legacy

### Objectif

Passer d’un ensemble de scripts accumulés à une CLI plus contractuelle.

### Sous-travaux

1. Isoler dans `start.sh`:
   - runtime topology/env assembly
   - ORT artifact resolution
   - TensorRT hard gating
   - process launch

2. Réduire les chemins legacy dans les scripts de qualification.

3. Centraliser les helpers partagés au lieu de recopier des règles dans plusieurs scripts.

4. Supprimer les commandes ou aliases obsolètes une fois les chemins canoniques confirmés.

### Done criteria

- `start.sh` plus court et moins polymorphe
- moins de fallback compatibility dans les qualifications
- commandes canoniques plus évidentes

## WS4. Watcher and Dashboard Simplification

### Scope

- [cockpit_live.ex](/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex:1)
- [telemetry.ex](/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/telemetry.ex:1)
- `Axon.Watcher.Progress`
- bridge client tests et live tests associés

### Constats

- c’est la zone où MCP voit le plus de wrappers, détours et code orphelin
- elle porte encore des surfaces de transformation possiblement sur-ingéniérées

### Objectif

Réduire la couche LiveView/observer à un rôle de projection lisible.

### Sous-travaux

1. Éliminer wrappers mono-appel et helpers purement transitifs.
2. Fusionner les détours évidents dans `Progress`.
3. Supprimer les symboles orphelins non justifiés dans `CockpitLive`.
4. Faire dépendre cette couche de payloads runtime plus compacts au lieu de recomposer trop localement.

### Done criteria

- baisse mesurable des wrappers/detours/orphan code sur cette zone
- moins de helpers locaux triviaux

## WS5. Legacy Compatibility Burn-Down

### Scope

- `legacy_monolith`
- `legacy_compatibility_shim`
- anciennes lectures de surfaces runtime
- branches fallback devenues historiques

### Constats

- beaucoup de code continue à parler un langage de transition
- cela brouille la cible produit actuelle

### Objectif

Distinguer:
- compatibilité encore nécessaire
- compatibilité simplement tolérée
- compatibilité morte à retirer

### Sous-travaux

1. Inventorier chaque occurrence `legacy_*`.
2. Classer:
   - `must_keep`
   - `timeboxed_keep`
   - `remove_now`
3. Supprimer d’abord les branches non utilisées dans les commandes canoniques.

### Done criteria

- forte baisse des occurrences `legacy_*` hors tests ciblés et migrations explicites
- surface runtime plus lisible

## WS6. Traceability and Safety Hardening

### Scope

- fichiers critiques runtime
- décisions et requirements déjà présents dans `SOLL`
- validations de changement

### Constats

- `change_safety` voit `embedder.rs` et `start.sh` comme non sûrs
- la traçabilité canonique existe au niveau intentionnel, mais pas assez au niveau fichier critique

### Objectif

Reconnecter les hotspots code à la vérité canonique.

### Sous-travaux

1. attacher des preuves directes pour les fichiers critiques
2. définir des validations ciblées par zone
3. améliorer la lisibilité du lien:
   - requirement
   - décision
   - fichier
   - test/qualification

### Done criteria

- `change_safety` moins pessimiste sur les fichiers critiques les plus exposés
- meilleure audibilité avant mutation

## WS7. Test Suite Restructuring

### Scope

- [mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs:1)
- [maillon_tests.rs](/home/dstadel/projects/axon/src/axon-core/src/tests/maillon_tests.rs:1)
- qualifications Python critiques

### Constats

- `mcp/tests.rs` est trop gros pour rester un seul conteneur
- les gros modules accumulent des tests au lieu d’en clarifier la structure

### Objectif

Faire de la suite de tests une aide à la simplification, pas un dépôt d’entropie.

### Sous-travaux

1. découper `mcp/tests.rs` par famille de tool
2. rapprocher les tests des sous-modules extraits
3. retirer les tests qui ne protègent plus de comportement canonique utile

### Done criteria

- structure de tests plus locale
- blast radius plus faible

## Ordre d’exécution

### Phase A. Freeze and Inventory

1. figer l’ajout de nouvelles branches dans les hotspots
2. inventorier les branches `legacy/fallback`
3. établir une baseline:
   - tailles de fichiers
   - compteurs anomalies
   - count des occurrences `legacy_*`

### Phase B. Decompose Runtime and MCP Hotspots

1. `embedder.rs`
2. `main_background.rs`
3. `tools_soll.rs`
4. `tools_framework.rs`
5. `graph_ingestion.rs`

### Phase C. Simplify Operator and Watcher Layers

1. `start.sh` et scripts de qualification
2. `Watcher.Progress`
3. `CockpitLive` / `Telemetry`

### Phase D. Burn Down Legacy

1. branches de compatibilité runtime
2. fallbacks scripts
3. vieux aliases ou chemins opérateurs redondants

### Phase E. Reconnect Safety and Tests

1. traçabilité fichier critique
2. validations ciblées
3. découpe de la suite de tests

## Mesures de succès

### Structure

- baisse des gros fichiers critiques
- baisse du nombre d’occurrences `legacy_*`
- baisse des wrappers/detours/orphan code

### Fiabilité

- mêmes surfaces MCP canoniques après refactor
- qualifications critiques toujours vertes
- pas de régression de `soll_validate` ou `soll_verify_requirements`

### Maintenabilité

- zones critiques avec responsabilité plus claire
- moins de raisons d’avoir des scripts “magiques”
- lecture plus simple des chemins runtime

## Livrables attendus

1. inventaire nettoyé des surfaces legacy/fallback
2. sous-modules extraits pour runtime/MCP hot paths
3. scripts opérateurs rationalisés
4. couche watcher/dashboard simplifiée
5. suite de tests réorganisée
6. traçabilité directe renforcée sur les fichiers critiques

## Risques

### Risque 1. Casser des chemins live peu visibles

Réponse:
- toujours passer par `status`, `project_status`, `change_safety`, `soll_validate`
- ne pas supprimer des branches sans qualification ciblée

### Risque 2. Déplacer sans vraiment simplifier

Réponse:
- extraction seule n’est pas un succès
- il faut aussi réduire les responsabilités et supprimer des branches

### Risque 3. Nettoyer le dashboard sans toucher la dette runtime dominante

Réponse:
- dashboard/watcher n’est pas la phase 1
- il vient après les hotspots runtime/MCP

## Décision de delivery

Le nettoyage doit être traité comme une migration structurante, pas comme une série de petits refactors opportunistes.

La bonne stratégie est:
- réduire d’abord la complexité centrale
- ensuite brûler la compatibilité résiduelle
- ensuite reconnecter tests et traçabilité

Ce plan est le contrat de nettoyage recommandé avant toute nouvelle phase d’optimisation lourde du pipeline.
