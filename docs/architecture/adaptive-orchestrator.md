# Adaptive Orchestrator For Axon

## Intention
Créer un coordinateur central en boucle fermée qui maximise la complétion du backlog tout en gardant Axon utilisable et stable sous contraintes.

Le rôle du coordinateur n’est pas de remplacer les mécanismes existants, mais de piloter explicitement les leviers déjà présents dans Axon à partir des mesures déjà disponibles.

## Objectif Et Contraintes

### Objectif principal
- Maximiser la complétion du backlog.

### Contraintes dures
- Garder au moins `1/3` de la RAM système disponible.
- Garder la latence du serveur MCP sous `300 ms`.
- Garder l’usage CPU d’Axon sous `50%` de la machine.

### Formulation
Maximiser `backlog_drain_rate` sous contraintes:
- `mem_available_ratio >= 0.33`
- `mcp_latency_p95 <= 300 ms`
- `cpu_usage_ratio <= 0.50`

## Architecture Conceptuelle

Le système se décompose en quatre blocs:

1. `Flux entrants`
- watcher / scanner
- queue de vectorisation
- demandes interactives MCP
- contraintes hôte CPU / RAM / GPU

2. `Sondes`
- backlog depth
- coût moyen d’embed par chunk
- coût par batch
- service pressure
- latence MCP
- mémoire disponible
- provider effectif (`cuda`, `cpu_fallback`, etc.)

3. `Coordinateur central`
- lit l’état courant
- vérifie les contraintes
- choisit un état de drain
- ajuste progressivement les vannes

4. `Actionneurs`
- `AXON_VECTOR_WORKERS`
- `AXON_CHUNK_BATCH_SIZE`
- `AXON_FILE_VECTORIZATION_BATCH_SIZE`
- `AXON_GRAPH_WORKERS`
- politique d’admission du travail en fond
- priorité interactive / pauses temporaires

## Leviers Déjà Présents Dans Axon

### Admission de travail de fond
- `compute_claim_policy(...)` dans [main_background.rs](/home/dstadel/projects/axon/src/axon-core/src/main_background.rs:1240)
- agit sur:
  - `mode`
  - `claim_count`
  - `sleep`

### Vannes embedding / vectorisation
- `AXON_VECTOR_WORKERS`
- `AXON_CHUNK_BATCH_SIZE`
- `AXON_FILE_VECTORIZATION_BATCH_SIZE`
- `AXON_GRAPH_WORKERS`
- `AXON_MAX_EMBED_BATCH_BYTES`

Sources:
- [main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs:150)
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:2152)

### Garde-fous existants
- `vector_worker_admitted(...)` dans [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:2437)
- `semantic_policy(...)` dans [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:2471)
- `current_vector_drain_state(...)` dans [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:2346)

## Sondes Déjà Disponibles

### Backlog et queues
- `fetch_file_vectorization_queue_counts()` dans [graph_ingestion.rs](/home/dstadel/projects/axon/src/axon-core/src/graph_ingestion.rs:1155)
- exposé dans la surface système MCP via [tools_system.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_system.rs:240)

### Pression service et priorité interactive
- `service_guard::current_pressure()`
- `interactive_priority_active()`
- métriques de suppression / interruption / reprise

Source:
- [service_guard.rs](/home/dstadel/projects/axon/src/axon-core/src/service_guard.rs:199)

### Coût réel du pipeline vectoriel
- `vector_runtime_metrics()`
- temps:
  - fetch
  - embed
  - db write
  - mark done
  - prepare / finalize queues
- volumes:
  - chunks embedded
  - files completed
  - embed calls
  - files touched

Source:
- [service_guard.rs](/home/dstadel/projects/axon/src/axon-core/src/service_guard.rs:364)

### État provider
- `current_embedding_provider_diagnostics()`
- utile pour distinguer:
  - GPU actif
  - CPU pur
  - CPU fallback
  - mismatch demandé/effectif

Source:
- [embedder.rs](/home/dstadel/projects/axon/src/axon-core/src/embedder.rs:2048)

## État Réel Observé

Le signal dominant actuel est que le coût d’ingestion est dominé par `embed_transform`, pas par la base ni par la préparation.

Conséquences:
- le problème principal n’est pas la DB
- le problème principal n’est pas le writer
- le problème principal est le coût du modèle / runtime embedding et sa régulation

Un point important identifié pendant l’analyse:
- le host est initialement configuré comme un host GPU quand `gpu_present = true`
- si CUDA échoue et qu’Axon tombe en `cpu_fallback`, il faut réajuster le runtime CPU de façon cohérente
- sinon on peut rester artificiellement trop prudent côté OpenMP / débit

## Logique Initiale Recommandée

### Principe
- ouvrir lentement
- fermer vite
- n’ajuster qu’une vanne majeure à la fois

### Boucle
1. Lire backlog, latence MCP, RAM disponible, CPU, coût embed, provider effectif.
2. Vérifier les contraintes dures.
3. Si une contrainte casse:
- réduire immédiatement le débit de fond
- protéger l’interactif
4. Si tout est stable:
- augmenter progressivement une seule vanne
- observer l’effet pendant une fenêtre courte
5. Si le gain est réel sans violation:
- conserver
6. Sinon:
- revenir au réglage précédent

## États Utiles

- `QuietCruise`
  - backlog faible
  - pas d’urgence

- `AggressiveDrain`
  - backlog élevé
  - contraintes encore respectées

- `Recovery`
  - service sous pression
  - reprise prudente

- `InteractiveGuarded`
  - priorité absolue à la fluidité MCP

- `GpuScalingBlocked`
  - GPU demandé mais non effectif
  - ne pas continuer à raisonner comme un host GPU sain

## Recommandations D’Implémentation

### V1
- introduire un vrai module `AdaptiveOrchestrator`
- stocker les décisions prises et les mesures observées
- sortir un état synthétique lisible dans la surface MCP
- faire de l’ajustement heuristique simple et explicable

### V2
- apprendre des réglages utiles par fenêtre de charge
- comparer plusieurs politiques de débit
- ajouter optimisation multi-objectifs plus formelle

### V3
- introduire des techniques plus avancées seulement si la V1/V2 plafonne:
  - bandits
  - Bayesian optimization
  - contrôle prédictif

## À Ne Pas Faire
- ne pas empiler de la “smartness” opaque avant d’avoir un historique fiable
- ne pas modifier plusieurs vannes à la fois sans attribution causale claire
- ne pas optimiser uniquement le débit en sacrifiant l’usage MCP
- ne pas traiter un host `cpu_fallback` comme un vrai host GPU

## Artefact Visuel
- Vue visuelle: [adaptive-orchestrator.html](/home/dstadel/projects/axon/docs/architecture/adaptive-orchestrator.html)

