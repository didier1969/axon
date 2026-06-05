# Axon

Axon est un runtime d’intelligence de code **Rust-first** pour l’indexation structurelle, la récupération de contexte pour LLM, et la continuité conceptuelle `SOLL`.

## Commencer ici

Si tu reprends le projet, lis dans cet ordre :

1. `README.md`
2. `docs/getting-started.md`
3. `STATE.md`
4. `ROADMAP.md`
5. `docs/working-notes/reality-first-stabilization-handoff.md`
6. `docs/working-notes/2026-04-01-reprise-handoff.md`
7. `docs/archive/README.md` seulement si tu as besoin du contexte historique

## Réalité actuelle

Au 2026-06-05, la vérité prouvée est la suivante :

- **Rust** est l’autorité de runtime, d’ingestion, de delta, de `IST`, de `MCP` et de `SQL`
- **Elixir/Phoenix** sert la visualisation, la télémétrie opérateur et les projections de lecture
- le backend canonique est **PostgreSQL 17 + pgvector** (HNSW, embeddings BGE-Large 1024d), `pgmq` pour les files de travail asynchrones, et FTS via colonne `tsvector` (`content_tsv`)
- l’`IST` vit dans le schéma `ist.*` ; `SOLL` vit dans le schéma `soll.*`
- **DuckDB/Canard, AGE, KuzuDB, Titan, HydraDB et le plugin FFI sont tous retirés/purgés** — aucune rétrocompatibilité

Le point important est celui-ci :

- le dashboard **n’est pas** l’autorité canonique d’ingestion
- il observe et rend la vérité produite par le runtime Rust
- le runtime Rust admet le travail selon un budget mémoire dynamique

## Ce qu’Axon fait réellement

- scanne un univers de projets et priorise le projet actif
- applique les règles d’ignore, les capabilities de parsing, puis écrit dans `IST` (schéma `ist.*`)
- estime le coût d’ingestion par `parser class + size bucket + confiance observée`
- admet un lot de fichiers qui tient dans le budget mémoire courant au lieu de dépendre d’un seuil fixe par taille
- refuse explicitement un fichier trop volumineux s’il ne peut pas tenir même seul dans l’enveloppe runtime
- indexe la structure du code dans **PostgreSQL 17** (schéma `ist.*`, edges canoniques en RAM `IstGraphView`, persistés via `ist.edge`)
- expose la vérité runtime via:
  - `MCP`
  - `SQL`
  - dashboard de visualisation
- protège `SOLL` comme couche conceptuelle séparée de `IST`
- dérive ensuite les couches sémantiques:
  - `Chunk`
  - `ChunkEmbedding`
  - `GraphProjection`
  - `GraphEmbedding`

## Architecture actuelle

- **Runtime canonique:** Rust + PostgreSQL 17 + pgvector (HNSW, BGE-Large 1024d), `pgmq`, FTS `tsvector`
- **Surface opérateur:** Phoenix/LiveView
- **Environnement local officiel:** Nix + Devenv
- **Gouvernance et Isolation (SOTA):** Le système implémente une stratégie **Dual-Track** absolue.
  - La **Production** (`live`) tourne sur les ports `44129` (MCP/SQL) et `44127` (Dashboard) sur la base réelle (`.axon/`).
  - Le **Développement** (`dev`) tourne sur les ports `44139` (MCP/SQL) et `44137` (Dashboard) sur une base clonée (`.axon-dev/`) pour permettre aux LLMs de prototyper sans aucun risque de corrompre la production.

## Workflow canonique

Avant tout travail:

```bash
devenv shell
./scripts/validate-devenv.sh
```

Bootstrap initial ou après dérive importante:

```bash
./scripts/setup.sh
```

Démarrage quotidien (façade 4-verbes canonique, orchestrée par process-compose):

```bash
./scripts/axon --instance dev start full      # dev + vectorisation
./scripts/axon --instance live start          # production
```

Arrêt propre:

```bash
./scripts/axon --instance live stop
./scripts/axon --instance dev stop
```

Statut:

```bash
./scripts/axon --instance live status
```

## Ce que font les scripts

- `setup.sh`
  - prépare l’environnement
  - compile le core Rust
  - compile le dashboard Elixir
  - exécute les validations principales

- `./scripts/axon {start|stop|status|qualify}` (alias `axon-live` / `axon-dev`)
  - surface 4-verbes canonique (DEC-AXO-060), orchestrée par **process-compose**
  - `start`: vérifie l’environnement, sélectionne l’instance `live`/`dev`, démarre brain + indexer + dashboard
  - `stop`: arrête uniquement les processus Axon (par PID/superviseur, jamais de `pkill` large)
  - `status`: vérité runtime, fraîcheur IST, prochaine action

## Vérification minimale après démarrage

Une fois `./scripts/axon ... start` terminé, les surfaces attendues sont celles affichées par le script.

Les ports sont stables par défaut, mais l’adresse annoncée peut dépendre du bind courant de l’environnement local.

Par défaut (`live`):

- dashboard: `http://127.0.0.1:44127/cockpit`
- SQL: `http://127.0.0.1:44129/sql`
- MCP: `http://127.0.0.1:44129/mcp`

Exemple de vérification SQL:

```bash
curl -sS -X POST http://127.0.0.1:44129/sql \
  -H "content-type: application/json" \
  --data '{"query":"SELECT count(*) FROM ist.indexed_file"}'
```

## Notes d’état

- `IST` est reconstructible et peut être invalidée proprement selon la politique de compatibilité runtime
- `SOLL` reste protégée et exportable/restaurable
- le chemin live des nouveaux exports `SOLL` est `docs/vision/`
- les snapshots historiques déplacés vivent dans `docs/archive/soll-exports/`
- les docs archivées dans `docs/archive/` ne doivent pas être lues comme contrat courant
- la règle d’admission canonique est le budget mémoire Rust dynamique
