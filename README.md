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

Au 2026-04-01, la vérité prouvée est la suivante :

- **Rust** est l’autorité de runtime, d’ingestion, de delta, de `IST`, de `MCP` et de `SQL`
- **Elixir/Phoenix** sert la visualisation, la télémétrie opérateur et les projections de lecture
- le backend nominal du chemin courant est **Canard DB** (`DuckDB` embarquée)
- **KuzuDB** fait partie de l’historique du projet, pas du chemin quotidien actuel

Le point important est celui-ci :

- le dashboard **n’est pas** l’autorité canonique d’ingestion
- il observe et rend la vérité produite par le runtime Rust
- le runtime Rust n’utilise plus de classe canonique `Titan`; il admet le travail selon un budget mémoire dynamique
- une dette de migration Elixir subsiste encore côté `Watcher`, mais elle ne remet pas en cause le socle exécutable actuel

## Ce qu’Axon fait réellement

- scanne un univers de projets et priorise le projet actif
- applique `Axon Ignore`, les capabilities de parsing, puis écrit dans `IST`
- estime le coût d’ingestion par `parser class + size bucket + confiance observée`
- admet un lot de fichiers qui tient dans le budget mémoire courant au lieu de dépendre d’un seuil fixe par taille
- refuse explicitement un fichier `oversized_for_current_budget` s’il ne peut pas tenir même seul dans l’enveloppe runtime
- indexe la structure du code dans une base **Canard DB** embarquée
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

- **Runtime canonique:** Rust + Canard DB (`DuckDB`)
- **Surface opérateur:** Phoenix/LiveView
- **Environnement local officiel:** Nix + Devenv
- **HydraDB:** détachée du workflow quotidien actuel
- **Embeddings code actuels:** profil primaire `jinaai/jina-embeddings-v2-base-code`, fallback `BAAI/bge-base-en-v1.5`, stockage dimensionnel gouverné par le runtime
- **Sélection du backend embeddings:** `AXON_EMBEDDING_BACKEND=auto|cpu|cuda` permet maintenant de forcer le backend indépendamment de l’heuristique locale `gpu_present`

Référence d’architecture pour cette filière:

- `docs/architecture/2026-04-08-gpu-code-embeddings.md`

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

Démarrage quotidien:

```bash
./scripts/start.sh
```

Arrêt propre:

```bash
./scripts/stop.sh
```

## Ce que font les scripts

- `setup.sh`
  - prépare l’environnement
  - compile le core Rust
  - compile le dashboard Elixir
  - exécute les validations principales

- `start.sh`
  - vérifie l’environnement Devenv
  - auto-répare le binaire `release` si nécessaire
  - démarre Axon dans `tmux`
  - attend le dashboard et le runtime
  - vérifie la surface SQL live quand le core est prêt

- `dev-fast.sh`
  - boucle courte Rust avec `cargo check` ou tests filtrés
  - réutilise un `target-dir` partagé
  - active l’incrémental par défaut
  - permet `sccache` en opt-in via `AXON_RUST_CACHE_MODE=sccache`
  - évite de relancer une suite complète pour chaque micro-changement

- `stop.sh`
  - arrête uniquement les processus Axon
  - ferme la session `tmux`
  - nettoie sockets, locks et WAL locaux

## Vérification minimale après démarrage

Une fois `./scripts/start.sh` terminé, les surfaces attendues sont celles affichées par le script.

Les ports sont stables par défaut, mais l’adresse annoncée peut dépendre du bind courant de l’environnement local.

Par défaut:

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
- le chemin live des nouveaux exports `SOLL` est `docs/vision/`
- les snapshots historiques déplacés vivent dans `docs/archive/soll-exports/`
- Python reste présent surtout pour les bridges Datalog/TypeQL et quelques outillages encore tolérés
- HydraDB ne fait plus partie du chemin nominal journalier
- les docs archivées dans `docs/archive/` ne doivent pas être lues comme contrat courant
- `Titan` ne fait plus partie du contrat d’ingestion courant; la règle canonique est le budget mémoire Rust
- les embeddings ne sont plus un contrat figé `384d`; toute bascule de profil doit suivre le runbook documenté dans `docs/architecture/2026-04-08-gpu-code-embeddings.md`
