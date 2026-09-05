# Session 139 — le promote enfin fait, une queue de latence à 1 Hz, et six frictions client vérifiées

*2026-09-05 · `v0.8.0-1727-g8ade669a` servi · HEAD `b5527b5a`*

## Ce que la session a livré

| commit | REQ | quoi |
|---|---|---|
| `8ade669a` | `902622` | lire le chemin ORT par sa FORME, pas par sa position |
| `86e0aee6` | `902623` | refuser un start CUDA sur un paquet sans provider CUDA |
| `7f90710d` | `902589` (a) | le tick 1 Hz de télémétrie quitte le runtime qui sert MCP |
| `b5527b5a` | — | la porte `GUI-AXO-1034` compte cinq commandes, et `--class` est mort |

Promote fait et vérifié : phase `clean`, 7/7 gates. **Mais `86e0aee6` et `7f90710d` sont postérieurs
au promote : ils sont commités, pas servis.**

## Le fil de la journée

Le promote a échoué deux fois avant d'aboutir. La deuxième fois disait
« Unable to materialize a valid ONNX Runtime output path » — sur un build nix qui avait **réussi**,
et dont le log joint portait `exit_code: 0, state: "succeeded"`.

La cause tenait en une ligne : `nix build --print-out-paths 2>&1 | tee LOG | tail -n 1`. Le `nix` du
PATH est un shim d'admission qui écrit son rapport — deux lignes puis un JSON de ~10 Ko — sur stderr,
**après** le résultat. Le `2>&1` le fusionne, et `tail -n 1` rend le JSON.

Le signal qui aurait fait gagner une heure : *un message d'échec dont le log joint dit que la commande
a réussi*. Ce n'est pas la commande qui a échoué, c'est la lecture de sa sortie.

## La queue de latence — trois gestes, aucune instrumentation

`REQ-AXO-902589` portait une piste marquée « non vérifiée, à éprouver et non à croire — deux points
ne font pas une période ». Éprouvée.

1 639 appels en `curl` direct, hors client MCP donc à coût de contexte nul : p50 15-17 ms,
**12 aberrants** de 0,7 à 1,7 s, **tous** commencés entre 75 et 89 ms après la seconde ronde. Douze
points sur une grille de 1 Hz à ±7 ms.

Puis le geste décisif : rejouer sur `help()`, qui ne touche ni base ni verrou applicatif. Il bloque
**identiquement**. L'hypothèse naturelle — deux chemins partageant `latest_lifecycle_heartbeat` —
était morte, et la cause déplacée vers le worker tokio. Le suspect était alors visible à la lecture :
`spawn_runtime_telemetry` tique à 1 Hz et exécute ~240 lignes **synchrones** dans une tâche `spawn`.

La preuve du correctif reste à faire après promote, et elle est écrite pour être falsifiable : zéro
aberrant aligné sur `t%1000 ∈ [70, 95]`.

## Le nettoyage SOLL a réfuté sa propre prémisse

L'intention était de fermer en lot les exigences partielles. Tri mécanique : sur 98 preuves de type
commit, 13 exigences sont déclarées dans un **titre** de commit, 24 seulement citées en corps.

Puis quatre vérifications, quatre réfutations. `902275` dit « le bash ne porte plus de politique »
alors que `axon-resource-policy.sh` fait encore 333 lignes. `902326` porte dans ses **propres
critères** la mention « REFUSÉ APRÈS MESURE ». La SOLL n'était pas sale, elle était honnête.

Ce qui a été nettoyé, en revanche : le graphe est **acyclique**. L'arête
`REQ-AXO-91498 REFINES REQ-AXO-325` — un nœud remplacé qui raffinait son remplaçant — a été retirée,
et `DEC-AXO-098` devient activable : le validateur de cycles exigeait 0 cycle pour s'armer.

## Six frictions MRG, vérifiées une par une

Toutes réelles. Deux méritent d'être retenues au-delà de leur correction.

Le pre-flight rend « Validation passed » et rien d'autre. Sa description dit honnêtement qu'il ne
lance ni test ni formateur — mais personne ne relit une description après un vert. Un tenant a
commité un `devenv.nix` qui ne s'évalue plus, et l'a découvert trois tours plus tard.

Le bundle d'ouverture pèse **105 677 caractères** mesurés, pas les 10 k annoncés. Son premier poste,
`soll_skeleton` à 67 371, n'était dans aucun plan d'optimisation.

Et un retour positif qui vaut garde-fou : *« deux fois aujourd'hui, la discipline SOLL m'a forcé à
inscrire une réfutation plutôt qu'à l'oublier — dont une où j'avais tort »*. La réduction de facture
doit porter sur l'entrée, jamais sur les surfaces d'écriture.

## Une erreur, et sa leçon

Un script de falsification faisait `git stash push` puis `pop`. Le push n'a rien empilé — un fichier
non suivi n'est pas stashable sans `-u` — et le pop a dépilé un stash vieux de plusieurs sessions, sur
des chemins renommés depuis : 12 fichiers pollués, 9 en conflit.

`git stash pop` est un *pop*, pas un « annule mon push ». Il ne sait pas ce qu'on croyait avoir
empilé. Réparé sans perte : un pop conflictuel ne supprime pas le stash — vérifié **avant** de jeter
la copie de travail.

## Pratiques déposées

`2177` lire une valeur machine par sa forme · `2182` jamais de `git stash` dans un script ·
`2184` un commit qui nomme une exigence ne prouve pas qu'elle est finie · `2185` diagnostiquer une
queue de latence sans instrumenter.
