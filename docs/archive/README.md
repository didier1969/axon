# Archive Documentation Policy

Cette arborescence contient des documents **historiques**.

Ils restent utiles pour comprendre l'évolution d'Axon, mais ils ne définissent plus le contrat d'exécution courant du projet.

## Classes de documents

- **Canonique**
  - À lire en premier pour reprendre le projet.
  - Source de vérité documentaire actuelle.
- **Archive**
  - Historique de migration, anciennes architectures, anciens prompts, anciennes specs.
  - À consulter seulement si le contexte historique est nécessaire.
- **Généré**
  - Snapshots et exports produits par des outils.
  - Utiles comme trace, mais pas comme documentation normative.

## Ce qui est archivé ici

- `v1.0/`
  Ancienne documentation du modèle Triple-Pod / HydraDB.
- `v2/`
  Documentation intermédiaire de migration, non canonique pour la reprise actuelle.
- `root-docs/`
  Documents racine historiques qui n'ont plus vocation à guider la reprise active.
- `soll-exports/`
  Extractions horodatées de `SOLL` conservées comme snapshots historiques.

## Ce qui reste canonique

Commencer par:

- `/home/dstadel/projects/axon/README.md`
- `/home/dstadel/projects/axon/docs/getting-started.md`
- `/home/dstadel/projects/axon/STATE.md`
- `/home/dstadel/projects/axon/ROADMAP.md`
- `/home/dstadel/projects/axon/docs/working-notes/reality-first-stabilization-handoff.md`
- `/home/dstadel/projects/axon/docs/working-notes/2026-04-01-reprise-handoff.md`

## SOLL exports

Le chemin canonique live pour les nouveaux exports `SOLL` est `docs/vision/`.

Les fichiers présents dans `docs/archive/soll-exports/` sont des snapshots historiques déplacés hors des chemins source afin de ne plus polluer la lecture du projet.
