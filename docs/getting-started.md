# Getting Started with Axon

Ce document décrit le **workflow source checkout canonique** du dépôt Axon.

Pour l’instant, la vérité opératoire est:
- **Rust** est le runtime canonique
- **Elixir/Phoenix** sert la visualisation et les diagnostics
- **Canard DB** (`DuckDB`) est le backend embarqué nominal
- **HydraDB** n’est pas dans le chemin nominal quotidien
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
./scripts/setup_v2.sh
```

Ce script:
- compile le core Rust
- prépare et compile le dashboard Elixir
- exécute les validations principales

## 3. Démarrer Axon

```bash
./scripts/start-v2.sh
```

Le script:
- vérifie l’environnement
- resynchronise `bin/axon-core`
- démarre Axon dans `tmux`
- attend le dashboard et la surface SQL
- vérifie `MCP`

## 4. Vérifier la surface live

Sur une instance démarrée:

- dashboard: `http://127.0.0.1:44127/cockpit`
- SQL: `http://127.0.0.1:44129/sql`
- MCP: `http://127.0.0.1:44129/mcp`

Exemple:

```bash
curl -sS -X POST http://127.0.0.1:44129/sql \
  -H "content-type: application/json" \
  --data '{"query":"SELECT count(*) FROM File"}'
```

## 5. Arrêter Axon

```bash
./scripts/stop-v2.sh
```

Le script arrête uniquement les processus Axon et nettoie sockets, locks et WAL locaux.

## Notes utiles

- `IST` est la vérité technique reconstructible
- `SOLL` est la vérité conceptuelle protégée
- le chemin live des exports `SOLL` est `docs/vision/`
- les snapshots historiques déplacés vivent dans `docs/archive/soll-exports/`
- Python reste présent surtout pour les bridges Datalog/TypeQL
- le vieux flux CLI `pip install axoniq` n’est **pas** le workflow source checkout canonique actuel
