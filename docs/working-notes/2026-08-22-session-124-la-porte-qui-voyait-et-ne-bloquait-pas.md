# Session 124 — la porte qui voyait et ne bloquait pas

**Audit only.** La vérité canonique est `CPT-AXO-052` (section « 🔴 CLÔTURE s124 ») et les REQ cités.
HEAD `43880d41` · live `v0.8.0-1586-g43880d41` (aligné) · `axon_handoff_check` = **FAIL, 240 violations**.

## Le fil

L'opérateur a répété trois fois la même consigne : *« implémenter le rule engine, pas corriger »*, et
*« le handoff ne pourra pas se faire tant que les règles ne seront pas respectées »*.

À la troisième, j'ai cherché **pourquoi** le handoff ne les respectait pas — au lieu de continuer à
poser des règles. `axon_handoff_check` (`tools_framework.rs:378`) lisait `soll_validate`, voyait le
verdict, et faisait `warns += 1`. **Jamais `fails += 1`.** Avec 484 violations, la porte rendait
« WARN, 0 fail ».

La directive ne vivait que dans une practice (`#1378`) et dans la mémoire du LLM qui la lit. C'est
exactement ce que `GUI-PRO-118` interdit : un geste qu'il faut se *rappeler* d'appliquer n'est pas
une porte, c'est une intention.

## Ce qui a été livré

| commit | objet |
|---|---|
| `a6a55099` | 4 axes du moteur sans règle en reçoivent une ; `duplicate_titles` migré, son SQL retiré |
| `ee3835f6` | direction `either` — les 3 derniers checks de rattachement deviennent des règles |
| `4a1a8749` | `GUI-PRO-124/125` avaient **zéro garde** |
| `2e9ee329` | **la porte REFUSE** une violation de règle |
| `43880d41` | `GUI-PRO-119/120` ignoraient `rejected`/`archived` comme retraits |

**13 règles, 8 prédicats.** Promote `v0.8.0-1586-g43880d41`, coupure 16 s.

## Trois choses trouvées en faisant, pas en relisant

**1. « Combien de règles sont implémentées ET validées ? »** — la question de l'opérateur a révélé
11 implémentées, **9 validées**. `GUI-PRO-124` et `GUI-PRO-125` n'étaient nommées par aucun test. Je
les avais déplacées sous `PRO` sans écrire de garde : j'avais appliqué « poser la première règle est
le test d'acceptation » aux 4 axes neufs, pas aux 2 que je me contentais de déplacer. Elles portaient
164 des 182 violations AXO — si elles avaient cessé de charger, le compte serait tombé à 18 en
silence.

**2. Réparer éprouve la règle.** Sur les 8 arêtes de `GUI-PRO-120` : 6 vraies inversions, 1 stub à
titre et corps vides, et 1 dont les deux extrémités étaient retirées — la cible était `rejected`, et
la règle ne comptait que `superseded`. Appliquer le remède prescrit aurait remis un nœud rejeté à
`current` : falsifier le registre pour verdir une porte.

**3. Un gate conditionné à sa propre cible ne s'arme jamais.** `soll_acyclic_audit` répétait à chaque
appel *« requires these to be 0 »* — et il y a 3 cycles. Il attendait un zéro que lui seul aurait pu
produire. `GUI-PRO-131` inverse le sens : elle signale au lieu d'attendre.

## Le promote — première tentative échouée

Étape 4 : `Workspace artifact drift`. Le correctif de `REQ-AXO-902454` avait déplacé le preflight en
1b (qui passe), mais `create_manifest.py:97` appelle `preflight.sh` **pour son propre compte** à
l'étape 4, après que l'étape 2 a recompilé dans le même target. Une porte dupliquée à deux endroits :
en corriger un laisse l'autre mordre. → `REQ-AXO-902460` (P0).

Aucune coupure pendant l'échec — le live a rendu HTTP 200 tout du long. Danger transitoire réel :
`bin/` portait le candidat, pas le build servi. Résolu par le second promote.

## Ce que j'ai fait de travers

- **J'ai dérivé vers la correction** alors que la consigne était de poser les règles. `REQ-AXO-902457`
  (le classifieur de preuves) est une correction que je n'aurais pas dû écrire. Reconnu, gardé parce
  que sans elle `GUI-PRO-124` comptait 4 faux positifs, mais c'est une dérive.
- **J'ai d'abord conclu « bruit syntaxique »** sur les cibles d'arêtes non résolues, en regardant deux
  exemples qui étaient le min et le max alphabétiques. Ce sont des appels stdlib. Corrigé avant de
  livrer la conclusion.
- **Une falsification n'a pas mordu** : un commentaire inséré dans une chaîne Rust a cassé la
  compilation, donc aucun test n'a tourné, et j'ai failli lire « pas d'échec » comme « le gate est
  bon ». → `#1380`, appliqué ensuite systématiquement.
- **J'ai oublié la moitié du handoff** au premier passage : `MEMORY.md` restait sur la s123 (live
  faux, « aucun promote », 174 violations), le titre du `session_pointer` sur la s122, et cette note
  n'existait pas. L'opérateur a demandé « tu n'as rien oublié ? ». C'est le défaut d'origine que
  `GUI-PRO-028` existe pour empêcher.

## Reste ouvert

240 violations à réparer (dont 60 gatées opérateur) · `REQ-AXO-902460` (P0) · **V4 : le moteur IST
est à 0 règle sur 254**, bloqué par ~16 530 fichiers sous un bail à `lease_until_ms = 0` et 87,6 %
d'arêtes `CALLS` fantômes.
