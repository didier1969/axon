# Pipeline v2 — Scorecard Qualite (2026-05-25)

Qualification du pipeline v2 Axon sur 4 axes, basee sur lecture exhaustive
des 15 fichiers specifies. Bench de reference : 132 ch/s sustained, RTX
3070 + Ryzen 7 5800H 8C/16T, cible 150 ch/s.

---

## Axe 1 — Throughput (73/100)

### Observations positives

1. **Batching A3 amortise le cliff PG** (`channels.rs:49-63`, `stage_a3.rs:102-314`).
   Le batch_size=32, batch_timeout=10ms elimine le goulot transaction-per-file
   qui faisait chuter le debit de 57 a 22 ch/s avec A3=6 workers. Le round-robin
   dispatcher (`orchestrator.rs:302-331`) offre un scaling horizontal propre.

2. **B1 bucket-sort pre-trie par token_count** (`orchestrator.rs:446-463`).
   `b1_pool_size = 4 * b2_batch_size` (256 par defaut) et le SQL ORDER BY
   token_count donnent a B2 des batches homogenes en longueur de sequence.
   Cela minimise le padding TensorRT dans chaque bucket (64/128/256/512) et
   maximise le GPU compute density. Innovation architecturale significative.

3. **B2 batch_size=64 avec timeout 200ms** (`channels.rs:36-47`,
   `stage_b2.rs:121-241`). Le dimensionnement correspond au sweet spot
   ORT/TensorRT mesure (280 ch/s peak vs 10 ch/s a batch=1). Le pattern
   first-item-blocking + deadline-drain est correct et economique.

4. **GPU utilisation 85-89%** est remarquable pour un pipeline
   heterosynchro CPU-PG-GPU. La separation A pipeline (CPU-bound, non-
   bloquant via `try_send`) et B pipeline (GPU-paced) est le bon design.

5. **Dedup cache entre A1 et A2** (`indexed_file_cache.rs`,
   `orchestrator.rs:233-266`). ArcSwap + DashMap = lock-free read path,
   atomic bulk reload. Elimine le tree-sitter parse sur les fichiers
   inchanges, ce qui est la majorite du steady-state.

### Faiblesses

1. **A3 est le drum a 99.6% work_ratio** et le pipeline ne depasse pas
   132 ch/s. Le batched writer fait une seule `execute_batch` par flush,
   ce qui serialise la charge PG. Avec A3=2 workers (defaut), chaque
   worker monopolise un slot deadpool pendant toute la duree de son
   `spawn_blocking` — le parallelisme reel est 2, pas N.

2. **try_send A3->B1 drop silencieux** (`stage_a3.rs:254`). Les chunks
   dropped pendant le buffer-full dependent du cold-start poll (30s par
   defaut) pour etre rattrapes. Pendant cette fenetre de 30s, B2 peut
   tourner a vide (pas de donnee). La fenetre est une perte de throughput.

3. **Bootstrap scan utilise try_send avec sleep 50ms sur Full**
   (`pipeline_v2_runtime.rs:344-365`). A haute cardinalite de fichiers
   (130K+), le taux de drop initial est eleve. La reconciliation scope
   60s rattrape, mais le premier passage perd du temps.

### Recommandations

1. **A3 parallel batching** : passer a 4 workers A3 avec des pipelines
   PG distincts (chacun avec sa propre connection pool). Le round-robin
   existant fonctionne deja ; le bottleneck est le contention sur
   execute_batch a travers le meme pool. Alternativement, pre-rendre
   les SQL sur le runtime tokio (non-blocking) et n'envoyer que le
   batch de strings rendues au spawn_blocking.

2. **Reduire le cold-start poll interval de 30s a 5-10s** quand le
   B2 GPU est idle (detecte via `t_recv_ratio > 0.9` sur B2). Le
   watchdog lifecycle sait quand le GPU est affame.

---

## Axe 2 — Simplicite architecturale (78/100)

### Observations positives

1. **Topologie lisible A1->A2->A3 | B1->B2->B3** (`orchestrator.rs`).
   Le wiring est lineaire, chaque stage a un fichier dedie, le flux de
   donnees est unidirectionnel. Le `PipelineAHandles` / `PipelineBFullHandles`
   expose une API minimale (input_tx, output_rx, metrics).

2. **Pattern de batching uniforme** : les trois stages batches (A3, B2, B3)
   partagent exactement le meme squelette : loop { recv first -> drain
   until batch_size or timeout -> spawn_blocking(flush) -> forward
   receipts }. Chaque implementation fait ~100-150 lignes. Ce pattern
   est lisible et maintenable.

3. **`spawn_stage_workers` generique** (`worker_pool.rs:39-104`). Le
   generic helper encapsule le Mutex-guarded receiver, le record_started/
   finished/error, et la backpressure detection en ~60 lignes. Les stages
   non-batches (A1, A2) l'utilisent tel quel.

4. **`PipelineChannelCaps` centralise tous les knobs** (`channels.rs`).
   12 constants avec defaults documentes, chacune overridable par env var
   avec un pattern identique de parsing. Pas de magie, pas de config file.

5. **Tests E2E inline** (`orchestrator.rs:540-815`). Le module `tests`
   de l'orchestrateur couvre le happy path A seul, A+B1, A+B full, les
   erreurs d'extension, et les worker counts defaults. Couverture correcte.

### Faiblesses

1. **Duplication round-robin dispatcher** (`orchestrator.rs:302-331` et
   `orchestrator.rs:501-529`). Le bloc de dispatch A3 multi-worker et B3
   multi-worker est identique a 3 lignes pres (le type du channel item).
   Un extracteur generique `spawn_round_robin_dispatcher<T>` eliminerait
   ~50 lignes.

2. **B1InboxItem::Inline vs FetchById** (`stage_b1.rs:42-46`) ajoute un
   concept (`B1InboxItem` enum) qui complique le flux mental. La justification
   perf est reelle (elimine 1 SELECT PG par chunk en steady-state), mais
   le dispatching dans `spawn_b1_batched_worker` (`stage_b1.rs:196-204`)
   avec deux branches de forwarding est un cas de complexite accidentelle
   liee a l'optimisation. Le design "pur" serait un seul type avec content
   optionnel.

3. **`pipeline_v2_runtime.rs` fait 900+ lignes** avec du wiring, du sweep
   periodique, du CPU gating, de la resolution DATABASE_URL. La
   responsabilite est trop large pour un seul module — le sweep worker
   et le CPU gating pourraient etre extraits.

### Recommandations

1. **Extraire `spawn_round_robin_dispatcher<T>`** dans `worker_pool.rs`
   pour DRY le pattern A3/B3.

2. **Scinder `pipeline_v2_runtime.rs`** en `runtime_wiring.rs` (spawn
   pipeline A+B, drain loop) et `sweep_worker.rs` (periodic sweep +
   CPU gate).

---

## Axe 3 — Optimum ingenierie (81/100)

### Observations positives

1. **Tous les accede PG sont dans `spawn_blocking`** (`stage_a3.rs:79-90`,
   `stage_b1.rs:113-116`, `stage_b2.rs:87-96`, `stage_b3.rs:61-70`).
   Le runtime tokio n'est jamais bloque par du SQL synchrone ou du GPU
   synchrone. C'est le pattern correct pour un runtime multi-threaded.

2. **Metriques Goldratt t_recv/t_work/t_send** (`metrics.rs:1-170`).
   Le triplet temporel permet de detecter automatiquement le drum
   (`argmax(t_work_ratio)`), la starvation (t_recv eleve), et la
   backpressure (t_send eleve). C'est de l'observabilite de classe
   industrielle, bien au-dessus de la moyenne des pipelines de donnees.

3. **Propagation de shutdown par drop de channel** (`orchestrator.rs:168-170`).
   Drop `input_tx` -> A1 recv() = None -> ferme a1_to_a2_tx -> cascade
   A2, A3. Pas de shutdown flag, pas de CancellationToken, pas de
   coordination explicite. Elegant et correct.

4. **Idempotence bout-en-bout** (`stage_a3.rs:63`, `stage_b3.rs:46-82`).
   Tous les UPSERT utilisent `ON CONFLICT DO UPDATE`. Re-indexer un
   fichier ou re-embedder un chunk est un no-op. Le cold-start poll
   peut tourner sans limite sans effet de bord.

5. **EmbedderLifecycle watchdog** (`pipeline_v2_runtime.rs:165-177`).
   Liberation VRAM apres 5min d'idle, reveil en 1-3s via cache TensorRT
   disque. Sur un GPU 8 Go (RTX 3070), liberer 5-7 Go de VRAM quand
   l'indexeur est idle est critique pour les workloads concurrents.

### Faiblesses

1. **Clone massif des `ParsedFile` dans `upsert_graph_v2_batch`**
   (`stage_a3.rs:215`, `graph_ingestion.rs:1195-1196`). Le batch entier
   est clone avant d'entrer dans le spawn_blocking (`group_for_block =
   group_batch.clone()`). Pour un batch de 32 fichiers dont chacun porte
   le `content: String` complet, c'est potentiellement plusieurs Mo de
   copies inutiles. Un `Arc<ParsedFile>` ou un passage par reference via
   scope task resoudrait.

2. **`per_item_us` approximatif dans A3** (`stage_a3.rs:234-235`).
   `elapsed_us / total_items` est calcule une seule fois pour tout le
   batch mais utilise pour chaque item individuellement. Si le batch
   contient des projets de tailles tres differentes, la metrique
   `mean_duration_us` est biaisee. Pas un bug, mais une source de
   confusion diagnostique.

3. **Tokenizer clone dans `token_count_for_text`** (`embedding_profile.rs:214-229`).
   `load_runtime_embedding_tokenizer()` retourne `Ok(tokenizer.clone())`
   depuis un `OnceLock`. Chaque appel clone l'objet `Tokenizer` complet
   (qui inclut le vocabulaire BPE). Avec ~19K chunks, ca fait ~19K clones
   d'un tokenizer de ~50 Mo. Un `Arc<Tokenizer>` dans le OnceLock
   eliminerait les copies.

### Recommandations

1. **Passer `ParsedFile` par `Arc`** dans le batching A3 pour eliminer
   les clones de contenu. Le pattern `Arc::new(parsed)` au moment de
   la reception dans le buffer, puis passage par reference dans le
   spawn_blocking.

2. **Remplacer `OnceLock<Result<Tokenizer, String>>` par
   `OnceLock<Arc<Tokenizer>>`** dans `embedding_profile.rs` pour eviter
   le clone a chaque appel. Impact direct sur la latence A2 (chunking).

---

## Axe 4 — Qualite recherche semantique (72/100)

### Observations positives

1. **Structure chunk riche** (`code_chunker.rs:166-195`). Chaque chunk
   porte un header semantique : `symbol: <name>\nkind: <kind>\n
   docstring: <text>\npart: N/M\ncontext:\n<header>`. Ce prefixe donne
   au modele d'embedding un ancrage structurel que le code brut n'a pas.
   C'est une bonne pratique pour BGE-Large qui beneficie du contexte
   textuel.

2. **Fusion des petits chunks** (`code_chunker.rs:342-430`). Les chunks
   < 100 tokens sont fusionnes avec leurs voisins adjacents par start_line.
   Avant fusion, 26.8% des chunks etaient < 100 tokens (degeneres pour
   BGE-Large qui a besoin de masse textuelle pour discriminer). La fusion
   reduit ce pourcentage et ameliore la densite semantique.

3. **target_chunk_tokens = 384** (`embedding_profile.rs:37-42`). 75%
   du max 512 laisse une marge pour le header semantique (symbol/kind/
   docstring/part/context) sans risquer la troncature. C'est le bon
   compromis entre couverture et granularite.

4. **Structural split heuristics** (`code_chunker.rs:122-155`). Le
   choix du point de coupure favorise les frontieres de blocs (blank
   lines, dedent, brace close) plutot que le milieu brut. Le scoring
   multi-critere (blank=200, dedent=120, block_close=90, depth penalty,
   distance penalty) est bien calibre pour du code multi-langage.

5. **Idempotence du chunk_id** (`graph_ingestion.rs:994-999`). Le
   chunk_id est derive de `symbol_id + part_index/part_count`, ce qui
   rend les chunk_ids stables entre re-indexations. Un fichier modifie
   re-genere les memes chunk_ids si la structure de symboles ne change
   pas, evitant des orphelins dans ChunkEmbedding.

### Faiblesses

1. **Pas de chunk "file-level"**. Seuls les symboles detectes par
   tree-sitter generent des chunks. Le code entre les symboles (imports,
   commentaires de module, constantes top-level, configuration) n'est
   pas indexe. Pour un `Cargo.toml`, un `README.md`, un `config.yaml`,
   aucun chunk n'est genere car tree-sitter ne detecte pas de "symboles"
   au sens fonction/struct/class. Cela cree des trous de couverture
   significatifs dans la recherche semantique.

2. **Pas d'instruction-prefix pour BGE-Large**. Le modele BGE-Large-v1.5
   a ete entraine avec le prefixe `"Represent this sentence: "` pour
   les passages et `"Represent this sentence for searching relevant passages: "`
   pour les queries. L'absence de ces prefixes dans le pipeline degrade
   la qualite de l'embedding de 5-15% selon les benchmarks MTEB publics.
   Ni le chunker (`code_chunker.rs`) ni l'embedder (`stage_b2.rs`) ne
   les injectent.

3. **Pas de deduplication semantique inter-fichiers**. Si le meme code
   est copie-colle dans 3 fichiers, il genere 3 chunks quasi-identiques
   avec 3 embeddings presque identiques. Le `semantic_clones` MCP tool
   le detecte a posteriori, mais le pipeline n'a pas de gate pour eviter
   d'embedder des doublons connus. Avec 19K chunks, le ratio de
   duplication est probablement bas, mais sur un mono-repo multi-projet
   il peut devenir significatif.

### Recommandations

1. **Ajouter un chunk file-level** pour les fichiers de configuration,
   les imports modules, et tout contenu entre symboles. Un chunk
   `kind: file_context` avec le top-level content (first N tokens)
   comblerait les trous de couverture. Impact majeur sur la qualite
   de `retrieve_context` pour les questions architecturales.

2. **Injecter le BGE-Large passage prefix** (`"Represent this sentence: "`)
   dans `stage_b2.rs` avant l'appel `embed_batch`, et le query prefix
   correspondant dans le retrieve path. C'est un changement de 2 lignes
   qui peut ameliorer le recall de 5-15% sans cout GPU additionnel.

---

## Score composite

| Axe | Poids | Score | Pondere |
|---|---|---|---|
| Throughput | 25% | 73 | 18.25 |
| Simplicite architecturale | 20% | 78 | 15.60 |
| Optimum ingenierie | 25% | 81 | 20.25 |
| Qualite recherche semantique | 30% | 72 | 21.60 |
| **Composite** | **100%** | | **75.7** |

### Synthese

Le pipeline v2 est un systeme d'ingenierie solide, bien au-dessus de la
moyenne des pipelines d'indexation de code open-source. Les points forts
sont l'observabilite Goldratt (metriques temporelles t_recv/t_work/t_send),
le batching B1 bucket-sort, et le design de shutdown par propagation de
channel drop.

Les gains les plus accessibles (impact/effort) sont :

1. **BGE prefix injection** (qualite +5-15%, effort 2 lignes) — axe 4
2. **Arc<Tokenizer> dans OnceLock** (latence A2, effort 10 lignes) — axe 3
3. **File-level chunks** (couverture, effort ~100 lignes) — axe 4
4. **Extract round-robin dispatcher** (maintenance, effort ~30 lignes) — axe 2
5. **A3 parallel batching / pre-render SQL** (throughput, effort moyen) — axe 1
