# GPU Subprocess Embedding Sketch

## Intent

Ce que je suis en train d’implémenter n’est pas un “GPU dans le CPU”.

L’idée est:
- garder le pipeline CPU principal dans `axon-indexer`
- sortir **uniquement l’exécution embedding ORT/CUDA** dans un **sous-processus dédié**
- pouvoir tuer ce sous-processus quand on veut vraiment remettre à zéro l’état VRAM / ORT / CUDA

Le but est de tester un vrai `process boundary`, parce que:
- détruire une `Session` ORT dans le même process n’a pas suffi
- recréer la session après chaque batch n’a pas suffi
- la mémoire semble rester accrochée plus haut, au niveau process/runtime ORT/CUDA

## Vue Macro

```mermaid
flowchart LR
    A[File graph_ready] --> B[Prepare workers CPU]
    B --> C[Ready queue]
    C --> D[Vector worker in axon-indexer]
    D --> E[GPU embed subprocess]
    E --> F[Embeddings returned]
    F --> G[Persist workers]
    G --> H[Finalize file]
```

## Répartition des responsabilités

```mermaid
classDiagram
    class AxonIndexer {
      +claim vector work
      +prepare batches
      +schedule GPU launches
      +persist embeddings
      +finalize files
    }

    class VectorWorker {
      +pop_best()
      +send batch to subprocess
      +receive embeddings
      +trigger subprocess recycle
    }

    class GpuEmbedSubprocess {
      +load ORT/CUDA model
      +receive texts on stdin
      +run embedding
      +return embeddings on stdout
    }

    class GraphStore {
      +persist chunk embeddings
      +mark file vectorized
    }

    AxonIndexer --> VectorWorker
    VectorWorker --> GpuEmbedSubprocess
    VectorWorker --> GraphStore
```

## Séquence nominale

```mermaid
sequenceDiagram
    participant CPU as axon-indexer / VectorWorker
    participant GPU as GPU embed subprocess
    participant DB as GraphStore

    CPU->>CPU: pop_best() from ready queue
    CPU->>GPU: JSON request { texts[] }
    GPU->>GPU: tokenize + ORT/CUDA inference
    GPU-->>CPU: JSON response { embeddings[] }
    CPU->>DB: persist envelope
    DB-->>CPU: persist ok
    CPU->>DB: finalize completed works
```

## Séquence de recovery VRAM

```mermaid
sequenceDiagram
    participant CPU as axon-indexer / VectorWorker
    participant GPU as GPU embed subprocess
    participant VRAM as CUDA/ORT state

    CPU->>GPU: batch request
    GPU->>VRAM: allocate / run / cache
    Note over VRAM: VRAM may stay high even after batch

    CPU->>CPU: detect threshold / stuck condition
    CPU-xGPU: kill subprocess
    Note over GPU,VRAM: process exit should release process-owned CUDA/ORT state
    CPU->>GPU: spawn fresh subprocess
    GPU->>VRAM: load fresh model/session
```

## Ce que ce design teste

### Hypothèse

Le vrai problème n’est pas seulement la `Session` ORT.
Le vrai problème est plus probablement:
- le process ORT
- l’`Environment` global
- le contexte CUDA associé

### Donc

Si on tue le **sous-processus GPU**:
- on détruit tout l’état ORT/CUDA de ce process
- sans redémarrer tout `axon-indexer`

## Différence avec ce qu’on a déjà essayé

```mermaid
flowchart TB
    A[Attempt 1: recycle heuristics] --> A1[Same process]
    B[Attempt 2: recreate ORT session] --> B1[Same process]
    C[Attempt 3: recreate session every batch] --> C1[Still same process]
    D[Current direction: subprocess GPU] --> D1[Different process boundary]
```

## Ce que j’attends comme observation

### Si ça marche

- la VRAM doit retomber beaucoup plus franchement quand on tue le sous-processus
- on aura prouvé que le bon levier est bien `process boundary`

### Si ça ne marche pas

- alors le problème est encore plus bas niveau
- par exemple driver/runtime CUDA global à la machine
- ou mauvaise lecture de la VRAM observée

## MVP d’implémentation

```mermaid
flowchart TD
    A[VectorWorker] --> B[GpuEmbedSubprocess.spawn()]
    B --> C[Handshake init OK]
    C --> D[embed_texts(texts)]
    D --> E[JSON over stdin/stdout]
    E --> F[Embeddings back to parent]
    F --> G[Persist as today]
    G --> H[Optional kill/restart subprocess]
```

## Point important

Ce design ne dit pas encore:
- que ce sera le meilleur throughput final

Il dit:
- que c’est le prochain test propre si on veut savoir si la maîtrise VRAM exige un vrai `process boundary`
