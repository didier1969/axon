# Multi-Resolution Retrieval for Axon — DeepSeek V4 Inspired

| | |
|---|---|
| **Date** | 2026-05-09 |
| **Statut** | Proposition (avant log SOLL) |
| **Auteur** | Session conversationnelle operator + Claude |
| **Audience** | LLM/équipe en charge de l'évolution d'Axon MCP |
| **Origine** | DeepSeek V4 paper (1.6T params, 1M context) — intuition operator |

---

## 1. Résumé exécutif

DeepSeek V4 résout un problème **structurellement identique** à celui d'Axon : comment un système de mémoire massive reste rapide, précis et économe quand l'espace à explorer dépasse ce qu'on peut scanner naïvement.

Trois techniques DeepSeek transposables à Axon :

1. **Hybrid attention** (CSA + HCA + sliding window) → **retrieval multi-résolution** combinant SOLL (compressé), IST/pgvector (chunks sélectionnés), edits récents (sliding window) en **une seule réponse layered**.
2. **Lightning Indexer** (scoring rapide pré-attention) → **query router** scorant les pillars/modules grossiers avant l'ANN pgvector.
3. **Anticipatory routing** (snapshots décalés) → **lectures servies depuis un snapshot consistant**, décorrélées de l'ingestion live.

**Gain attendu** :
- Petits contextes (≤50K) : marginal (10-20%).
- Contextes moyens (100-300K) : net (−40% latence, +20% qualité).
- **Grands contextes (≥500K, 1M) : transformatif** (+30-45% qualité, −50% tokens à qualité égale, cache hit ×3-5).

**Recommandation** : implémenter en 3 phases séquentielles. **Phase A est la priorité absolue** — c'est elle qui livre la valeur perçue. Phases B et C sont du levier de scaling et de robustesse, à engager après validation A.

---

## 2. Origine et inspiration

### 2.1 DeepSeek V4 — points clés

DeepSeek V4 atteint la parité avec les top closed models (Opus 4.6 Max, Gemini 3.1 Pro) en **étant 40× plus petit en effectifs et avec moins de compute**. Leur insight central :

> *"Don't treat all past information as equally important. Compress the past and ignore most of it."*

Ils inversent la question : au lieu de **"comment traiter tout ça ?"**, ils demandent **"combien peut-on en ignorer et toujours bien répondre ?"**.

Mécanismes principaux dans le paper :

- **CSA** (Compressed Sparse Attention) : groupe ~4 tokens en un vecteur dense + Lightning Indexer pour sélection sparse.
- **HCA** (Heavily Compressed Attention) : groupe ~128 tokens (paragraphe) en un seul vecteur — vue globale.
- **Sliding window** : ~128 derniers tokens en fidélité totale.
- **MHC** (Manifold Constrained Hyperconnections) : prévient l'explosion de signal à ≥1T params.
- **Anticipatory routing** : snapshots paramétriques décalés pour ignorer le bruit step-by-step.
- **Curriculum training** : 4K → 16K → 64K → 1M context.

### 2.2 L'intuition operator

L'operator a perçu un alignement structurel entre les techniques DeepSeek et Axon, sans pouvoir l'articuler initialement. L'analyse a confirmé le feeling : Axon **possède déjà les couches** (IST, SOLL, embeddings) mais **manque l'orchestration multi-résolution** que DeepSeek formalise.

---

## 3. Mapping vers Axon

### 3.1 Hybrid attention → retrieval multi-résolution

**DeepSeek** : trois vues parallèles du passé, interleavées dans le réseau.

**Axon** : trois couches existantes, **non orchestrées en pipeline unifié** :

| Couche DeepSeek | Équivalent Axon | État aujourd'hui |
|---|---|---|
| HCA (très compressé) | SOLL : concepts, pillars, decisions, requirements | ✅ Existe, ❌ pas fusionné dans `retrieve_context` |
| CSA (chunks + Lightning Indexer) | IST + pgvector ANN sur ChunkEmbedding | ✅ Existe, ❌ pas de pré-scoring hiérarchique |
| Sliding window (fidélité totale) | Edits récents, fichier focus, cwd | ⚠️ Partiel via `axon_pre_flight_check`, pas exposé en retrieval |

**Lacune** : un client LLM doit aujourd'hui faire 3-4 calls MCP séparés (`soll_query_context`, `query`, `inspect`, `retrieve_context`) et reconstruire lui-même le layering. Coût en round-trips, en tokens, en latence.

### 3.2 Lightning Indexer → query router avant ANN

**DeepSeek** : avant l'attention coûteuse, un petit modèle scoring sélectionne les blocs pertinents.

**Axon** : aujourd'hui `retrieve_context` fait un ANN pgvector sur **toute la table ChunkEmbedding** (potentiellement millions de chunks à terme). C'est l'équivalent de "scanner tous les tokens passés".

**Solution** : un pré-scoring rapide à granularité grossière (pillar, module, fichier), réduisant l'espace candidat de l'ANN d'un ou deux ordres de grandeur. Implémentable :

- en première itération comme une **fonction Rust pure** (similarité cosine sur embeddings de pillars existants),
- en seconde itération comme un petit modèle dédié si nécessaire.

### 3.3 Anticipatory routing → snapshots stables

**DeepSeek** : utilise des snapshots paramétriques légèrement antérieurs pendant l'entraînement pour ignorer le bruit step-by-step.

**Axon** : pendant l'indexation, l'état IST/embeddings est bruité (chunks half-written, embeddings en flight). Les requêtes peuvent voir des états partiels sous charge — risque de réponses inconsistantes.

**Solution** : reads servis depuis un **snapshot logique stable** (équivalent MVCC), avancé périodiquement quand l'indexer atteint un état stable. Décorrèle retrieval et ingestion.

---

## 4. Philosophie centrale

> **La mémoire utile n'est pas la plus complète — c'est la plus sélective au bon niveau d'abstraction au bon moment.**

C'est l'inversion DeepSeek appliquée à Axon. Aujourd'hui, Axon livre **des couches** ; demain, Axon livre **une réponse multi-résolution orchestrée**, qui imite la stratégie d'un humain qui révise (sliding window des pages ouvertes + résumés des chapitres + extraits surlignés des sections pertinentes).

---

## 5. Estimations de performance

### 5.1 Impact sur le LLM client

| Métrique | Phase A | Phase B | A+B+C | Confiance |
|---|---|---|---|---|
| Round-trips MCP par tâche (3-4 → 1) | **−70%** | 0% | **−70%** | Haute |
| Latence end-to-end par tour | −30 à −50% | −5 à −15% | **−40 à −60%** | Haute |
| Tokens consommés / réponse utile | −20 à −35% | −5 à −10% | **−25 à −40%** | Moyenne |
| Qualité réponse (réduction hallucinations sur intent) | +10 à +20% | +5% | **+15 à +25%** | Moyenne |
| Latence ANN à ≥1M chunks | 0% | **−80 à −90%** | −80 à −90% | Haute |
| Cache prompt hit rate (structure stable) | +15 à +25% | 0% | **+15 à +25%** | Moyenne |

### 5.2 Avantage en grand contexte

Le vrai levier apparaît à grande échelle. Trois effets connus :

**(a) Lost-in-the-middle dégrade les LLMs de 20-40% sur les contextes ≥100K tokens** quand l'info est noyée. Une réponse layered place l'intent en haut + sliding window en bas — **les deux zones où l'attention LLM est la plus forte**.

| Taille contexte | Système actuel | Avec Phase A | Gain qualité |
|---|---|---|---|
| 50K tokens | baseline | +5 à +10% | marginal |
| 200K tokens | baseline | +15 à +25% | significatif |
| 500K tokens | baseline | +25 à +35% | majeur |
| **1M tokens** | baseline (30-40% précision perdue) | **+30 à +45%** | **transformatif** |

**(b) Économie de contexte à qualité constante** : pour atteindre la même précision, un LLM peut se contenter de **40-60% du contexte actuel** si les bandes sont triées par pertinence. Sur Sonnet à 1M tokens (~3$/query), c'est **1.20-1.80$ économisé par requête**.

**(c) Cache prompt friendly** : la bande SOLL change rarement → préfixe stable → hit rate cache prompt **2-5×** sur les sessions multi-tours. Sur Anthropic, **−90% sur les tokens du préfixe caché**.

### 5.3 Mise en garde

Ces chiffres sont des **estimations heuristiques** basées sur la littérature RAG hiérarchique + lost-in-the-middle (NIH, Liu et al. 2023). Phase A doit être **benchmarkée** sur 5-10 scénarios représentatifs avant d'engager B et C — fourchettes ±10% facilement.

---

## 6. Cahier des charges

Document d'implémentation pour le LLM en charge de la production d'Axon. Spécifications fonctionnelles + critères d'acceptation + métriques de validation.

### 6.1 Phase A — Layered Envelope (PRIORITÉ ABSOLUE)

#### Objectif

Transformer `retrieve_context` (et éventuellement `axon_pre_flight_check`) en une **API multi-résolution** retournant en un seul appel les trois bandes d'information dont a besoin un LLM client.

#### Spécification de l'API

**Input** (extension de l'API actuelle, backward compatible) :

```json
{
  "query": "string (requête sémantique ou symbol)",
  "project_code": "string (auto-resolved si absent)",
  "bands": {
    "intent": { "enabled": true, "max_tokens": 2000 },
    "code": { "enabled": true, "max_tokens": 6000, "k": 12 },
    "recent": { "enabled": true, "max_tokens": 1500, "window": "24h" }
  },
  "format": "layered" | "legacy"
}
```

**Output** (mode `layered`) :

```json
{
  "intent_band": {
    "concepts": [{ "id": "CPT-AXO-XXX", "title": "...", "summary": "..." }],
    "decisions": [{ "id": "DEC-...", "title": "...", "summary": "..." }],
    "requirements": [{ "id": "REQ-...", "title": "...", "status": "..." }],
    "tokens_used": 1840
  },
  "code_band": {
    "chunks": [
      { "file_path": "src/...", "symbol": "...", "code": "...", "score": 0.87 }
    ],
    "tokens_used": 5720
  },
  "recent_band": {
    "git_recent_edits": [{ "file": "...", "ts": "...", "summary": "..." }],
    "current_focus": { "file": "...", "function": "..." },
    "tokens_used": 1320
  },
  "metadata": {
    "snapshot_id": "...",
    "freshness": "fresh" | "stale",
    "retrieval_path": "soll+ann+git",
    "total_tokens": 8880,
    "elapsed_ms": 142
  }
}
```

**Mode `legacy`** : conserve l'API actuelle pour clients non-migrés.

#### Critères d'acceptation

| # | Critère | Mesure |
|---|---|---|
| A1 | Mode `layered` retourne 3 bandes en un appel | Test d'intégration |
| A2 | Backward compat 100% sur mode `legacy` | Suite de tests existante passe sans modif |
| A3 | Budget tokens respecté ±10% par bande | Tests par-bande |
| A4 | Latence p50 ≤ 1.3× actuelle, p95 ≤ 1.5× | Benchmark sur scénarios `quality_bench` |
| A5 | Bande `intent` populée à partir de `soll_query_context` interne (pas de double-call MCP côté LLM) | Trace MCP |
| A6 | Bande `recent` populée via git log + cwd détectés depuis `project_path` | Test offline |
| A7 | Snapshot ID stable pendant la durée d'une requête | Test concurrence |

#### Métriques de validation (gate de passage)

Avant d'autoriser Phase B, valider sur ≥10 scénarios représentatifs :

| Métrique | Cible | Méthode |
|---|---|---|
| Round-trips MCP par tâche | −60% au minimum | Trace agent Claude/Codex sur scénarios refactor + impact analysis |
| Tokens consommés / tâche complète | −20% au minimum | Idem |
| Qualité réponse (jury LLM-as-judge) | +10% au minimum sur tâches "intent + code" | Eval benchmark dédié |
| Latence end-to-end par tour | −25% au minimum | Profiling agent |

#### Implémentation suggérée (macro)

1. **Composer côté brain** : nouvelle fonction `retrieve_context_layered` qui orchestre en interne `soll_query_context`, le pgvector ANN existant, et un nouveau fetch `git log` / cwd inspection.
2. **Budgets par bande** : enforcer `max_tokens` via tokenizer (ttok ou similar).
3. **Backward compat** : router via le paramètre `format` ; default = `legacy` durant 1 release.
4. **Pas de nouveau modèle ML** : tout se fait avec les briques existantes.
5. **Tests** : suite `quality_bench` + nouveaux scénarios layered.

#### Effort estimé

**2-4 semaines pour un dev senior** (spec + impl + tests + bench). Pas de nouveau composant infra. Risque technique faible.

---

### 6.2 Phase B — Lightning Indexer / Query Router

#### Objectif

Réduire le coût de l'ANN pgvector à grande échelle (≥1M chunks) en pré-scorant les blocs grossiers (pillars, modules, fichiers) avant de plonger dans le ANN au niveau chunk.

#### Spécification

**Pipeline** :

1. Query `q` arrive.
2. **Étape 1 (Lightning Indexer)** : score `q` contre les embeddings de **pillars/modules** existants (granularité grossière, déjà calculés en SOLL). Sélectionne le top-N (ex: top-10 pillars).
3. **Étape 2 (ANN ciblé)** : pgvector ANN restreint au sous-ensemble de chunks appartenant aux pillars sélectionnés.
4. Retourne les chunks finaux à la bande `code_band` de Phase A.

**Implémentation V1 (sans nouveau modèle ML)** :

- Cosine similarity pure entre l'embedding de la query et les embeddings de pillars/modules pré-calculés.
- Filtre WHERE sur la requête pgvector : `WHERE module_id IN (...)`.

**Implémentation V2 (si V1 insuffisant)** :

- Petit modèle dédié (cross-encoder ou bi-encoder léger) pour scoring plus fin.
- Pas avant validation V1.

#### Critères d'acceptation

| # | Critère | Mesure |
|---|---|---|
| B1 | Latence ANN à 1M chunks ≤ 30% de baseline | Benchmark synthétique |
| B2 | Recall@10 ≥ 95% du baseline non-routé | Eval recall sur ground truth |
| B3 | Désactivable via flag (fallback ANN plat) | Test config |
| B4 | Couplé proprement à Phase A (transparent côté API) | Test E2E |

#### Métriques de validation

- Latence retrieve_context p95 < 200ms à 1M chunks (vs ~2s actuel projeté).
- Recall@10 préservé.
- Coverage : 100% des chunks ranked accessibles via ≥1 pillar.

#### Effort estimé

**3-6 semaines** (implémentation V1 + benchmark + tuning). À engager **uniquement après gate Phase A validé**.

---

### 6.3 Phase C — Snapshot Stability

#### Objectif

Découpler les reads (retrieval LLM) de l'ingestion live (indexer écrivant des chunks/embeddings). Servir les requêtes depuis un snapshot logique consistant, avancé atomiquement.

#### Spécification

**Mécanisme** :

- Brain maintient un `current_snapshot_id` (entier monotone, persisté).
- Indexer écrit dans une zone "in-flight" + commit atomique → bumping `current_snapshot_id`.
- Tous les reads MCP (`retrieve_context`, `query`, etc.) capturent `snapshot_id` au début de la requête et utilisent les vues filtrées par cet ID.
- Implémentation côté PG : utiliser MVCC natif + un champ `visible_from_snapshot` sur ChunkEmbedding/SymbolNode, ou table de mapping snapshot_id → set de transaction IDs.

**Exposition** :

- `snapshot_id` retourné dans `metadata.snapshot_id` de Phase A.
- Outil `mcp__axon__snapshot_history` (existe déjà ?) à enrichir.

#### Critères d'acceptation

| # | Critère | Mesure |
|---|---|---|
| C1 | Aucune réponse avec état partiel sous charge ingestion | Test stress concurrent indexer + 100 reads/s |
| C2 | Latence de read non dégradée vs Phase A+B | Benchmark |
| C3 | Lag snapshot ≤ 30s sous charge nominale | Métrique runtime |
| C4 | Reproducibilité : 2 reads avec même `snapshot_id` retournent même résultat | Test E2E |

#### Effort estimé

**4-8 semaines** (engineering concurrence côté PG, test stress, monitoring). À engager **après stabilisation Phase A+B en production live**.

---

### 6.4 Hors-scope (pour mémoire)

Les éléments DeepSeek suivants sont **non-applicables** ou **non-prioritaires** pour Axon :

| Technique DeepSeek | Statut Axon |
|---|---|
| MHC (manifold constrained hyperconnections) | N/A — concerne l'entraînement de LLM, pas un système de mémoire |
| Muon optimizer | N/A — concerne l'entraînement |
| TileLang fused kernels | N/A — Axon délègue le ML à ONNX Runtime |
| Curriculum training | Potentiellement applicable au cycle d'embedding (priorité d'indexation par centralité) — backlog futur |
| Compute/comm overlap | Déjà tenté côté async writer (REQ-AXO-193 direction E) — résultats mitigés, pas dans ce périmètre |

---

## 7. Séquencement et gates

```
[ Phase A ] ──── gate A validé ────► [ Phase B ] ──── gate B validé ────► [ Phase C ]
   2-4 sem                              3-6 sem                              4-8 sem
   priorité                             scaling                              robustness
   ROI visible                          coût ANN à scale                     concurrence
```

**Gate A** (avant Phase B) :
- 4 critères de validation 5.x atteints sur ≥10 scénarios.
- Phase A déployée en `live` ≥2 semaines sans régression.
- Decision SOLL **DEC-AXO-XXX-A** approuvée par operator.

**Gate B** (avant Phase C) :
- B1-B4 validés.
- Phase B en `live` ≥2 semaines.
- Charge effective ≥500K chunks observée OU projetée à 6 mois.

**Gate C** : engagement uniquement si bug avéré d'inconsistance lecture sous charge, OU charge live croît au-delà du seuil de risque.

---

## 8. Risques et mitigations

| Risque | Impact | Probabilité | Mitigation |
|---|---|---|---|
| Bande `intent` trop bruyante (concepts non pertinents) | Qualité dégradée | Moyenne | Scoring sémantique de la bande SOLL avant inclusion ; budget tokens strict |
| Round-trip économisés mais tokens augmentent (intent_band ajoute du bruit) | Faux gain | Moyenne | Mesure stricte `tokens_per_useful_answer`, pas seulement tokens absolus |
| Phase B casse recall sur queries "long-tail" | Manqué de chunks pertinents | Moyenne | Flag de désactivation ; fallback ANN plat si recall < seuil |
| Phase C complexifie l'architecture sans bénéfice perçu | Dette technique | Faible | N'engager que si bug observé en production |
| Chiffres %  ne se confirment pas en bench | Perte de crédibilité | Moyenne | Présenter les fourchettes comme heuristiques ; valider avant d'annoncer |

---

## 9. Actions opérateur recommandées

1. **Log SOLL** : transformer ce document en triplet de nœuds canoniques :
   - `CPT-AXO-XXX` — *Multi-resolution retrieval philosophy* (philosophie centrale, section 4)
   - `REQ-AXO-XXX-A` — *Layered envelope for retrieve_context* (Phase A, section 6.1)
   - `REQ-AXO-XXX-B` — *Lightning indexer query router* (Phase B, section 6.2)
   - `REQ-AXO-XXX-C` — *Snapshot consistency for reads* (Phase C, section 6.3)
   - `DEC-AXO-XXX` — *Sequencing gates A → B → C* (section 7)
2. **Bench baseline** : capturer l'état actuel (round-trips, tokens, latence, qualité) sur 10 scénarios représentatifs avant tout dev — sinon impossible de mesurer le delta.
3. **Décision go/no-go Phase A** : à prendre après log SOLL.

---

## 10. Références

- DeepSeek V4 Technical Report (1.6T params, 1M context, hybrid attention).
- DeepSeek V3.2 Technical Report (baseline d'efficacité).
- Liu et al. 2023, *"Lost in the Middle: How Language Models Use Long Contexts"*.
- Anthropic Prompt Caching documentation (cache hit rate économique).
- Axon SOLL : `CPT-AXO-018` (MCP contract hygiene), `CPT-AXO-029` (IST freshness), `MIL-AXO-015` (Postgres migration — fournit la fondation pgvector requise pour Phase B).

---

## Annexe A — Dialogue origine

Document construit lors de la session conversationnelle 2026-05-09 entre l'operator et Claude (Opus 4.7, 1M context). Trois questions opérateur ont structuré l'analyse :

1. *"Pourquoi j'ai ce feeling que DeepSeek pourrait augmenter le potentiel d'Axon ?"* → Mapping section 3.
2. *"Cela augmentera-t-il la valeur perçue d'Axon, et comment l'implémenter ?"* → Sections 5 + 6 (cahier des charges).
3. *"Avantage en grand contexte (100K-1M tokens) ?"* → Section 5.2 (gain transformatif documenté).

Ces trois questions reflètent une trajectoire **intuition → validation → spécification** typique du protocole CPT-AXO-019 (documente). Le document doit être transformé en nœuds SOLL avant d'être considéré comme canonique.
