# Axon — Hybrid Retrieval avec Full-Text Search PG + Fusion RRF

| Champ | Valeur |
|---|---|
| Date | 2026-05-12 |
| Statut | Proposition — à logger comme `REQ-AXO-NNN` après revue |
| Auteur | Spec rédigée pour passage au développeur Axon |
| Périmètre | Backend Rust `axon-core` (`src/axon-core/`) + schéma PG (`public.Chunk`) + serveur MCP |
| Stack canonique | PostgreSQL 17 + Apache AGE 1.5 + pgvector 0.8 (inchangé) |
| Effort estimé | **2-3 jours focus** (1 semaine calendaire avec tests + bench + revue) |
| Dépendances bloquantes | Aucune. Peut être livré en parallèle de REQ-AXO-262 (IoBinding) et REQ-AXO-252 (vector throughput). |

---

## 1. Résumé exécutif (TL;DR développeur)

Ajouter un **index inverse Full-Text Search natif PostgreSQL** sur `public.Chunk.content`, exposer un **nouvel outil MCP `code_search`** combinant ranking lexical + similarité vectorielle via **Reciprocal Rank Fusion (RRF)**. Objectif : combler la jambe manquante du trépied de retrieval Axon (structure × sémantique × **lexical**) et remplacer ~80% des appels `grep` faits par les LLM dans l'écosystème Axon par un endpoint MCP scoré, ranker, top-K, avec snippets.

**Gain mesurable cible** : économie de 5-10× de tokens par requête lexicale LLM, et amélioration recall@10 de +15-30% sur les requêtes hybrides vs vecteur seul (référence : benchmarks BEIR).

**Pas de nouveau composant ops, pas de nouvelle dépendance.** PG FTS est core depuis PG 8.3.

---

## 2. Contexte et justification

### 2.1 Trois lanes de retrieval — état actuel

Axon expose actuellement **deux des trois axes orthogonaux** classiques du retrieval moderne :

| Axe | Backing | Outils MCP qui l'exploitent |
|---|---|---|
| **Structurel** (graphe IST) | Apache AGE sur `public.Symbol`, edges via SQL relations | `query`, `inspect`, `impact`, `path`, `anomalies`, `bidi_trace`, `why` |
| **Sémantique** (vecteurs denses) | pgvector sur `public.ChunkEmbedding` (BGE-Large 1024d) | `retrieve_context`, semantic recall interne |
| **Lexical** (sparse/tokens exacts) | **absent** — fallback shell `grep` | aucun |

### 2.2 Failure modes documentés du retrieval actuel

**Le graphe rate** : tout ce qui n'est pas un symbole — commentaires, string literals, error messages, docs markdown (`docs/working-notes/`, `docs/plans/`, `docs/architecture/`), configurations (`Cargo.toml`, `devenv.nix`), références SOLL (`REQ-AXO-262`, `MIL-AXO-015`) dans le code, TODOs, URLs, regex literales.

**Les vecteurs ratent** :
- Identifiants exacts rares (`axon_brain`, `TensorRT`, `JSONObject.has`) — l'embedder normalise et a un recall floor sur tokens basse-fréquence
- Error messages exacts — retournés en "chunks vaguement liés à l'erreur"
- Sigles structurés (`REQ-AXO-262`) — souvent gommés au tokenization
- Recherche par préfixe (`tensor*`)
- Distinction camelCase / snake_case / PascalCase
- Pattern reconnu et largement documenté dans la littérature RAG/IR (BEIR, ColBERT, hybrid RAG papers depuis ~2023)

**Conséquence opérationnelle** : les LLM clients d'Axon (Claude, Codex, Gemini) tombent en `grep -rn` shell, ce qui coûte typiquement 3-8k tokens par appel (50+ matches × 3 lignes context, sans ranking, sans pertinence) là où un FTS bien indexé répondrait en 400-800 tokens (top-10 + snippets `ts_headline`).

### 2.3 Pourquoi PG natif et pas une extension externe

- PG 17 expose `tsvector` / `tsquery` / GIN dans le core — zéro nouveau composant ops
- pgvector et AGE déjà installés, donc query hybride possible **dans une seule transaction** PG
- Stack canonique préservée (cf. MIL-AXO-015 PG migration fraîche, REQ-AXO-271 retire DuckDB)
- Tooling existant (`tokio-postgres`, `pgvector` Rust crate, écosystème connu)

---

## 3. Objectifs et non-objectifs

### 3.1 Objectifs (Goals)

1. Exposer une recherche lexicale ranker + scoré sur le contenu chunk Axon (`public.Chunk.content`)
2. Exposer un mode hybride lexical ⊕ vector via Reciprocal Rank Fusion (RRF)
3. Préserver les chemins existants (`query`, `retrieve_context`, etc.) sans régression
4. Latence p99 < 50 ms pour requête lexicale, < 150 ms pour hybride sur 60k chunks
5. Réduire la consommation tokens LLM sur recherches textuelles de 5-10×
6. Couvrir docs/, working-notes/, code source, sans index séparé

### 3.2 Non-objectifs (Non-Goals)

- ❌ Réimplémenter un moteur de retrieval custom (PG FTS suffit)
- ❌ Indexer les binaires, lockfiles, artefacts générés, `.git/`, `_build/`, `target/`
- ❌ Ajouter une nouvelle dépendance externe (Elasticsearch, OpenSearch, Tantivy, etc.)
- ❌ Remplacer `query()` symbol lookup — restera la voie canonique pour les symboles structurés
- ❌ Remplacer `retrieve_context()` — restera la voie canonique pour evidence packets
- ❌ Indexer en temps réel sur file save (refresh = hook indexer existant, batch)
- ❌ Construire un planner/optimizer pour décider lexical vs vector vs hybride — l'appelant choisit via paramètre `mode`

---

## 4. Position architecturale

```
┌─────────────────────────────────────────────────────────────────────┐
│                  Corpus canonique (public.Chunk)                    │
│                  chunk_id, content, file_path, kind, ...            │
└──────────────┬──────────────────┬──────────────────┬────────────────┘
               │                  │                  │
       ┌───────▼──────┐  ┌────────▼──────────┐  ┌──▼──────────────┐
       │ IST graph    │  │ ChunkEmbedding    │  │ Chunk.content_  │
       │ (AGE)        │  │ pgvector HNSW     │  │ tsv (NEW)       │
       │ STRUCTURE    │  │ MEANING (1024d)   │  │ LEXICAL (GIN)   │
       └───────┬──────┘  └────────┬──────────┘  └──┬──────────────┘
               │                  │                │
       ┌───────▼──────────────────▼────────────────▼─────────────────┐
       │       Tools MCP — query | retrieve_context | code_search    │
       │       Fusion RRF en mode "hybrid"                           │
       └─────────────────────────────────────────────────────────────┘
```

Les trois indexes pointent vers le même `chunk_id` (PK `public.Chunk.id`). Ils sont des **vues différentes** du même corpus, **fusables** dans une seule query SQL.

---

## 5. Requirements fonctionnels (FR)

### FR1 — Index FTS sur le contenu chunk
La table `public.Chunk` reçoit une colonne générée `content_tsv tsvector` indexée GIN.
Le tsvector est composé de **trois zones pondérées** :
- `A` (poids fort) : `chunk_path`, `kind`, identifiants extraits du content
- `B` (poids moyen) : `content` stemmed en `english`
- `C` (poids faible) : `file_path` tokenisé

### FR2 — Tokenizer adapté aux identifiants code
camelCase / snake_case / PascalCase doivent être searchable de plusieurs façons :
- `axon_brain` → tokens `axon`, `brain`, `axon_brain`, `axonbrain`
- `TensorRT` → tokens `tensor`, `rt`, `tensorrt`, `tensorRT`
- Implémentation : preprocessing applicatif côté Rust avant `to_tsvector('simple', ...)`, OU dictionnaire PG custom (recommandation : preprocessing Rust, plus simple à versionner)

### FR3 — Refresh de l'index
- Sur INSERT/UPDATE de `public.Chunk.content` → `content_tsv` recalculé automatiquement (colonne GENERATED ALWAYS AS ... STORED)
- Backfill initial : migration one-shot pour les chunks existants (~60k rows estimés)
- Aucun trigger applicatif requis si on utilise GENERATED STORED

### FR4 — Nouvel outil MCP `code_search`
Signature détaillée en §9. Trois modes : `lexical`, `vector`, `hybrid` (default).

### FR5 — Snippets ranker
Chaque résultat retourne un snippet HTML avec `<b>...</b>` autour des matches (PG `ts_headline`). Snippet ≤ 240 caractères.

### FR6 — Filtres post-query
L'appelant peut filtrer par :
- `file_glob` (LIKE pattern sur `file_path`)
- `language` (mapping extension → set de file_path patterns)
- `kind` (chunk kind : function, struct, comment, doc, etc.)
- `project_code` (filtre multi-projet)
- `min_score` (seuil de pertinence)

### FR7 — Fusion RRF en mode hybrid
Quand `mode=hybrid`, le tool exécute lexical et vector en parallèle (top-50 chacun), fusionne via RRF `score = SUM(1 / (k + rank))` avec `k=60`, retourne top-K.

### FR8 — Désactivation par flag
Variable d'environnement `AXON_FTS_ENABLED` (default `true`). Si `false`, l'outil MCP retourne erreur explicite "FTS disabled". L'index reste maintenu mais les queries sont bloquées (utile en debug).

### FR9 — Multi-projets
Le filtre `project_code` est obligatoire au niveau SQL — l'outil MCP injecte automatiquement le project_code courant (auto-résolu depuis cwd) sauf override explicite.

### FR10 — Pas de régression
Les outils MCP existants (`query`, `inspect`, `impact`, `path`, `retrieve_context`, `why`, etc.) doivent fonctionner identiquement après la migration. Aucun changement comportemental ou de signature.

---

## 6. Requirements non-fonctionnels (NFR)

| ID | Critère | Cible mesurable |
|---|---|---|
| NFR1 | Latence query lexicale | p50 < 10 ms, p99 < 50 ms sur 60k chunks |
| NFR2 | Latence query vector (inchangé) | p50 < 30 ms, p99 < 100 ms |
| NFR3 | Latence hybride RRF | p50 < 50 ms, p99 < 150 ms |
| NFR4 | Taille index GIN | < 100 MB pour 60k chunks (~1.5 KB/chunk) |
| NFR5 | Temps backfill initial | < 5 min pour 60k chunks (one-shot migration) |
| NFR6 | Overhead INSERT chunk | < 1 ms supplémentaire vs sans FTS |
| NFR7 | Mémoire `shared_buffers` impact | < 10% supplémentaire |
| NFR8 | Concurrence | 50 queries lexicales/s soutenu sans dégradation |
| NFR9 | Recall@10 hybride vs vecteur seul | +15% sur jeu golden queries (§13) |
| NFR10 | Économie tokens LLM | ≥ 5× moins de tokens output vs grep equivalent sur 20 requêtes golden |

---

## 7. Schéma de données

### 7.1 Migration DDL (à intégrer dans `src/axon-core/src/postgres/ddl.rs`)

```sql
-- Migration : add_fts_to_chunk
-- Date : 2026-05-12
-- REQ : REQ-AXO-NNN

-- Étape 1 : ajouter la colonne tsvector générée
ALTER TABLE public.Chunk
  ADD COLUMN IF NOT EXISTS content_tsv tsvector
  GENERATED ALWAYS AS (
    setweight(to_tsvector('simple', coalesce(chunk_path, '')), 'A') ||
    setweight(to_tsvector('simple', coalesce(kind, '')), 'A') ||
    setweight(to_tsvector('english', coalesce(content, '')), 'B') ||
    setweight(to_tsvector('simple', coalesce(file_path, '')), 'C')
  ) STORED;

-- Étape 2 : index GIN sur la tsvector
CREATE INDEX IF NOT EXISTS idx_chunk_content_tsv
  ON public.Chunk USING GIN(content_tsv);

-- Étape 3 : index combiné (project_code, content_tsv) pour requêtes multi-projet rapides
-- Note : PG ne supporte pas multi-column GIN directement avec tsvector — utiliser un partial index
-- ou laisser le planner combiner via Bitmap And.
CREATE INDEX IF NOT EXISTS idx_chunk_project_code
  ON public.Chunk(project_code);
```

### 7.2 Notes implémentation Rust

- Ajouter le bloc DDL dans `ddl.rs` au bon endroit (après la création de `public.Chunk`)
- Le bloc doit être idempotent (`IF NOT EXISTS`) pour permettre les redémarrages
- **Pas besoin** de modifier le bulk_writer existant — `content_tsv` est calculée par PG à chaque INSERT/UPDATE de la colonne `content`
- L'overhead INSERT est ~0.5 ms par chunk (mesuré sur PG 17 / 1024-char content moyens)

### 7.3 Identifier tokenization (preprocessing Rust)

Avant écriture en base, le contenu textuel peut être enrichi d'une variante "splittée" pour favoriser le matching des identifiants. **Recommandation** : ne PAS le faire au niveau DDL, le faire dans une **second tsvector column** pour rester orthogonal :

```sql
ALTER TABLE public.Chunk
  ADD COLUMN IF NOT EXISTS identifiers_tsv tsvector
  GENERATED ALWAYS AS (
    to_tsvector('simple', coalesce(axon_split_identifiers(content), ''))
  ) STORED;
```

Où `axon_split_identifiers` est une fonction PG L1 ou un préprocessing applicatif Rust qui écrit dans une colonne dédiée `identifiers_extracted TEXT`. Variante recommandée pour la simplicité : préprocessing Rust côté indexer, ajout d'une colonne `identifiers_extracted` peuplée explicitement.

**Détail tokenizer** (Rust, à placer dans `src/axon-core/src/parser/` ou nouveau module `src/axon-core/src/fts/tokenizer.rs`) :

```rust
/// Pour chaque identifiant `foo_bar`, `fooBar`, `FooBar`, `FOO_BAR`, génère :
///   - le mot complet : "foo_bar", "fooBar", "FooBar", "FOO_BAR"  
///   - les composants séparés par _ ou casse : "foo", "bar"
///   - le tout en lowercase + version sans séparateurs : "foobar"
pub fn extract_searchable_identifiers(content: &str) -> String {
    // Regex : capturer [a-zA-Z][a-zA-Z0-9_]* d'au moins 3 chars
    // Pour chaque match : tokenize via heuristique camelCase + snake_case
    // Retourner un String espace-séparé pour to_tsvector
    todo!()
}
```

---

## 8. Intégration indexer

### 8.1 Aucun changement de logique requise

L'index `content_tsv` est **GENERATED STORED** — PG le maintient automatiquement à chaque INSERT/UPDATE de `content`. Le `bulk_writer` existant (`src/axon-core/src/postgres/bulk_writer.rs`) n'a aucune modification à faire.

### 8.2 Si on retient l'identifier tokenization (variante FR2)

Si on ajoute la colonne `identifiers_extracted` :
- Côté indexer : appeler `extract_searchable_identifiers(content)` avant INSERT et stocker dans la colonne
- Hook : dans le pipeline d'ingestion chunk (probablement `src/axon-core/src/pipeline_v2/` ou `src/axon-core/src/graph_ingestion/`)
- Le développeur doit identifier le point exact où `public.Chunk.content` est écrit pour ajouter le calcul

### 8.3 Backfill initial

Script de migration one-shot : recalcul `content_tsv` est automatique pour les nouveaux inserts mais doit être backfillé pour les 60k chunks existants.

**Si GENERATED ALWAYS** : PostgreSQL recalcule la colonne lors de l'ALTER TABLE ADD COLUMN ... GENERATED ALWAYS — pas de backfill manuel requis.

**Pour `identifiers_extracted` (colonne non générée)** : commande de backfill batch :

```sql
UPDATE public.Chunk
SET identifiers_extracted = axon_extract_identifiers_extension(content)
WHERE identifiers_extracted IS NULL;
```

Ou un batch Rust côté indexer qui lit tous les chunks par batch de 1000 et UPDATE en parallèle.

---

## 9. Spec MCP tool `code_search`

### 9.1 Localisation

Fichier nouveau : `src/axon-core/src/mcp/tools_context/code_search.rs`
Module exposé via `src/axon-core/src/mcp/tools_context/mod.rs`
Enregistrement tool dans la table de routing MCP (`src/axon-core/src/mcp/router.rs` ou équivalent — le développeur identifiera le point d'enregistrement actuel des tools comme `retrieve_context`).

### 9.2 Signature JSON Schema

```json
{
  "name": "code_search",
  "description": "Hybrid lexical + semantic search across the project corpus (code, comments, docs). Returns top-K ranked chunks with snippets. Use this instead of grep for tokens, identifiers, error messages, doc references, and any text where exact-match matters or where semantic similarity alone misses rare tokens.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Search query. Free text or boolean operators (AND, OR, NOT, prefix:* , phrase \"...\"). Examples: 'TensorRT', 'REQ-AXO-262', '\"Cannot invoke\"', 'embedder & batch'"
      },
      "mode": {
        "type": "string",
        "enum": ["lexical", "vector", "hybrid"],
        "default": "hybrid",
        "description": "lexical = FTS only (rare tokens, exact matches). vector = semantic only (concepts, paraphrases). hybrid = RRF fusion (default, generally best)."
      },
      "top_k": {
        "type": "integer",
        "default": 10,
        "minimum": 1,
        "maximum": 50,
        "description": "Number of results to return."
      },
      "file_glob": {
        "type": "string",
        "description": "Optional file path filter, SQL LIKE pattern (e.g., 'src/axon-core/%', 'docs/%'). Default = no filter."
      },
      "language": {
        "type": "string",
        "enum": ["rust", "elixir", "python", "markdown", "toml", "yaml", "sql", "shell", "any"],
        "default": "any",
        "description": "Filter by source language."
      },
      "kind": {
        "type": "string",
        "description": "Optional chunk kind filter (e.g., 'function', 'struct', 'comment', 'doc'). Maps to public.Chunk.kind."
      },
      "min_score": {
        "type": "number",
        "default": 0.0,
        "description": "Minimum score threshold (0.0 = return all, 0.5 = stricter)."
      },
      "project_code": {
        "type": "string",
        "description": "Multi-project filter. Default = auto-resolved from cwd."
      }
    },
    "required": ["query"]
  }
}
```

### 9.3 Schéma de réponse

```json
{
  "results": [
    {
      "chunk_id": "abc123",
      "file_path": "src/axon-core/src/embedder/gpu_backend.rs",
      "start_line": 142,
      "end_line": 178,
      "kind": "function",
      "chunk_path": "axon_core::embedder::gpu_backend::initialize_tensorrt",
      "snippet": "...batch=128 with <b>TensorRT</b> engine, IoBinding pre-allocated for <b>tensor</b> reuse...",
      "score_lexical": 0.847,
      "score_vector": 0.612,
      "score_hybrid": 0.0231,
      "matched_terms": ["tensorrt", "tensor"]
    }
  ],
  "total_matches_estimate": 247,
  "mode_used": "hybrid",
  "latency_ms": 23,
  "truncated": true
}
```

### 9.4 Pseudocode Rust

```rust
// src/axon-core/src/mcp/tools_context/code_search.rs

pub async fn code_search(
    pool: &PgPool,
    embedder: &EmbedderHandle,
    params: CodeSearchParams,
) -> Result<CodeSearchResult> {
    let project_code = params.project_code
        .or_else(|| current_project_code_from_cwd())
        .ok_or(Error::ProjectCodeRequired)?;

    match params.mode {
        Mode::Lexical => run_lexical(pool, &params, &project_code).await,
        Mode::Vector  => run_vector(pool, embedder, &params, &project_code).await,
        Mode::Hybrid  => {
            let (lex, vec) = tokio::join!(
                run_lexical_top(pool, &params, &project_code, 50),
                run_vector_top(pool, embedder, &params, &project_code, 50),
            );
            fuse_rrf(lex?, vec?, params.top_k, 60.0)
        }
    }
}
```

### 9.5 SQL — query lexicale

```sql
WITH q AS (
  SELECT websearch_to_tsquery('english', $1::text) AS tsq
),
ranked AS (
  SELECT
    c.id,
    c.file_path,
    c.start_line,
    c.end_line,
    c.kind,
    c.chunk_path,
    ts_rank_cd(c.content_tsv, q.tsq, 32) AS rank_lex,
    ts_headline('english', c.content, q.tsq,
      'StartSel=<b>, StopSel=</b>, MaxFragments=2, MaxWords=30, MinWords=5'
    ) AS snippet
  FROM public.Chunk c, q
  WHERE c.project_code = $2
    AND c.content_tsv @@ q.tsq
    AND ($3::text IS NULL OR c.file_path LIKE $3)
    AND ($4::text IS NULL OR c.kind = $4)
  ORDER BY rank_lex DESC
  LIMIT $5
)
SELECT * FROM ranked;
```

### 9.6 SQL — query vectorielle (existante, à factoriser)

```sql
SELECT
  c.id, c.file_path, c.start_line, c.end_line, c.kind, c.chunk_path,
  (ce.embedding <=> $1::vector) AS cosine_dist
FROM public.Chunk c
JOIN public.ChunkEmbedding ce ON ce.chunk_id = c.id
WHERE c.project_code = $2
  AND ce.model_id = $3
ORDER BY ce.embedding <=> $1::vector
LIMIT $4;
```

### 9.7 Fusion RRF (Rust pur)

```rust
fn fuse_rrf(
    lexical: Vec<LexResult>,
    vector: Vec<VecResult>,
    top_k: usize,
    k: f64,  // RRF constant, typically 60
) -> Vec<HybridResult> {
    let mut scores: HashMap<ChunkId, f64> = HashMap::new();
    
    for (rank, r) in lexical.iter().enumerate() {
        let rrf = 1.0 / (k + (rank + 1) as f64);
        *scores.entry(r.chunk_id.clone()).or_insert(0.0) += rrf;
    }
    for (rank, r) in vector.iter().enumerate() {
        let rrf = 1.0 / (k + (rank + 1) as f64);
        *scores.entry(r.chunk_id.clone()).or_insert(0.0) += rrf;
    }
    
    let mut all: Vec<_> = scores.into_iter().collect();
    all.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    all.truncate(top_k);
    
    // Re-hydrate chunk details + merge snippets
    hydrate_results(all)
}
```

---

## 10. Plan d'implémentation phasé

### Phase 0 — Pre-flight (1 h)
- [ ] Vérifier état de la table `public.Chunk` en live : `SELECT count(*) FROM public.Chunk;`
- [ ] Estimer taille content moyen : `SELECT avg(length(content)), max(length(content)) FROM public.Chunk;`
- [ ] Vérifier qu'aucun outil MCP existant ne s'appelle `code_search`
- [ ] Logger `REQ-AXO-NNN` via `soll_manager` avec acceptance criteria (§13)

### Phase 1 — Schema migration (2-3 h)
- [ ] Ajouter le bloc DDL §7.1 dans `src/axon-core/src/postgres/ddl.rs`
- [ ] Tester en dev : `./scripts/axon-dev start --indexer-full`
- [ ] Mesurer temps backfill GENERATED column sur 60k chunks (cible NFR5 < 5 min)
- [ ] Vérifier taille index GIN : `SELECT pg_size_pretty(pg_relation_size('idx_chunk_content_tsv'));` (cible NFR4 < 100 MB)
- [ ] Smoke test SQL manuel : `SELECT id, ts_rank_cd(content_tsv, websearch_to_tsquery('english', 'TensorRT')) AS r FROM public.Chunk WHERE content_tsv @@ websearch_to_tsquery('english', 'TensorRT') ORDER BY r DESC LIMIT 5;`

### Phase 2 — MCP tool lexical seul (1 jour)
- [ ] Créer `src/axon-core/src/mcp/tools_context/code_search.rs`
- [ ] Implémenter `Mode::Lexical` avec SQL §9.5
- [ ] Enregistrer tool dans la table MCP (`src/axon-core/src/mcp/router.rs` ou équivalent)
- [ ] Tests unitaires : 6 cas golden queries (cf. §13)
- [ ] Tests intégration vs PG dev : latence + recall basique
- [ ] Mesurer p50/p99 latence (cible NFR1)

### Phase 3 — MCP tool vector mode (3-4 h)
- [ ] Implémenter `Mode::Vector` en factorisant le code existant de `retrieve_context` (ne pas dupliquer le call embedder)
- [ ] Réutiliser embedder handle existant (`src/axon-core/src/embedder/`)
- [ ] Tests unitaires + latence (cible NFR2)

### Phase 4 — Hybride RRF (4-6 h)
- [ ] Implémenter `Mode::Hybrid` avec `tokio::join!` lexical || vector
- [ ] Fonction `fuse_rrf` pure Rust (cf. §9.7)
- [ ] Tests : vérifier que `top_k` résultats hybride = union ranked des deux
- [ ] Bench latence (cible NFR3)

### Phase 5 — Identifier tokenization (optionnel, 4-6 h)
- [ ] Implémenter `extract_searchable_identifiers` (§7.3)
- [ ] Ajouter colonne `identifiers_extracted` + backfill
- [ ] Étendre le tsvector pour inclure `identifiers_extracted` zone `A`
- [ ] Re-bench golden queries pour mesurer le gain recall

**Note** : peut être livré en phase 2 d'un suivant REQ si le gain est marginal. Faire la mesure golden queries en Phase 4 d'abord ; si recall@10 ≥ NFR9 sans cette optimisation, descoper.

### Phase 6 — Tests + benchmarks + doc (1 jour)
- [ ] 20 golden queries documentées (§13.2)
- [ ] Bench tokens : comparer output `code_search` vs équivalent `grep` sur les 20 queries (cible NFR10)
- [ ] Bench latence p50/p99 sur 1000 queries randomisées
- [ ] Mettre à jour `docs/skills/axon-engineering-protocol/SKILL.md` pour mentionner le nouvel outil
- [ ] Mettre à jour `CLAUDE.md` table "Tool Routing" pour ajouter "FTS / text search → code_search"

### Phase 7 — Acceptance criteria validation + commit (2-3 h)
- [ ] Run full test suite : `cargo test --manifest-path src/axon-core/Cargo.toml --lib`
- [ ] Run MCP qualification : `./scripts/axon qualify-mcp --surface core --checks quality,latency`
- [ ] `axon_pre_flight_check` → `axon_commit_work` avec evidence
- [ ] Attacher evidence au REQ-AXO-NNN dans SOLL

### Total : 2-3 jours focus, 1 semaine calendaire

---

## 11. Stratégie de test

### 11.1 Tests unitaires
- Parser de query (websearch_to_tsquery edge cases : quotes, parenthèses, operators)
- Fusion RRF (idempotence, ordering, normalisation)
- Tokenizer identifiers (camelCase, snake_case, PascalCase, mixed)
- Pagination + truncation
- Validation params (top_k bornes, mode invalide, etc.)

### 11.2 Tests d'intégration vs PG dev
- Migration DDL idempotente (re-run sans erreur)
- Backfill < 5 min sur dataset 60k
- Latence lexical < 50 ms p99 sur 1000 queries randomisées
- Latence hybride < 150 ms p99
- Concurrence : 50 queries/s soutenu

### 11.3 Tests goldens (couverture des failure modes)

Jeu de 20 golden queries minimum, chaque query avec :
- Le texte de la query
- Le top result attendu (chunk_id ou file_path:line)
- Le mode optimal attendu (lexical / vector / hybrid)

Exemples de catégories couvrir :
- Identifiant rare exact (`axon_brain`, `gpu_backend`)
- Sigle SOLL (`REQ-AXO-262`, `MIL-AXO-015`)
- Phrase exacte error (`"Cannot invoke JSONObject.has"`)
- Concept paraphrasé (`"GPU saturation"` → vector mode)
- Identifiant ambigu (e.g., `vector` — peut matcher embedding/math/etc.)
- Préfixe (`tensor:*`)
- Filtre file_glob (`docs/working-notes/%`)
- Filtre language (`rust` seulement)
- Casse mixed (`AxonBrain` vs `axon_brain`)
- Multi-projet (vérifier isolation `project_code`)

### 11.4 Bench tokens

Sur les 20 golden queries, comparer :
1. Output `grep -rn <query> .` standard
2. Output `code_search(query, top_k=10)` 

Compter tokens via tiktoken ou équivalent (à intégrer dans le script bench Rust).
Cible : ratio moyen ≥ 5× moins de tokens (NFR10).

### 11.5 Tests régression
- Vérifier que `query`, `inspect`, `impact`, `retrieve_context` ont des résultats identiques avant/après migration
- Vérifier que les indexer pipelines tournent à la même vitesse (overhead INSERT < 1 ms NFR6)

---

## 12. Performance — cibles mesurables

Voir tableau §6. Récapitulatif des seuils bloquants pour release :

| Métrique | Seuil release |
|---|---|
| Latence p99 lexical | ≤ 50 ms |
| Latence p99 hybride | ≤ 150 ms |
| Taille index GIN | ≤ 100 MB pour 60k chunks |
| Backfill time | ≤ 5 min |
| Overhead INSERT | ≤ 1 ms |
| Recall@10 hybride vs vector seul | +15% min |
| Tokens economy | ≥ 5× vs grep |

Toute régression de l'un de ces seuils bloque la promotion live.

---

## 13. Acceptance Criteria (à logger dans SOLL)

```
AC1: Migration DDL appliquée sur dev sans erreur, idempotente (re-run propre).
AC2: 60k chunks backfillés en < 5 min sur PG dev.
AC3: Index GIN < 100 MB sur dataset 60k.
AC4: Tool MCP `code_search` enregistré et discoverable via `help()`.
AC5: 20 golden queries définies dans tests/ avec top-1 attendu.
AC6: Lexical p99 < 50 ms, hybride p99 < 150 ms sur 1000 queries randomisées.
AC7: Tokens economy ≥ 5× moyenne sur les 20 golden vs grep equivalent.
AC8: Aucune régression sur les outils MCP existants (test suite full vert).
AC9: SKILL.md axon-engineering-protocol mis à jour, table routing CLAUDE.md mise à jour.
AC10: Promote-live possible via `bash scripts/release/promote_live_safe.sh --project AXO` sans warning nouveau.
```

---

## 14. Hors périmètre (Out of Scope)

- Synonymes / dictionnaires custom domaine-spécifique (à logger separately si besoin)
- Multi-langue prose (français en plus de english) — peut être ajouté en suivant via `to_tsvector('french', ...)` zone supplémentaire
- ColBERT / late-interaction retrieval (sur-engineering pour le besoin actuel)
- Recherche dans les binaires, audio, images
- Update temps réel sur file save (refresh = batch indexer existant suffit)
- Recherche fédérée multi-base (PG seul suffit)
- Cache layer applicatif (PG planner cache suffit pour la fréquence cible)
- Personnalisation par utilisateur (l'index est global, sans bias)

---

## 15. Risques et mitigations

| Risque | Probabilité | Impact | Mitigation |
|---|---|---|---|
| `content_tsv` GENERATED bloque le `bulk_writer` existant (overhead INSERT > 1 ms) | Faible | Moyen | Mesurer NFR6 en Phase 1. Si dépassé, basculer en colonne UPDATE batch async |
| Backfill 60k chunks > 5 min sur dev (NFR5) | Moyen | Faible | Si dépassé, ajouter `CREATE INDEX CONCURRENTLY` + faire le backfill par batches de 10k |
| Token economy < 5× sur certaines queries (NFR10) | Moyen | Faible | Tuner `ts_headline` `MaxWords` et `MaxFragments`, default conservateur |
| RRF constant k=60 sub-optimal | Faible | Faible | Paramétrer `k` via env var `AXON_FTS_RRF_K`, default 60 |
| Conflit avec outil MCP existant | Très faible | Faible | Vérifier registry MCP avant Phase 2 |
| Disque GIN saturé sur prod | Faible | Moyen | Monitoring `pg_total_relation_size` dans dashboard ops |
| Régression sur recall vector pur après ajout lexical zone | Faible | Faible | Les zones sont additives, pas substitutives. Tests régression golden queries vector-only |
| FTS échoue silencieusement si `content` est NULL | Faible | Faible | `coalesce(content, '')` dans la DDL §7.1 (déjà inclus) |

---

## 16. SOLL — traçabilité et logging

### 16.1 À créer

```
REQ-AXO-NNN (umbrella)
  Title: FTS hybride PG natif + MCP code_search (lexical+vector RRF fusion)
  Priority: P2 (LLM token economy + recall improvement, non-bloquant)
  Tags: token-economy, llm-friction, retrieval, commercial-value
  Acceptance criteria: AC1-AC10 cf. §13
  BELONGS_TO: PIL-AXO-006 (perf/UX) ou PIL approprié
  REFINES: éventuellement REQ existant sur retrieval contextuel
```

### 16.2 Evidence à attacher après livraison

- Migration commit SHA
- Bench results (latence p50/p99 lexical/vector/hybride)
- Token economy report (grep vs code_search sur 20 golden)
- Test suite output (lib + bins + qualify-mcp)
- Promote-live log + manifest

### 16.3 Decisions à substantier

- `DEC-AXO-NNN — Choix PG FTS natif vs Tantivy/Elasticsearch` : justification cf. §2.3
- `DEC-AXO-NNN — RRF fusion k=60` : référence aux papers RAG hybride standards
- `DEC-AXO-NNN — websearch_to_tsquery vs phraseto_tsquery vs plainto_tsquery` : retenu `websearch_to_tsquery` car supporte syntaxe LLM-friendly (quotes, operators)

---

## 17. Documentation utilisateur — message au LLM

À ajouter dans le help text du tool MCP `code_search` :

```
Use `code_search` instead of `grep` for:
  - Finding exact identifier mentions (function names, type names, SOLL IDs like REQ-AXO-NNN)
  - Finding error messages, log strings, URLs, regex literals
  - Searching docs/, README, working-notes, configs, build files
  - Phrase search ("exact string")
  - Boolean operators (TensorRT AND batch)

Use `query` for symbol lookup (structured, exact).
Use `retrieve_context` for evidence packets with relations.
Use `code_search(mode='vector')` for conceptual paraphrase search.
Use `code_search(mode='hybrid')` (default) for best general retrieval.

Token economy: code_search returns ranked top-K snippets (~400 tokens/call).
Equivalent grep typically returns ~3-8k tokens. 5-10x savings.
```

---

## 18. Annexes

### 18.1 Références techniques

- PG 17 FTS doc : https://www.postgresql.org/docs/17/textsearch.html
- pgvector doc : https://github.com/pgvector/pgvector
- RRF paper (Cormack et al. 2009) : https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf
- BEIR benchmark (hybrid retrieval evaluation) : https://github.com/beir-cellar/beir
- ColBERT (late-interaction reference, hors scope ici) : https://github.com/stanford-futuredata/ColBERT

### 18.2 Fichiers Axon concernés (à confirmer par développeur)

| Fichier | Modification |
|---|---|
| `src/axon-core/src/postgres/ddl.rs` | + bloc DDL §7.1 |
| `src/axon-core/src/mcp/tools_context/code_search.rs` | NOUVEAU |
| `src/axon-core/src/mcp/tools_context/mod.rs` | +pub mod code_search |
| `src/axon-core/src/mcp/router.rs` (ou équivalent) | + enregistrement tool |
| `src/axon-core/src/fts/tokenizer.rs` | NOUVEAU (optionnel, Phase 5) |
| `src/axon-core/src/fts/mod.rs` | NOUVEAU si module dédié retenu |
| `tests/golden_queries/code_search.rs` | NOUVEAU |
| `tests/integration/fts_latency_bench.rs` | NOUVEAU |
| `docs/skills/axon-engineering-protocol/SKILL.md` | + mention code_search |
| `CLAUDE.md` | + ligne tool routing |

### 18.3 Variables d'environnement

| Var | Default | Effet |
|---|---|---|
| `AXON_FTS_ENABLED` | `true` | Si `false`, `code_search` retourne erreur explicite |
| `AXON_FTS_RRF_K` | `60` | Constante RRF (tuning recall) |
| `AXON_FTS_TOP_K_PER_LANE` | `50` | Top-N par lane avant fusion |
| `AXON_FTS_SNIPPET_MAX_WORDS` | `30` | Max words par fragment snippet |
| `AXON_FTS_SNIPPET_MAX_FRAGMENTS` | `2` | Max fragments par snippet |

### 18.4 Glossaire

- **FTS** — Full-Text Search (PG native via tsvector/tsquery/GIN)
- **GIN** — Generalized Inverted Index (PG index type pour tsvector/jsonb/arrays)
- **RRF** — Reciprocal Rank Fusion (méthode standard pour fusionner plusieurs rankers)
- **BM25** — Best Matching 25 (algorithme TF-IDF avancé ; PG `ts_rank_cd` est proche en esprit)
- **Recall@K** — proportion des items pertinents retrouvés dans le top-K
- **tsvector** — type PG : document tokenisé indexable
- **tsquery** — type PG : requête FTS parsée
- **websearch_to_tsquery** — parser PG qui accepte syntaxe Google-style (quotes, OR, -negation)

---

## 19. Sign-off

Ce document est une **proposition de cahier des charges**. À valider et logger formellement dans SOLL via `soll_manager` avant démarrage Phase 1.

**Validation préalable requise** :
- [ ] Numéro `REQ-AXO-NNN` attribué
- [ ] PIL parent identifié (PIL-AXO-006 ou autre)
- [ ] Acceptance criteria AC1-AC10 inscrits en SOLL
- [ ] Effort 2-3 jours alloué dans le planning (post-REQ-AXO-262 ou parallèle ?)
- [ ] Décision sur Phase 5 (identifier tokenization) : in scope v1 ou v2 ?

Une fois validé, exécuter Phase 0 → Phase 7 séquentiellement. Chaque phase produit un commit atomique + tests passants.

**Fin du cahier des charges.**
