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
./scripts/setup.sh
```

Ce script:
- compile le core Rust
- prépare et compile le dashboard Elixir
- exécute les validations principales

## 3. Démarrer Axon

```bash
./scripts/start.sh
```

Le script:
- vérifie l’environnement
- resynchronise `bin/axon-core`
- démarre Axon dans `tmux`
- attend le dashboard et la surface SQL
- vérifie `MCP`

Modes utiles:

```bash
./scripts/start.sh --graph-only
./scripts/start.sh --full
```

- `graph_only`: surface graphe/MCP légère, sans ingestion autonome complète
- `full`: serveur partagé complet, avec surface MCP complète et mutations routées via jobs

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

Vérification opératoire rapide:

```bash
./scripts/status.sh
python3 scripts/qualify_runtime.py --profile smoke --mode graph_only
python3 scripts/qualify_runtime.py --profile smoke --mode full
```

La qualification smoke vérifie le démarrage runtime, la surface MCP et le cockpit en conditions réelles.

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

## 7. Arrêter Axon

```bash
./scripts/stop.sh
```

Le script arrête uniquement les processus Axon et nettoie sockets, locks et WAL locaux.

## Notes utiles

- `IST` est la vérité technique reconstructible
- `SOLL` est la vérité conceptuelle protégée
- `SOLL` contient des données de production: ne jamais la purger ni la réinitialiser
- le chemin live des exports `SOLL` est `docs/vision/`
- les snapshots historiques déplacés vivent dans `docs/archive/soll-exports/`
- Python reste présent surtout pour les bridges Datalog/TypeQL
- le vieux flux CLI `pip install axoniq` n’est **pas** le workflow source checkout canonique actuel
