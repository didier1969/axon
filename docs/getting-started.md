# Getting Started with Axon

Ce document décrit le **workflow source checkout canonique** du dépôt Axon.

Pour l’instant, la vérité opératoire est:
- **Rust** est le runtime canonique
- **Elixir/Phoenix** sert la visualisation et les diagnostics
- **PostgreSQL 17 + pgvector** est le backend canonique (HNSW, BGE-Large 1024d, `pgmq`, FTS `tsvector`) ; IST dans le schéma `ist.*`, SOLL dans `soll.*`
- DuckDB/Canard, AGE, KuzuDB, Titan, HydraDB et le plugin FFI sont tous retirés/purgés
- les documents sous `docs/archive/` sont historiques, pas normatifs

Avant de plonger dans les archives, lire d’abord:

- `README.md`
- `STATE.md`
- `ROADMAP.md`
- `docs/working-notes/reality-first-stabilization-handoff.md`
- `docs/working-notes/2026-04-01-reprise-handoff.md`

## Prérequis

- Nix
- Devenv
- `tmux`
- `curl`
- `nc`

## 1. Entrer dans l’environnement officiel

```bash
devenv shell
./scripts/validate-devenv.sh
```

Si le validateur échoue, le shell courant n’est pas l’environnement supporté pour Axon.

## 2. Bootstrap initial

```bash
./scripts/setup.sh
```

Ce script:
- compile le core Rust
- prépare et compile le dashboard Elixir
- exécute les validations principales

## 3. Démarrer Axon

## 3a. Installer TensorRT localement

TensorRT n’est pas téléchargé implicitement par Axon. Pour une installation client reproductible,
le tarball NVIDIA approuvé doit être fourni localement, puis Axon valide le nom, la version et le
checksum avant de construire l’artefact ONNX Runtime TensorRT.

Chemin attendu par défaut:

```bash
.axon/downloads/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz
```

Prévalidation rapide:

```bash
./scripts/axon setup-tensorrt --precheck-only
```

Installation de l’artefact TensorRT:

```bash
./scripts/axon setup-tensorrt --build-only
```

Installation + qualification VRAM bornée:

```bash
./scripts/axon setup-tensorrt --qualify \
  --max-vram-used-mb 2048 \
  --tensorrt-workspace-mb 1024
```

Démarrage explicite d'un indexer vectoriel/full avec TensorRT:

```bash
./scripts/axon --instance dev start --indexer-full --tensorrt
./scripts/axon --instance live start --indexer-full --tensorrt
```

`--tensorrt` refuse les modes non vectoriels (`brain_only`, `indexer_graph`) afin d'éviter une configuration affichée comme accélérée alors que la lane vectorielle ne tourne pas.
Sur les cartes 8 GB, la qualification considère `7900 MiB` utilisés comme un overshoot dur et arrête le run.

Le bootstrap global peut aussi déclencher TensorRT:

```bash
./scripts/setup.sh --with-tensorrt
./scripts/setup.sh --with-tensorrt --tensorrt-qualify
```

Contrat:
- `setup-tensorrt` est la procédure reproductible pour installer l’artefact ORT TensorRT Axon
- le profil par défaut `axon_embedding` évite les kernels ORT non utiles à la vectorisation Axon
- le build ORT force le parser TensorRT intégré; il ne doit pas télécharger ni compiler `onnx-tensorrt`
- la prévalidation doit vérifier `NvInfer.h`, `NvOnnxParser.h`, `libnvinfer.so`, `libnvonnxparser.so` et `libnvinfer_plugin.so` avant le build long
- le manifest généré devient la source de vérité runtime pour `libonnxruntime` et les providers CUDA/TensorRT
- aucune installation client ne doit dépendre d’une commande tapée à la main hors scripts

## 3b. Démarrer Axon

Entrypoints canoniques:

```bash
./scripts/axon-live start --full
./scripts/axon-dev start --full
```

Les wrappers `axon-live` et `axon-dev` ciblent explicitement les deux instances.
La forme équivalente via la façade unique est:

```bash
./scripts/axon --instance live start --full
./scripts/axon --instance dev start --full
```

```bash
./scripts/axon-live start --full
```

Le script:
- vérifie l’environnement
- sélectionne l’instance `live` ou `dev`
- resynchronise ou réhydrate `bin/axon-core` selon l’instance
- démarre Axon dans `tmux`
- attend le dashboard et la surface SQL
- vérifie `MCP`

Modes utiles:

```bash
./scripts/axon-live start --graph-only
./scripts/axon-dev start --full
```

- `graph_only`: surface graphe/MCP légère, sans ingestion autonome complète
- `full`: serveur partagé complet, avec surface MCP complète et mutations routées via jobs

## 4. Vérifier la surface

Sur `live`:

- dashboard: `http://127.0.0.1:44127/cockpit`
- SQL: `http://127.0.0.1:44129/sql`
- MCP: `http://127.0.0.1:44129/mcp`

Sur `dev`:

- dashboard: `http://127.0.0.1:44137/cockpit`
- SQL: `http://127.0.0.1:44139/sql`
- MCP: `http://127.0.0.1:44139/mcp`

Exemple:

```bash
curl -sS -X POST http://127.0.0.1:44129/sql \
  -H "content-type: application/json" \
  --data '{"query":"SELECT count(*) FROM File"}'
```

Vérification opératoire rapide:

```bash
./scripts/axon-live status
./scripts/axon-dev status
./scripts/axon qualify --profile smoke --mode graph_only
./scripts/axon-live qualify-mcp --surface core --checks quality --project AXO
./scripts/axon-dev qualify-mcp --surface core --checks quality --project AXO
```

Règles:

- `./scripts/axon qualify ...` cible `dev` par défaut
- pour qualifier explicitement `live`, utiliser `./scripts/axon --instance live qualify ...`
- la qualification runtime archive maintenant aussi:
  - `runtime-status.json`
  - `runtime-quiescent-summary.json`
- si le runtime est joignable mais que `runtime_quiescent` est encore `watch` ou `blocked`, le `runtime_smoke` remonte en `warn`

## 5. Contrat MCP `0.1`

- le serveur MCP partagé expose les outils de lecture et les outils mutateurs
- les outils mutateurs ne modifient pas l’état en ligne directement: ils créent un job serveur
- la réponse mutatrice doit rendre immédiatement les identifiants utiles quand ils existent déjà:
  - `job_id`
  - `entity_id`
  - `preview_id`
  - `revision_id`
- l’état détaillé du serveur et des jobs se lit depuis le dashboard, en lecture seule

## 6. Dashboard

Le cockpit `http://127.0.0.1:44127/cockpit` est la surface de lecture opératoire:

- santé runtime
- disponibilité SQL/MCP
- progression graph/embedding
- jobs MCP récents et leurs statuts

Le dashboard n’est pas une surface d’action. Les mutations passent par MCP, puis sont observées dans le cockpit.

## 7. Seed `dev` depuis `live`

Pour rafraîchir la base de développement depuis la vérité `live`:

```bash
./scripts/axon seed-dev-from-live
```

Par défaut:

- `dev` doit être arrêté
- `live` doit être arrêté pour un snapshot cohérent
- un backup timestampé du root `dev` est créé sous `.axon-dev/backups/`

Le mode `--allow-live-running` existe, mais reste un flux best-effort avec copie de la WAL.

## 8. Arrêter Axon

```bash
./scripts/axon --instance live stop
./scripts/axon --instance dev stop
```

Le script arrête uniquement les processus Axon et nettoie sockets, locks et WAL locaux.

## Notes utiles

- `IST` est la vérité technique reconstructible
- `SOLL` est la vérité conceptuelle protégée
- `SOLL` contient des données de production: ne jamais la purger ni la réinitialiser
- l’autorité publique MCP/SOLL reste portée par le `brain`; le `indexer` ne doit pas redevenir une autorité `SOLL`
- en runtime split, `indexer` traite `IST`/ingestion et le `brain` porte la surface MCP, la lecture `SOLL` et l’écriture `SOLL`
- le chemin canonique des exports `SOLL` est `docs/vision/`
- les snapshots historiques déplacés vivent dans `docs/archive/soll-exports/`
- la documentation HTML sous `docs/derived/soll/` est dérivée et non canonique: elle sert à la lecture, pas au restore
- pour une lecture compacte de l’intention canonique, préférer `soll_query_context`, `soll_work_plan` et `soll_validate`
- Python reste présent surtout pour les bridges Datalog/TypeQL
- le vieux flux CLI `pip install axoniq` n’est **pas** le workflow source checkout canonique actuel
