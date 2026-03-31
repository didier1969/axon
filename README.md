# Axon

Axon est un runtime d’intelligence de code **Rust-first** pour l’indexation structurelle, la récupération de contexte pour LLM, et la continuité conceptuelle `SOLL`.

Aujourd’hui, la frontière canonique est la suivante:
- **Rust** est l’autorité de runtime, d’ingestion, de delta, de `IST`, de `MCP` et de `SQL`
- **Elixir/Phoenix** sert la visualisation, la télémétrie opérateur et les projections de lecture

## Ce qu’Axon fait réellement

- scanne un univers de projets et priorise le projet actif
- applique `Axon Ignore`, les capabilities de parsing, puis écrit dans `IST`
- indexe la structure du code dans une base DuckDB embarquée
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

- **Runtime canonique:** Rust + DuckDB
- **Surface opérateur:** Phoenix/LiveView
- **Environnement local officiel:** Nix + Devenv
- **HydraDB:** détachée du workflow quotidien actuel

Point important:
- le dashboard **n’est pas** l’autorité d’ingestion
- il observe et rend la vérité produite par le runtime Rust

## Workflow canonique

Avant tout travail:

```bash
devenv shell
./scripts/validate-devenv.sh
```

Bootstrap initial ou après dérive importante:

```bash
./scripts/setup_v2.sh
```

Démarrage quotidien:

```bash
./scripts/start-v2.sh
```

Arrêt propre:

```bash
./scripts/stop-v2.sh
```

## Ce que font les scripts

- `setup_v2.sh`
  - prépare l’environnement
  - compile le core Rust
  - compile le dashboard Elixir
  - exécute les validations principales

- `start-v2.sh`
  - vérifie l’environnement Devenv
  - auto-répare le binaire `release` si nécessaire
  - démarre Axon dans `tmux`
  - attend le dashboard et le runtime
  - vérifie la surface SQL live quand le core est prêt

- `stop-v2.sh`
  - arrête uniquement les processus Axon
  - ferme la session `tmux`
  - nettoie sockets, locks et WAL locaux

## Vérification minimale après démarrage

Une fois `./scripts/start-v2.sh` terminé, les surfaces attendues sont:

- dashboard: `http://127.0.0.1:44127/cockpit`
- SQL: `http://127.0.0.1:44129/sql`
- MCP: `http://127.0.0.1:44129/mcp`

Exemple de vérification SQL:

```bash
curl -sS -X POST http://127.0.0.1:44129/sql \
  -H "content-type: application/json" \
  --data '{"query":"SELECT count(*) FROM File"}'
```

## Notes d’état

- `IST` est reconstructible et peut être invalidée proprement selon la politique de compatibilité runtime
- `SOLL` reste protégée et exportable/restaurable
- Python reste présent surtout pour les bridges Datalog/TypeQL et quelques outillages encore tolérés
- HydraDB ne fait plus partie du chemin nominal journalier
