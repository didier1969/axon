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
./scripts/status-live.sh
./scripts/status-dev.sh
./scripts/axon-live qualify-mcp --surface core --checks quality --project AXO
./scripts/axon-dev qualify-mcp --surface core --checks quality --project AXO
```

La qualification et le `status` doivent toujours cibler explicitement l’instance voulue.

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
./scripts/stop-live.sh
./scripts/stop-dev.sh
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
