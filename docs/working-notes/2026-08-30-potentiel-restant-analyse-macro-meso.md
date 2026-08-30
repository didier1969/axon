# Potentiel restant d'Axon — analyse macro et méso

Session 132 · 2026-08-30 · build `v0.8.0-1663-g05fa97af`
Page lisible : https://claude.ai/code/artifact/00e7887e-89f5-4800-8203-f34c8c2eed5f

## Verdict

Le produit existe et sert. **Il ne survit pas seul** — et c'est la seule chose qui l'empêche
de quitter l'atelier. L'indexeur est mort le 29/08 à 22h36 (signal externe) ; 17 h plus tard
il l'était encore, sans alerte, l'IST gelée avec lui.

## Acquis, compté

| | |
|---|---|
| Dépôts portant une mémoire d'intention Axon | **128** (sur 147 dossiers rendus) |
| Nœuds d'intention, tous projets | **7 617** — AXO 1 853, OPV 1 126, FSF 686, APS 666, NEX 360 |
| Outils MCP publics | **114**, deux transports |
| Exigences AXO | 1 312 dont **1 234 livrées**, 75 partielles, 3 manquantes |
| Couverture pondérée par centralité | **91,2 %** de la masse PageRank testée |
| Stubs `todo!()` / `unimplemented!()` | **0** |

## Santé structurelle — `structural_health_index` = 0,783

Cinq axes tiennent leur cible : `impact_radius` 0,997 · `acyclicity` 0,994 ·
`god_objects` 0,964 · `weighted_coverage` 0,912 · `module_depth` 0,878.

Quatre sont sous cible :

| Axe | Valeur | Cible | Détail |
|---|---|---|---|
| `duplication` | **0,402** | 0,90 | 3 192 paires quasi-doublons / 5 341 symboles testables |
| `intent_alignment` | **0,635** | 0,85 | **636 / 1 743 nœuds SOLL sans trace de code** |
| `main_sequence` | **0,648** | 0,75 | D=0,352 sur 359 modules couplés · Δ = −1e-16, `re_surfaced` |
| `resilience` | **0,878** | 0,95 | 3 812 points d'articulation / 31 336 |

Lecture : la duplication est le pire chiffre mais le moins urgent — `debt_digest` écarte déjà
174 paires comme inouvrables et les 5 plus centrales sont des diagrammes Mermaid en HTML.
`main_sequence` est le seul axe qui régresse par immobilité. `intent_alignment` est le seul
qui parle du produit lui-même.

## Les trois verrous

1. **Exploitation** — le runtime ne se relève pas seul (`REQ-AXO-902348` panne muette ·
   `REQ-AXO-902233` promote = coupure de plusieurs minutes · `REQ-AXO-902543` build de
   promotion non reproductible, bloqué à l'extérieur).
   *Libère* : l'installation chez un tiers.
2. **Preuve** — `intent_alignment` 0,635 : Axon relie intention et code sur moins des deux
   tiers de sa propre base ; 141 exigences ouvertes sans preuve.
   *Libère* : l'argument qu'aucun concurrent ne peut copier sans la même discipline.
3. **Contrat** — 128 tenants consomment la méthodologie, `MIL-AXO-056` reste à 10/17.
   *Libère* : le passage de l'outil au produit, au meilleur rapport levier/effort.

## Les quatre jalons

| Jalon | Ouvertes | Pari | Coût de l'inaction |
|---|---|---|---|
| `MIL-AXO-053` | 6/12 (1 bloquée) | la base est fiable | tout le reste repose sur du sable |
| `MIL-AXO-055` | 19/40 | le runtime survit et se promeut | pas d'adoption tierce |
| `MIL-AXO-054` | 25/64 | les surfaces disent vrai | un agent code contre des chiffres faux |
| `MIL-AXO-056` | 10/17 | la méthode devient le produit | 128 tenants sans engagement |

Ordre par dépendance : 053 → 055 → 054 → 056. Ordre par levier : **055 en priorité**
(seul à débloquer l'adoption), **056 en parallèle** (petit, sans dépendance, bénéficiaires
existants), 054 au fil des sessions.

## Le contre-exemple trouvé dans la même session

`CPT-AXO-054`, statut `current`, décrit un étage B1, un canal `try_send` et
`AXON_B1_WORKERS` — les trois retirés du code, ce que l'orchestrateur écrit dans ses propres
commentaires. Un agent lisant le *pourquoi* canonique coderait contre un fantôme : le mode
d'échec exact que la vision prétend supprimer, reproduit chez soi. `REQ-AXO-902572`.

## Réserve de mesure

L'IST est **gelée depuis le 29/08 22h36** ; code-intel 856/918. `structural_health_index`,
`debt_digest` et `project_status` décrivent cet état, pas celui d'aujourd'hui. L'indexeur
n'a pas été relancé : 19,7 Gio libres, swap 6,6/8 Gio, et il est mort par pression mémoire —
rafraîchir la mesure au prix d'une seconde panne était un mauvais échange. Les données SOLL
sont lues en direct.

Aucun pourcentage global d'avancement n'est proposé ; la dette brute (2 848 paires) n'est pas
présentée comme 2 848 tâches.
