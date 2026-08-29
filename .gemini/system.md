# Axon — amorçage LLM (projet `AXO`)

Ce dépôt est gouverné par **Axon MCP** : mémoire structurelle (IST + SOLL) et méthodologie exécutable.
La SOLL fait foi ; ce fichier n'est qu'un point d'entrée. Source canonique : `PRT-PRO-999`.
**Ne rien dupliquer ici — pointer par ID.**

## Connexion

Même surface (114 outils) sur les deux transports ; le harnais choisit :
stdio `~/.local/bin/axon-mcp` · HTTP `http://127.0.0.1:44129/mcp`

## Séquence d'ouverture de session — `GUI-PRO-102`

1. `status mode=brief` — état runtime + fraîcheur. La fraîcheur **calibre la confiance, elle ne bloque jamais** (`CPT-AXO-029`).
2. `axon_init_project project_path=<cwd>` — Vision et Pillars arrivent inlinés dans la réponse. **Jamais de `sql SELECT description`.**
3. `practice_recall scope=AXO` — mémoire « comment travailler ». Primaire, pas optionnelle.
4. `mcp_inbox_read` — lire les **corps**, pas seulement les sujets.
5. `soll_get(id=<session pointer>)`, puis `soll_validate`, puis `soll_work_plan top=8`.
6. Émettre les 5 sections du contrat de sortie Phase B.

## Séquence de travail

Analyse → jeu de changements explicite → lot d'implémentation → **validation de fin de tranche**.
Ne pas dériver en boucles opportunistes patch / test / rapport.

## Navigation de code

`query` → `inspect` → `why` / `impact` / `path` **avant** tout grep. `retrieve_context` pour un dossier de preuve.
Une note « l'outil X est cassé » est une **hypothèse à falsifier** par un appel-test, pas un fait.

## Livraison

`axon_pre_flight_check` → `axon_commit_work`. **Jamais `git commit` brut.**
Tenir la SOLL à jour en continu : décision arrêtée, exigence stabilisée, point de contrôle → `soll_manager`, puis `soll_attach_evidence`.
Clôture de session : `GUI-PRO-028` — procédure via `skill_invoke id=SKI-PRO-1006`.

## Méthode

Les procédures vivent dans Axon, pas dans des fichiers :
`skill_list` pour découvrir · `skill_invoke id=SKI-PRO-N` pour le corps · `prompt_template_get` pour un gabarit.
Quand une méthode existe aussi sur disque, **la SOLL fait foi**.

## Règles dures de ce dépôt

Corps via `soll_get(id=…)` — ne pas recopier :

| ID | Règle |
|---|---|
| `GUI-AXO-1034` | Porte de build : `--lib` + `--bins` + `cargo build --tests` |
| `GUI-AXO-1035` | Sonder l'hôte avant toute suite `--lib` complète |
| `GUI-AXO-1036` | Build et test **dans** `devenv shell` |
| `GUI-AXO-1037` | `promote_live_safe.sh` est le seul chemin vers `bin/` |
| `GUI-AXO-1038` | Politique de données : SOLL jamais supprimée |
| `GUI-AXO-1039` | Jamais de `pkill` large |

## Arrêts

S'arrêter uniquement sur : action destructive irréversible · décision d'architecture réclamant l'humain · blocage externe dur.
Les choix réversibles se tranchent sans demander.
