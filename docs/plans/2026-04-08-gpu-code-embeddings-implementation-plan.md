# GPU Code Embeddings Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrer Axon d'un pipeline embeddings code `384d` CPU-centrique vers un pipeline configurable, benchmarke et GPU-ready, avec `jinaai/jina-embeddings-v2-base-code` comme cible primaire et `BAAI/bge-base-en-v1.5` comme fallback.

**Architecture:** Le travail se fait en quatre couches: contrat de configuration des embeddings, backend runtime GPU/CPU explicite, migration du stockage dimensionnel, puis qualification par benchmarks qualite/debit. La migration reste gouvernee par TDD et par une bascule par etapes, jamais par remplacement brutal.

**Tech Stack:** Rust, fastembed, ONNX Runtime, DuckDB plugin Axon, tests cargo, benchmarks Axon sur corpus reel.

**Etat de la cible de performance au `2026-04-08`:**
- objectif strategique a prouver: `300_000 embeddings/heure` soit `83.33 embeddings/s`
- cible materielle: GPU `8 Go VRAM`
- surfaces visees: `file`, `type`, `procedure`
- verite actuelle:
  - le harness benchmark reel existe et est versionne
  - la cible est mesuree comme non atteinte, y compris sur des runs `cuda` avec preuve externe `nvidia-smi`
  - le prochain travail n'est plus de "rendre benchmarkable", mais d'expliquer l'ecart de debit

**Etat de certification globale au `2026-04-08`:**
- le blocage de compilation global sur `RuntimeTelemetrySnapshot` a ete corrige en realignant les compteurs ingress et `malloc_trim` entre `main_background.rs`, `main_telemetry.rs` et `bridge.rs`
- la suite complete `cargo test -- --nocapture` recompile a nouveau le binaire et le bridge
- les trois echecs MCP de certification ont ete corriges:
  - `semantic_clones` utilise maintenant le `graph.model_id` canonique du profil runtime, pas un `graph-bge-small-en-v1.5-384` fige
  - les fixtures `GraphEmbedding` des tests MCP sont construites a la dimension canonique courante, plus en `FLOAT[384]`
  - `axon_debug` calcule les files `GraphProjectionQueue` et `FileVectorizationQueue` sur la verite canonique SQL, pas sur une vue reader potentiellement en retard
- validation finale executee:
  - `cargo test --manifest-path src/axon-core/Cargo.toml -- --nocapture`
  - resultat: `251` tests lib verts, `48` tests bin verts, doc-tests verts
- bruit residuel connu:
  - plusieurs tests bootstrap impriment encore `ALTER TABLE soll.Registry ADD COLUMN last_gui ... already exists`
  - ce signal n'est pas bloquant pour la certification fonctionnelle, mais reste une dette de hygiene de bootstrap a traiter separement

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

**Status:** Complete le `2026-04-08`

**Resultat implemente:**
- le bootstrap cree maintenant `Symbol.embedding`, `ChunkEmbedding.embedding` et `GraphEmbedding.embedding` a partir de la dimension canonique du profil actif, au lieu d'un `FLOAT[384]` fige
- `RuntimeMetadata` persiste desormais le contrat embeddings minimal necessaire a la compatibilite runtime:
  - `embedding_version`
  - `embedding_dimension`
  - `embedding_model_name`
- un drift embeddings ne se contente plus d'invalider les donnees semantiques; il remet aussi le stockage physique au bon format:
  - `ALTER COLUMN` pour `Symbol.embedding`
  - recreation controlee de `ChunkEmbedding`
  - recreation controlee de `GraphEmbedding` et de son index unique
- les chemins d'ecriture runtime ne sont plus figes a `FLOAT[384]` pour `Symbol`, `ChunkEmbedding` et `GraphEmbedding`

**Validation executee:**
- `cargo test --lib embedding_schema_migration -- --nocapture`
- `cargo test --lib test_maillon_2c_ -- --nocapture`
- `cargo test --lib embedding_provider -- --nocapture`

**Vigilance residuelle hors perimetre Task 4:**
- plusieurs outils MCP et tests restent encore couples a des `model_id` historiques `*-bge-small-en-v1.5-384`
- ces usages ne bloquent plus la migration physique du stockage embeddings, mais devront etre realignes avant la bascule vers un nouveau profil primaire

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

**Status:** Complete le `2026-04-08`

**Resultat implemente:**
- un catalogue de profils embeddings existe desormais dans `embedder.rs`:
  - `JinaCodeV2Base`
  - `BgeBaseEnv15`
  - `LegacyBgeSmallEnv15`
- la pile par defaut est maintenant:
  - primaire: `jinaai/jina-embeddings-v2-base-code`
  - fallback: `BAAI/bge-base-en-v1.5`
- les deux profils modernes sont alignes en `768d`, ce qui evite une derive de stockage entre primaire et fallback
- le worker embeddings sait maintenant tenter une pile de profils plutot qu'un modele unique
- le contrat runtime peut etre derive d'un profil explicite, pas seulement du profil par defaut
- la selection est pilotable par environnement:
  - `AXON_EMBEDDING_PROFILE`
  - `AXON_EMBEDDING_FALLBACK_PROFILE`

**Validation executee:**
- `cargo test --lib jina_embedding_profile -- --nocapture`
- `cargo test --lib embedding_config -- --nocapture`
- `cargo test --lib test_embedding_runtime_contract_exposes_current_runtime_truth -- --nocapture`
- `cargo test --lib embedding_provider -- --nocapture`
- `cargo test --lib embedding_schema_migration -- --nocapture`

**Vigilance residuelle hors perimetre Task 5:**
- le bootstrap/runtime continue encore a raisonner surtout sur le profil canonique; la synchronisation explicite du profil effectivement charge reste a durcir si l'on veut une semantique parfaite du fallback au redemarrage
- plusieurs outils MCP et tests hors de cette tranche restent encore couples a des `model_id` historiques `*-384`

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

---

### Task 9: Benchmarker reellement le debit embeddings contre la cible strategique

**Status:** Complete le `2026-04-08`

**Resultat implemente:**
- un vrai harness benchmark existe maintenant dans:
  - `src/axon-core/src/embedding_benchmark.rs`
  - `src/axon-core/src/bin/embedding_benchmark.rs`
- il extrait un corpus local reel et mesure les surfaces:
  - `file`
  - `type`
  - `procedure`
- il produit un rapport JSON avec:
  - modele
  - dimension
  - backend demande
  - corpus reel
  - debit par cible
  - verdict versus `300_000 embeddings/h`
- la branche distingue explicitement:
  - `backend requested`
  - preuve externe GPU
  - absence encore d'un `provider_effective` fort dans la telemetrie runtime

**Validation executee:**
- `cargo test embedding_real_benchmark --manifest-path src/axon-core/Cargo.toml -- --nocapture`
- `cargo run --manifest-path src/axon-core/Cargo.toml --bin embedding_benchmark -- --help`

**Mesures reelles obtenues:**

CPU, `BAAI/bge-small-en-v1.5`:
- `file`: `~28_992 embeddings/h`
- `type`: `~33_549 embeddings/h`
- `procedure`: `~28_974 embeddings/h`

CPU, `jinaai/jina-embeddings-v2-base-code`:
- `file`: `~7_902 embeddings/h`
- `type`: `~14_464 embeddings/h`
- `procedure`: `~13_209 embeddings/h`

CUDA demande, `BAAI/bge-small-en-v1.5`:
- preuve externe: `nvidia-smi` a observe `~41%` GPU et `~798 MiB` VRAM utilises
- `file`: `~27_252 embeddings/h`
- `type`: `~22_153 embeddings/h`
- `procedure`: `~26_302 embeddings/h`

CUDA demande, `jinaai/jina-embeddings-v2-base-code`:
- preuve externe: `nvidia-smi` a observe `~28-34%` GPU et `~798-810 MiB` VRAM utilises
- `file`: `~7_973 embeddings/h`
- `type`: `~8_889 embeddings/h`
- `procedure`: `~12_519 embeddings/h`

**Conclusion ferme de Task 9:**
- la cible `300_000 embeddings/h` n'est pas atteinte
- l'ecart n'est pas marginal; il est massif
- le systeme est maintenant benchmarke reellement
- le probleme ouvert n'est plus un manque d'observabilite benchmark, mais un manque de debit effectif
- une incoherence de prod a aussi ete identifiee apres benchmark:
  - le worker calculait bien un profil GPU calibre
  - mais continuait a fetcher les symboles et projections graphe avec des constantes CPU historiques
  - cette derive a ete corrigee ensuite, avec tests cibles

**Vigilance residuelle hors perimetre Task 9:**
- `RuntimeProfile::detect()` continue a reporter `gpu_present=false` dans cet environnement alors qu'un GPU est bien visible par `nvidia-smi`
- `provider_effective` reste `null` dans le rapport JSON; la preuve actuelle de CUDA repose sur telemetrie externe, pas sur un signal runtime interne robuste

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

**Status:** Complete le `2026-04-08`

**Diagnostic confirme le `2026-04-08`:**
- le provider CUDA doit etre prouve explicitement au chargement du modele; la simple detection `gpu_present` ne suffit pas
- le hot path actuel sous-alimente le GPU:
  - un seul worker semantique
  - batchs conservateurs (`16/32/8/6`)
  - boucle fichier-par-fichier avec allers-retours `fetch -> embed -> write -> re-fetch`
- le goulot prioritaire n'est pas seulement la taille des batchs, mais l'absence de lot cross-file sous budget runtime
- la couture TDD retenue est mixte:
  - fonctions pures de calibration et de budget runtime
  - methode `GraphStore` capable de recuperer des chunks non vectorises sur plusieurs fichiers en une vague

**Resultat implemente:**
- le runtime embeddings sait maintenant calibrer le profil par backend demande:
  - lots plus grands en mode `cuda`
  - profil inchange en mode `cpu`
- la vectorisation fichier est pilotee par un budget runtime explicite:
  - `file_fetch_limit`
  - `total_chunk_budget`
  - `pause`
- le hot path fichier n'est plus strictement fichier-par-fichier:
  - le worker reclame plusieurs fichiers
  - recupere un lot global de chunks non vectorises sur plusieurs chemins
  - execute un seul `model.embed(...)` pour la vague
  - ne vide la queue que pour les fichiers devenus `vector_ready`
- les tests maillon proches du stockage embeddings ont ete realignes sur le contrat `768d` actuel

**Validation executee:**
- `cargo test --lib file_vectorization_throughput -- --nocapture`
- `cargo test --lib embedding_config -- --nocapture`
- `cargo test --lib embedding_benchmark -- --nocapture`
- `cargo test --lib test_maillon_2r4_vector_ready_flips_true_after_chunk_embeddings_land -- --nocapture`
- `cargo test --lib test_maillon_7b_chunk_embedding_storage -- --nocapture`
- `cargo test --lib test_maillon_7e_chunk_invalidation_requeues_only_changed_file_embeddings -- --nocapture`
- `cargo test --lib test_tombstone_missing_path_invalidates_dependent_graph_derivations -- --nocapture`

**Vigilance residuelle hors perimetre Task 6:**
- le provider GPU reel reste encore infere a partir du backend demande; la preuve definitive du provider effectivement retenu au runtime devra etre durcie plus tard
- plusieurs outils MCP et tests hors de cette tranche restent encore couples a des identifiants `*-384`
- les messages `ALTER TABLE ... last_gui already exists` restent visibles au bootstrap de test; ce bruit n'a pas ete traite dans cette tranche

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

**Status:** Complete le `2026-04-08`

**Resultat implemente:**
- un harness comparatif pur existe maintenant pour les profils embeddings Axon
- la matrice couvre:
  - `jinaai/jina-embeddings-v2-base-code`
  - `BAAI/bge-base-en-v1.5`
  - `BAAI/bge-small-en-v1.5`
- le reporting est volontairement en mode `proxy`, pour rester stable en TDD locale:
  - `profile_key`
  - `backend`
  - `model_name`
  - `dimension`
  - `symbol/chunk/graph model_id`
  - batch sizes calibres
  - budget runtime de vectorisation fichier
- la comparaison expose aussi les lignes GPU demandees pour `jina` et `bge-base`, sans charger les modeles ni telecharger quoi que ce soit

**Validation executee:**
- `cargo test --lib embedding_profile_benchmark -- --nocapture`

**Vigilance residuelle hors perimetre Task 7:**
- ce harness mesure un contrat comparatif stable, pas encore une inference reelle ni un vrai debit `embeddings/s`
- le mode benchmark reel devra etre ajoute dans une tranche suivante si l'on veut chronometrer `cold_start`, `warm_infer` et `db_write`
- la qualite retrieval reste encore hors de ce harness proxy

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

**Pivot de realite constate le `2026-04-08`:**
- la branche n'expose encore qu'une matrice proxy, pas un benchmark reel d'inference
- cette task doit donc etre executee comme une vraie tranche d'implementation:
  - harness de benchmark reel
  - corpus local representatif
  - sortie JSON stable
  - verdict explicite contre la cible `300_000 embeddings/h`
  - distinction claire `backend demande` vs `backend observe`

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Create: `src/axon-core/src/embedding_benchmark.rs`
- Create: `src/axon-core/src/tests/embedding_real_benchmark_tests.rs`
- Create: `src/axon-core/src/bin/embedding_benchmark.rs`
- Modify: `src/axon-core/src/lib.rs`
- Test: tests cibles + benchmark Axon reel

**Etat courant du `2026-04-08`:**
- harness reel implemente
- tests de contrat du harness verts
- benchmark CPU reel execute
- verite mesuree:
  - `bge-small 384d`: environ `29k` a `33.5k embeddings/h`
  - `jina 768d`: environ `7.9k` a `14.5k embeddings/h` sur un run reduit
- cible `300k/h` non atteinte en CPU
- `gpu_present=false` dans cet environnement, donc aucune certification GPU possible ici

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
