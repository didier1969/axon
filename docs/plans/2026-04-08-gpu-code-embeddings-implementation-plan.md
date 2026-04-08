# GPU Code Embeddings Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrer Axon d'un pipeline embeddings code `384d` CPU-centrique vers un pipeline configurable, benchmarke et GPU-ready, avec `jinaai/jina-embeddings-v2-base-code` comme cible primaire et `BAAI/bge-base-en-v1.5` comme fallback.

**Architecture:** Le travail se fait en quatre couches: contrat de configuration des embeddings, backend runtime GPU/CPU explicite, migration du stockage dimensionnel, puis qualification par benchmarks qualite/debit. La migration reste gouvernee par TDD et par une bascule par etapes, jamais par remplacement brutal.

**Tech Stack:** Rust, fastembed, ONNX Runtime, DuckDB plugin Axon, tests cargo, benchmarks Axon sur corpus reel.

---

### Task 1: Exposer le contrat de verite du pipeline embeddings actuel

**Files:**
- Create: `src/axon-core/src/tests/embedding_benchmark_tests.rs`
- Modify: `src/axon-core/src/lib.rs`
- Test: `src/axon-core/src/tests/embedding_benchmark_tests.rs`

**Step 1: Write the failing test**

Ecrire un test minimal qui:
- instancie le runtime embeddings actuel
- capture le modele actif, la dimension active, les kinds actifs, les batch sizes et le provider d'execution si disponible
- echoue si ces informations ne sont pas disponibles

**Step 2: Run test to verify it fails**

Run: `cargo test embedding_benchmark_reports_active_model_and_dimension -- --nocapture`
Expected: FAIL car Axon n'expose pas encore proprement ce contrat runtime.

**Step 3: Write minimal implementation**

Ajouter les hooks de lecture necessaires sans changer encore le modele:
- exposition du modele actif
- exposition de la dimension
- exposition des kinds et batch sizes
- exposition du provider d'execution si disponible

Contrainte documentee:
- le debit observe n'est pas mesure ici, car un benchmark fidele necessite le chargement effectif du modele et doit rester dans la Task 7 pour ne pas rendre la TDD locale fragile ou dependante du cache modele

**Step 4: Run test to verify it passes**

Run: `cargo test embedding_benchmark_reports_active_model_and_dimension -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/tests/embedding_benchmark_tests.rs src/axon-core/src/lib.rs
git commit -m "test: expose current embedding runtime contract"
```

### Task 2: Deconfigurer le contrat embeddings

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/config.rs`
- Test: `src/axon-core/src/tests/embedding_config_tests.rs`

**Step 1: Write the failing test**

Ecrire des tests qui echouent tant que:
- la dimension reste figee a `384`
- le modele reste code en dur
- les IDs `sym/chunk/graph` ne derivent pas d'une config canonique

**Step 2: Run test to verify it fails**

Run: `cargo test embedding_config -- --nocapture`
Expected: FAIL

**Step 3: Write minimal implementation**

Introduire une configuration canonique du type:
- `EmbeddingProfile`
- `EmbeddingKindConfig`
- `model_name`
- `model_id`
- `dimension`
- `backend`
- `execution_provider`

Sans encore activer Jina par defaut.

**Step 4: Run test to verify it passes**

Run: `cargo test embedding_config -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/graph_ingestion.rs src/axon-core/src/graph_bootstrap.rs src/axon-core/src/config.rs src/axon-core/src/tests/embedding_config_tests.rs
git commit -m "refactor: make embedding model contract configurable"
```

### Task 3: Ajouter le provider runtime explicite GPU/CPU

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/runtime_profile.rs`
- Modify: `src/axon-core/src/main.rs`
- Test: `src/axon-core/src/tests/embedding_provider_tests.rs`

**Step 1: Write the failing test**

Ecrire des tests qui echouent tant que:
- le runtime ne sait pas annoncer `cuda` vs `cpu`
- aucun fallback explicite n'est visible
- aucun log/etat de provider effectif n'est disponible

**Step 2: Run test to verify it fails**

Run: `cargo test embedding_provider -- --nocapture`
Expected: FAIL

**Step 3: Write minimal implementation**

Brancher `InitOptions` avec provider explicite quand le runtime le permet, sinon fallback CPU.

Le code doit:
- tenter le provider GPU configure
- journaliser le provider retenu
- exposer le resultat a la telemetrie

**Step 4: Run test to verify it passes**

Run: `cargo test embedding_provider -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/runtime_profile.rs src/axon-core/src/main.rs src/axon-core/src/tests/embedding_provider_tests.rs
git commit -m "feat: make embedding execution provider explicit"
```

### Task 4: Migrer le schema pour supprimer l'hypothese `FLOAT[384]`

**Files:**
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Test: `src/axon-core/src/tests/embedding_schema_migration_tests.rs`

**Step 1: Write the failing test**

Ecrire un test de migration qui echoue tant que:
- `Symbol.embedding`
- `ChunkEmbedding.embedding`
- `GraphEmbedding.embedding`
restent implicitement couples a `384`

**Step 2: Run test to verify it fails**

Run: `cargo test embedding_schema_migration -- --nocapture`
Expected: FAIL

**Step 3: Write minimal implementation**

Introduire une migration versionnee permettant:
- de stocker proprement la nouvelle dimension
- de rendre la revectorisation necessaire visible
- de ne pas casser les bases existantes silencieusement

**Step 4: Run test to verify it passes**

Run: `cargo test embedding_schema_migration -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_bootstrap.rs src/axon-core/src/graph_ingestion.rs src/axon-core/src/tests/embedding_schema_migration_tests.rs
git commit -m "feat: migrate embedding storage beyond 384 dimensions"
```

### Task 5: Integrer `jinaai/jina-embeddings-v2-base-code` comme cible primaire

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/config.rs`
- Test: `src/axon-core/src/tests/jina_embedding_profile_tests.rs`

**Step 1: Write the failing test**

Ecrire des tests qui echouent tant que:
- le profil `jina-code-gpu` n'existe pas
- `jina` n'est pas selectionnable proprement
- le fallback `bge-base` n'existe pas

**Step 2: Run test to verify it fails**

Run: `cargo test jina_embedding_profile -- --nocapture`
Expected: FAIL

**Step 3: Write minimal implementation**

Ajouter:
- profil primaire `jinaai/jina-embeddings-v2-base-code`
- profil fallback `BAAI/bge-base-en-v1.5`
- strategie de selection par config/env

**Step 4: Run test to verify it passes**

Run: `cargo test jina_embedding_profile -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/config.rs src/axon-core/src/tests/jina_embedding_profile_tests.rs
git commit -m "feat: add jina code embedding profile with bge fallback"
```

### Task 6: Recaler le pipeline de vectorisation sur le GPU

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Test: `src/axon-core/src/tests/file_vectorization_throughput_tests.rs`

**Step 1: Write the failing test**

Ecrire des tests qui echouent tant que:
- le pipeline reste mono-worker sans calibration batch
- les batch sizes restent rigides
- le backlog `FileVectorizationQueue` n'est pas pilote par un budget runtime

**Step 2: Run test to verify it fails**

Run: `cargo test file_vectorization_throughput -- --nocapture`
Expected: FAIL

**Step 3: Write minimal implementation**

Introduire:
- batch sizes derives de profil runtime
- separation plus claire `symbol/chunk/graph`
- controles de pression pour eviter l'emballement memoire

**Step 4: Run test to verify it passes**

Run: `cargo test file_vectorization_throughput -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/graph_ingestion.rs src/axon-core/src/main_background.rs src/axon-core/src/tests/file_vectorization_throughput_tests.rs
git commit -m "perf: calibrate vectorization pipeline for gpu execution"
```

### Task 7: Ecrire le benchmark comparatif modele/qualite/debit

**Files:**
- Create: `src/axon-core/src/tests/embedding_profile_benchmark_tests.rs`
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/tests/embedding_profile_benchmark_tests.rs`

**Step 1: Write the failing test**

Ecrire un benchmark compare:
- `bge-small` actuel
- `bge-base`
- `jina-v2-base-code`

Mesures requises:
- embeddings/s
- temps moyen par lot
- dimension
- provider
- qualité retrieval sur jeu de requetes Axon

Le test doit echouer tant que ces comparaisons ne sont pas produites.

**Step 2: Run test to verify it fails**

Run: `cargo test embedding_profile_benchmark -- --nocapture`
Expected: FAIL

**Step 3: Write minimal implementation**

Ajouter le harness de benchmark et le reporting standardise.

**Step 4: Run test to verify it passes**

Run: `cargo test embedding_profile_benchmark -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/tests/embedding_profile_benchmark_tests.rs src/axon-core/src/embedder.rs
git commit -m "test: add comparative embedding benchmark harness"
```

### Task 8: Documentation et runbook de revectorisation

**Files:**
- Create: `docs/architecture/2026-04-08-gpu-code-embeddings.md`
- Modify: `docs/getting-started.md`
- Modify: `README.md`

**Step 1: Write the failing test**

Pas de test code ici. Ecrire une checklist documentaire de verification manuelle:
- modele actif documente
- provider actif documente
- migration et revectorisation documentees
- fallback CPU documente

**Step 2: Verify documentation gap**

Run: `rg -n "jina|embedding provider|revector" README.md docs/getting-started.md docs/architecture`
Expected: contenu manquant ou incomplet

**Step 3: Write minimal documentation**

Documenter:
- choix du modele
- fallback
- prerequis GPU
- procedure de migration
- procedure de benchmark

**Step 4: Verify documentation completeness**

Run: `rg -n "jina|embedding provider|revector" README.md docs/getting-started.md docs/architecture/2026-04-08-gpu-code-embeddings.md`
Expected: correspondances presentes

**Step 5: Commit**

```bash
git add README.md docs/getting-started.md docs/architecture/2026-04-08-gpu-code-embeddings.md
git commit -m "docs: document gpu code embeddings architecture and runbook"
```

### Task 9: Validation finale

**Files:**
- Modify: none
- Test: tests cibles + benchmark Axon reel

**Step 1: Run targeted verification**

Run:
- `cargo test embedding_benchmark_reports_active_model_and_dimension -- --nocapture`
- `cargo test embedding_config -- --nocapture`
- `cargo test embedding_provider -- --nocapture`
- `cargo test embedding_schema_migration -- --nocapture`
- `cargo test jina_embedding_profile -- --nocapture`
- `cargo test file_vectorization_throughput -- --nocapture`
- `cargo test embedding_profile_benchmark -- --nocapture`

Expected: PASS

**Step 2: Run real benchmark**

Run: benchmark Axon sur corpus reel via le harness ajoute

Expected:
- provider reel visible
- debit mesure exploitable
- recommendation finale `jina` ou fallback `bge-base`

**Step 3: Commit verification state**

```bash
git add -A
git commit -m "chore: certify gpu code embedding migration"
```
