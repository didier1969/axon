# Axon `v0.1` Publication State

Ce document fixe le périmètre opératoire réellement qualifié pour la publication `v0.1`.

## Cible

`v0.1` est un serveur Axon partagé, exécutable localement, avec:

- runtime Rust canonique
- dashboard Phoenix en lecture seule
- surface SQL et MCP actives
- profils runtime explicites
- chaîne graph + embeddings disponible
- outils MCP mutateurs routés via jobs

## Profils qualifiés

Les profils recertifiés au smoke sont:

- `graph_only`
- `full`

Commandes de qualification utilisées:

```bash
python3 scripts/qualify_runtime.py --profile smoke --mode graph_only
python3 scripts/qualify_runtime.py --profile smoke --mode full
```

Résultat attendu:

- `overall_verdict=pass`
- `runtime_smoke: pass`
- `mcp_validate: pass`

## Contrat runtime

Le runtime publié doit rendre visible:

- `runtime_mode`
- `runtime_profile`
- `boot_phase`
- `boot_status`

La vérité opératoire se vérifie via:

```bash
./scripts/status.sh
```

État attendu sur une instance saine:

- `axon-core` joignable
- dashboard joignable
- MCP joignable
- `STATUS HEALTHY`

## Contrat MCP

La surface MCP `v0.1` couvre:

- outils de lecture
- outils de diagnostic
- outils mutateurs `SOLL`

Règle opératoire:

- une mutation ne s’exécute pas en ligne
- elle crée un job serveur
- le client reçoit immédiatement les identifiants utiles déjà réservables

Identifiants critiques exposés selon le tool:

- `job_id`
- `entity_id`
- `preview_id`
- `revision_id`

Un outil `job_status` permet ensuite de suivre l’issue du job.

## Dashboard

Le cockpit est une surface de lecture complète:

- santé système
- état runtime
- disponibilité MCP/SQL
- progression graph/embedding
- jobs MCP récents
- compteurs `queued/running/succeeded/failed`

Le dashboard ne pilote pas le runtime. Il observe le système partagé.

## Données

Invariants opératoires:

- `SOLL` est une base de production protégée
- aucune purge ou réinitialisation destructive n’est acceptable sur `SOLL`
- `IST` est reconstructible si nécessaire

## Point de fermeture

La base `v0.1` est considérée fermée quand:

- `graph_only` smoke passe
- `full` smoke passe
- le dashboard lecture reste cohérent
- la surface MCP répond et route les mutations via jobs
