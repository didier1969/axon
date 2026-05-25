# Session 55 — Validation Report (2026-05-25)

Audit des 7 commits session 55 (ce32fd1a..bb7399d9) par un expert Rust senior.
Scope : pipeline async tokio, GPU ONNX/TensorRT, PostgreSQL bulk write.

---

## Findings

### F-01 : COPY BINARY Chunk omet `token_count`

- **Severite** : ELEVE
- **Fichier** : `src/axon-core/src/postgres/bulk_writer.rs:679-776`
- **Description** : `copy_chunks_in_tx` ecrit 13 colonnes dans le staging table (`_bulk_chunk_stage`) et le merge `INSERT INTO public.Chunk` ne mentionne pas `token_count`. Pourtant la DDL du table Chunk (db/ddl/03_ist_schema.sql:97) contient `token_count INTEGER`, la structure `ChunkRow` (async_writer.rs:71) a `pub token_count: Option<i64>`, et le renderer SQL non-COPY (`render_chunks_pg`) ecrit cette colonne correctement. Sous le chemin COPY BINARY (`AXON_BULK_WRITER_ENABLED=true`), toutes les lignes Chunk recevront `token_count = NULL`.
- **Impact** : Le `ORDER BY COALESCE(c.token_count, length(c.content) / 3)` dans les SELECTs B1 (`fetch_chunks_for_embedding_batch`) applique le fallback `length(content)/3` au lieu de la valeur exacte du tokenizer. Le tri par bucket est degrade : les batches GPU croisent des limites de sequence TensorRT, augmentant le padding et reduisant le throughput effectif. Pas un bug fonctionnel (fallback existe), mais une regression de performance directe sur le chemin optimise.
- **Correction suggeree** : Ajouter `token_count INTEGER` au staging DDL, `Type::INT4` (pas INT8 : la DDL dit `INTEGER`) au tableau column_types, `&row.token_count` au write, et la colonne dans les clauses SELECT/INSERT/ON CONFLICT. Attention : le type PG est `INTEGER` (i32), pas `BIGINT` (i64) ; le ChunkRow stocke `i64` donc il faut un cast `as i32` ou passer `Option<i32>`.

---

### F-02 : Metriques B1 double-comptage sur dedup skip

- **Severite** : MOYEN
- **Fichier** : `src/axon-core/src/pipeline_v2/stage_b1.rs:233-258`
- **Description** : Dans `spawn_b1_batched_worker_with_dedup`, quand un Inline item est dedup-skipped (cache hit, ligne 243-245), le code appelle `metrics.record_started()` + `metrics.record_finished(0)`. Ensuite, la boucle ligne 256-258 appelle `metrics.record_started()` pour TOUS les items du batch, y compris ceux deja comptes. Les items dedup-skipped recoivent donc 2x `record_started()` et 1x `record_finished()`, ce qui fausse `items_in_total` et potentiellement `inflight`.
- **Impact** : Les snapshots de metriques B1 surestiment `items_in_total` et sous-estiment `inflight` pour les items dedupliques. Pas de corruption de donnees, mais le diagnostic Goldratt (drum identification, Little's Law sanity check) est biaise.
- **Correction suggeree** : Retirer les appels `record_started()` / `record_finished(0)` du bloc dedup-skip (lignes 244-245), et laisser la boucle globale (ligne 256) compter uniformement. Ou inverser : exclure les dedup-skipped de la boucle globale ligne 256 en ne bouclant que sur `inline_payloads.len() + fetch_ids.len()` au lieu de `batch.len()`.

---

### F-03 : Truncation UTF-8 par index de bytes dans file-level chunks

- **Severite** : ELEVE
- **Fichier** : `src/axon-core/src/graph_ingestion.rs:1221-1222`
- **Description** : `&parsed.content[..2000]` effectue un slicing par position de byte. Si le byte 2000 tombe au milieu d'un code point multi-octets (e.g. commentaire en francais avec accents, code CJK, emoji), cela provoque un `panic!` immediat en Rust. Le code apparait dans `upsert_graph_v2_batch`, appele depuis le worker A3 via `spawn_blocking`.
- **Impact** : Le panic depuis `spawn_blocking` remonte comme `JoinError` et declenche `record_error()` pour tout le groupe. Le fichier concerne n'est jamais indexe. Fichiers impactes : tout fichier sans symboles mais avec du contenu non-ASCII de plus de 2000 bytes (README internationaux, fichiers de configuration avec commentaires non-ASCII, etc.).
- **Correction suggeree** : Utiliser une truncation safe avec `parsed.content.char_indices()` :
  ```rust
  let truncated = match parsed.content.char_indices().nth(2000) {
      Some((idx, _)) => &parsed.content[..idx],
      None => &parsed.content,
  };
  ```
  Ou bien `parsed.content.chars().take(2000).collect::<String>()` si la semantique souhaitee est 2000 caracteres et non 2000 bytes.

---

### F-04 : Prefixe BGE indexation vs recherche — asymetrie deliberee mais fragile

- **Severite** : INFO
- **Fichier** : `src/axon-core/src/pipeline_v2/stage_b2.rs:184-186` et `src/axon-core/src/embedder.rs:2010-2012`
- **Description** : Le prefixe B2 (indexation) est `"Represent this sentence: "` tandis que le prefixe query (`batch_embed`) est `"Represent this sentence for searching relevant passages: "`. BGE-Large-v1.5 recommande officiellement : indexation = `"Represent this sentence: "`, recherche = `"Represent this sentence for searching relevant passages: "`. L'asymetrie est donc **correcte** selon le papier BGE et le README Hugging Face. Ce n'est pas un bug.
- **Observation** : Le fait que les deux prefixes soient hardcodes a deux endroits differents sans constante partagee cree un risque de desynchronisation lors de futures modifications. Considerer l'extraction en constantes dans `embedding_contract.rs`.

---

### F-05 : Embedding dedup cache non mis a jour apres B3 write

- **Severite** : ELEVE
- **Fichier** : `src/axon-core/src/pipeline_v2/stage_b1.rs:24-41` et `src/axon-core/src/pipeline_v2/stage_b3.rs:84-241`
- **Description** : Le `DashMap<chunk_id, source_hash>` (embedding dedup cache) est hydrate au boot par `load_embedding_dedup_cache` puis consulte par B1 pour skipper les chunks inchanges. Cependant, apres que B3 persiste un nouvel embedding, **le cache n'est jamais mis a jour**. Il n'y a aucun appel `cache.insert(chunk_id, source_hash)` dans `spawn_b3_batched_worker` ni dans `b3_persist_embedding`.
- **Impact** :
  - **Premiere indexation** : pas de probleme, le cache est vide au boot, tous les chunks passent.
  - **Re-indexation intra-session** : si un fichier est re-indexe dans la meme session (e.g. modification rapide), A3 envoie un Inline avec un nouveau `content_hash`. B1 consulte le cache qui ne connait pas ce chunk_id (jamais insere apres B3 success) => le chunk passe correctement.
  - **Cependant, au prochain re-index du meme fichier SANS changement** : le cache ne connait toujours pas le chunk_id => B1 NE skip PAS => le chunk est re-embeddable inutilement, exactement le scenario que le cache devait eviter.
  - En resume : le cache ne protege que le premier cycle (boot -> premier passage). Tous les re-indexes subsequents dans la meme session ne beneficient pas du dedup cache car il n'est jamais rafraichi.
- **Correction suggeree** : Apres un B3 write reussi, mettre a jour le cache :
  ```rust
  if let Some(ref cache) = embedding_cache {
      cache.insert(embedded.chunk_id.clone(), embedded.source_hash.clone());
  }
  ```
  Cela necessite de passer le `EmbeddingDedupCache` jusqu'au worker B3 ou d'utiliser un `Arc` partage.

---

### F-06 : Double-buffering B2 et recompilation TensorRT engine

- **Severite** : MOYEN
- **Fichier** : `src/axon-core/src/pipeline_v2/orchestrator.rs:496-498`
- **Description** : Avec `n_b2 = 2` embedders, le batch_size par worker est divise par 2 (`caps.b2_batch_size / n_b2`). Si le default est `b2_batch_size=64`, chaque worker recoit `batch_size=32`. TensorRT compile et cache des execution plans par combinaison unique de `(model, batch_size, seq_len_bucket)`. Un engine pour `batch=64` et un engine pour `batch=32` sont deux engines distincts.
- **Impact** :
  - Si TensorRT avait deja un engine cache pour `batch=64`, il doit maintenant compiler un engine supplementaire pour `batch=32`. Premiere compilation ~5-10s. L'engine cache est persistant sur disque (`/tmp/trt-cache/` ou `.cache/axon/`), donc ce cout n'est paye qu'une fois.
  - Le padding interne TensorRT est optimise pour le batch_size exact du plan : `batch=32` peut etre plus efficace que `batch=64` a demi-rempli si les batches partiels etaient frequents avec l'ancien single-worker.
  - Le temps de compilation n'est PAS double car les deux workers ne compilent pas simultanement (ORT session init est sequentiel au premier `embed_batch`). Apres la premiere compilation, les engines sont caches.
- **Verdict** : Pas un bug. Impact performance unique au premier run. La documentation (CLAUDE.md `bench`) devrait mentionner un warmup plus long avec `AXON_B2_WORKERS > 1`.

---

### F-07 : Channel lifecycle — drop b1_inbox_tx dans le bench

- **Severite** : INFO
- **Fichier** : `src/axon-core/src/bin/axon-bench-pipeline-v2.rs:266`
- **Description** : Le bench drop explicitement `handles_a.b1_inbox_tx` pour debloquer la cascade shutdown B. C'est correct : sans ce drop, le channel `b1_inbox` garde un sender vivant (celui du poll cold-start que le bench n'utilise pas), et `recv()` dans B1 ne retourne jamais `None`.
- **Production (`pipeline_v2_runtime`)** : le sender supplementaire est clone vers `b1_inbox_tx_for_poll` et `b1_inbox_tx_for_listener` qui vivent dans des tasks `tokio::spawn` infinite-loop. En production, le shutdown se fait par SIGTERM → tokio runtime drop, pas par cascade de channels. La divergence bench/prod est correcte.
- **Risque residuel** : le multi-embedder dispatcher (orchestrator.rs:509-533) cree des `worker_txs` qui sont dropped quand le dispatcher task termine. Si le dispatcher panic (recv error), les `worker_txs` sont dropped ce qui cascade correctement vers les B2 workers. Pas de sender leak identifie.

---

### F-08 : fuse_small_chunks — stabilite du chunk_id entre re-indexations

- **Severite** : MOYEN
- **Fichier** : `src/axon-core/src/code_chunker.rs:361-430`
- **Description** : `fuse_small_chunks` fusionne les chunks adjacents dont `estimated_tokens < MIN_FUSE_TOKENS` (100). Le `chunk_id` du groupe fusionne est derive du `symbol_id` du premier element. Si les symboles changent legerement (un symbole renomme, un nouveau symbole ajoute entre deux existants), les groupes de fusion changent :
  - Le chunk_id du groupe fusionne change (premier symbole different)
  - L'ancien chunk_id n'existe plus => l'ancien embedding est orphelin dans ChunkEmbedding
  - Le nouveau chunk_id necessite un nouveau embedding GPU
- **Impact** : La stabilite des chunk_ids est degradee pour les fichiers avec beaucoup de petits symboles (< 100 tokens chacun). Un changement mineur dans un tel fichier peut invalider tous les embeddings de tous les groupes fusionnes. Cela dit, le `ON CONFLICT (id) DO UPDATE` dans Chunk assure la coherence ; les embeddings orphelins dans ChunkEmbedding ne causent pas de corruption, juste du travail GPU inutile.
- **Attenuation existante** : Le `content_hash` du chunk fusionne change aussi, ce qui fait que le B1 dedup cache (s'il fonctionnait correctement, cf F-05) le reembarquerait de toute facon. Le surcout est proportionnel au nombre de fichiers avec beaucoup de micro-symboles.

---

### F-09 : Arc<Tokenizer> thread safety

- **Severite** : INFO
- **Fichier** : `src/axon-core/src/embedding_profile.rs:214-224`
- **Description** : `tokenizers::Tokenizer` (crate `tokenizers` v0.22.1 de Hugging Face) implemente `Send + Sync`. Le type est safe pour un partage via `Arc<Tokenizer>` dans un `OnceLock`. Pas de probleme ici.
- **Preuve** : le code compile (`Arc::new(tokenizer)` dans un `OnceLock<Result<Arc<Tokenizer>, String>>`) et le compilateur Rust aurait refuse `Arc<T>` si `T` n'etait pas `Send + Sync`. La version 0.22.1 du crate confirme l'implementation.

---

### F-10 : Edge COPY BINARY — colonne `metadata` non ecrite

- **Severite** : INFO
- **Fichier** : `src/axon-core/src/postgres/bulk_writer.rs:838-884`
- **Description** : La DDL de `public.Edge` inclut `metadata JSONB`, mais le COPY BINARY staging table et le merge INSERT ne mentionnent pas cette colonne. La valeur sera donc `NULL`.
- **Impact** : Le renderer SQL non-COPY (`render_unified_edge_pg` dans async_writer.rs) ne mentionne pas non plus `metadata` dans ses INSERTs — la colonne est reservee pour un usage futur (annotations, poids). La parite entre les deux chemins est maintenue. Pas de regression.

---

### F-11 : file-level chunk_id stabilite

- **Severite** : INFO
- **Fichier** : `src/axon-core/src/graph_ingestion.rs:1220`
- **Description** : Le chunk_id `{project_code}::{path}::file_context::chunk` est stable entre re-indexations car il ne depend que du chemin fichier et du project_code. Le `content_hash` est `stable_content_hash(&file_content)` ou `file_content` inclut le path et le contenu tronque. Si le contenu change, le hash change, le chunk est mis a jour via `ON CONFLICT DO UPDATE`, et B1 peut le reembarquer.
- **Verdict** : Stable et correct.

---

## Resume par commit

| Commit | Scope | Verdict |
|--------|-------|---------|
| `ce32fd1a` | drop b1_inbox_tx bench | OK (F-07 info) |
| `697d0945` | B1InboxItem::Inline | OK, F-02 metriques |
| `da472ea3` | IndexedFileCache dedup | OK |
| `65f9f2d6` | fuse_small_chunks | F-08 stabilite (MOYEN) |
| `f0f2e493` | BGE prefix, Arc<Tokenizer>, file-level chunks | F-03 ELEVE (UTF-8 panic), F-04 info, F-09 info, F-11 info |
| `4eef27fa` | COPY BINARY bulk_writer | F-01 ELEVE (token_count missing), F-10 info |
| `bb7399d9` | embedding dedup + double-buffering | F-05 ELEVE (cache non mis a jour), F-06 MOYEN |

## Verdict global

**VALIDE AVEC RESERVES**

3 findings ELEVES qui necessitent correction avant mise en production :

1. **F-01** : COPY BINARY omet `token_count` => regression du tri bucket B1 sous `AXON_BULK_WRITER_ENABLED=true`.
2. **F-03** : Truncation UTF-8 par index de bytes => panic potentiel sur fichiers non-ASCII > 2000 bytes sans symboles.
3. **F-05** : Embedding dedup cache jamais mis a jour apres B3 write => le dedup est inefficace apres le premier passage.

2 findings MOYENS a planifier :

4. **F-02** : Metriques B1 double-comptage sur dedup skip => diagnostics biaises.
5. **F-08** : Stabilite chunk_id degradee par fuse_small_chunks => embeddings GPU inutiles sur micro-symboles.

Les 3 ELEVES sont des corrections locales (< 10 lignes chacune). Aucun n'est bloquant en exploitation immediate (fallbacks existent), mais ils representent des regressions silencieuses de performance et un risque de panic en production.
